#include "modloader_rt64.h"

#include <algorithm>
#include <memory>
#include <mutex>
#include <stdio.h>
#include <string.h>
#include <string>
#include <vector>

#include "gbi/rt64_gbi.h"
#include "hle/rt64_application.h"
#include "plume_vulkan.h"
#include "swap_chain.h"

namespace {
constexpr uint32_t gAddressMask = 0x7FFFFF8;
constexpr uint32_t gDmemMask = 0xFF8;
constexpr uint32_t gDpStatusXbus = 1 << 0;

std::unique_ptr<RT64::Application> sApplication;
RT64_CORE sCore;
ModLoader_Swap_Chain_Config sSwapChain;
uint32_t sInterrupts;
std::vector<uint32_t> sXbusCommands;

struct Swap_Chain_Size_Config {
    RT64::UserConfiguration::Resolution resolution;
    RT64::UserConfiguration::AspectRatio aspectRatio;
    double resolutionMultiplier;
    double aspectTarget;
};

std::mutex sSizeMutex;
Swap_Chain_Size_Config sSizeConfig;

void Update_Size_Config(const RT64::UserConfiguration* user) {
    std::lock_guard lock(sSizeMutex);

    sSizeConfig.resolution = user->resolution;
    sSizeConfig.aspectRatio = user->aspectRatio;
    sSizeConfig.resolutionMultiplier = user->resolutionMultiplier;
    sSizeConfig.aspectTarget = user->aspectTarget;
}

void Check_Interrupts() { }

void Read_User_Config(const char* text, RT64::UserConfiguration* out_config) {
    json object = json::parse(text != nullptr ? text : "", nullptr, false);

    if (object.is_object()) {
        RT64::from_json(object, *out_config);
    }
    out_config->validate();
}

bool Client_Device(const uint8_t* uuid, std::string* out_name) {
    VkApplicationInfo application = { VK_STRUCTURE_TYPE_APPLICATION_INFO };
    VkInstanceCreateInfo info = { VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO };
    VkInstance instance = VK_NULL_HANDLE;
    PFN_vkEnumeratePhysicalDevices enumerate;
    PFN_vkGetPhysicalDeviceProperties2 properties2;
    PFN_vkDestroyInstance destroy;
    VkPhysicalDevice devices[16];
    uint32_t count = 16;
    bool found = false;

    if (volkInitialize() != VK_SUCCESS) {
        return false;
    }

    application.apiVersion = VK_API_VERSION_1_1;
    info.pApplicationInfo = &application;
    if (vkCreateInstance(&info, nullptr, &instance) != VK_SUCCESS) {
        return false;
    }

    enumerate = reinterpret_cast<PFN_vkEnumeratePhysicalDevices>(vkGetInstanceProcAddr(instance, "vkEnumeratePhysicalDevices"));
    properties2 = reinterpret_cast<PFN_vkGetPhysicalDeviceProperties2>(vkGetInstanceProcAddr(instance, "vkGetPhysicalDeviceProperties2"));
    destroy = reinterpret_cast<PFN_vkDestroyInstance>(vkGetInstanceProcAddr(instance, "vkDestroyInstance"));
    if (enumerate(instance, &count, devices) >= VK_SUCCESS) {
        for (uint32_t index = 0; index < count && !found; index++) {
            VkPhysicalDeviceIDProperties id = { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES };
            VkPhysicalDeviceProperties2 properties = { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2 };

            properties.pNext = &id;
            properties2(devices[index], &properties);
            if (memcmp(id.deviceUUID, uuid, VK_UUID_SIZE) == 0) {
                *out_name = properties.properties.deviceName;
                memcpy(sSwapChain.clientLuid, id.deviceLUID, sizeof(sSwapChain.clientLuid));
                sSwapChain.clientLuidValid = id.deviceLUIDValid;
                found = true;
            }
        }
    }

    destroy(instance, nullptr);
    return found;
}

std::unique_ptr<RenderSwapChain> Create_Swap_Chain(RenderDevice* device, RenderCommandQueue* queue, RenderSwapChainDesc& desc, bool uses_hdr) {
#if defined(_WIN32)
    if (sApplication->chosenGraphicsAPI == RT64::UserConfiguration::GraphicsAPI::D3D12) {
        return D3D12_Swap_Chain_Create(device, queue, desc, uses_hdr, sSwapChain);
    }
#endif
    if (sApplication->chosenGraphicsAPI == RT64::UserConfiguration::GraphicsAPI::Vulkan) {
        return Vulkan_Swap_Chain_Create(device, queue, desc, uses_hdr, sSwapChain);
    }
    return nullptr;
}

} // namespace


void Swap_Chain_Size(const ModLoader_Swap_Chain_Config& config, uint32_t* width, uint32_t* height) {
    Swap_Chain_Size_Config user;
    uint32_t view_width = 0;
    uint32_t view_height = 0;
    bool view = config.outputSize != nullptr && config.outputSize(config.user, &view_width, &view_height) && view_width != 0 && view_height != 0;
    double scale = 1.0;
    double aspect = 4.0 / 3.0;

    {
        std::lock_guard lock(sSizeMutex);

        user = sSizeConfig;
    }
    if (user.resolution == RT64::UserConfiguration::Resolution::Manual) {
        scale = user.resolutionMultiplier;
    }
    else if (user.resolution == RT64::UserConfiguration::Resolution::WindowIntegerScale && view) {
        scale = std::max((view_height + 239) / 240, 1u);
    }

    if (user.aspectRatio == RT64::UserConfiguration::AspectRatio::Expand && view) {
        aspect = double(view_width) / view_height;
    }
    else if (user.aspectRatio == RT64::UserConfiguration::AspectRatio::Manual) {
        aspect = user.aspectTarget;
    }

    *height = uint32_t(240.0 * scale + 0.5);
    *width = uint32_t(*height * aspect + 0.5);
}

extern "C" int32_t rt64_open(const RT64_CORE* core, const RT64_CONFIG* config) {
    RT64::Application::Core application_core = {};
    RT64::ApplicationConfiguration application_config;
    std::string device_name;

    sCore = *core;
    sSwapChain.frameCallback = config->frame_callback;
    sSwapChain.outputSize = config->output_size;
    sSwapChain.user = config->user;
    sSwapChain.firstValue = config->first_value;
    memcpy(sSwapChain.clientUuid, config->gpu_uuid, sizeof(sSwapChain.clientUuid));
    if (!Client_Device(config->gpu_uuid, &device_name)) {
        fprintf(stderr, "ModLoader: !Client_Device(config->gpu_uuid, &device_name)\n");
        return 0;
    }

    application_core.HEADER = core->rom_header;
    application_core.RDRAM = core->rdram;
    application_core.DMEM = core->dmem;
    application_core.IMEM = core->imem;
    application_core.MI_INTR_REG = &sInterrupts;
    application_core.DPC_START_REG = &core->dpc_regs[0];
    application_core.DPC_END_REG = &core->dpc_regs[1];
    application_core.DPC_CURRENT_REG = &core->dpc_regs[2];
    application_core.DPC_STATUS_REG = &core->dpc_regs[3];
    application_core.DPC_CLOCK_REG = &core->dpc_regs[4];
    application_core.DPC_BUFBUSY_REG = &core->dpc_regs[5];
    application_core.DPC_PIPEBUSY_REG = &core->dpc_regs[6];
    application_core.DPC_TMEM_REG = &core->dpc_regs[7];
    application_core.VI_STATUS_REG = &core->vi_regs[0];
    application_core.VI_ORIGIN_REG = &core->vi_regs[1];
    application_core.VI_WIDTH_REG = &core->vi_regs[2];
    application_core.VI_INTR_REG = &core->vi_regs[3];
    application_core.VI_V_CURRENT_LINE_REG = &core->vi_regs[4];
    application_core.VI_TIMING_REG = &core->vi_regs[5];
    application_core.VI_V_SYNC_REG = &core->vi_regs[6];
    application_core.VI_H_SYNC_REG = &core->vi_regs[7];
    application_core.VI_LEAP_REG = &core->vi_regs[8];
    application_core.VI_H_START_REG = &core->vi_regs[9];
    application_core.VI_V_START_REG = &core->vi_regs[10];
    application_core.VI_V_BURST_REG = &core->vi_regs[11];
    application_core.VI_X_SCALE_REG = &core->vi_regs[12];
    application_core.VI_Y_SCALE_REG = &core->vi_regs[13];
    application_core.checkInterrupts = Check_Interrupts;
    application_config.dataPath = config->data_path;
    application_config.detectDataPath = false;
    application_config.useConfigurationFile = false;
    application_config.preferredDeviceName = device_name;
    application_config.createSwapChain = Create_Swap_Chain;
    sApplication = std::make_unique<RT64::Application>(application_core, application_config);
    Read_User_Config(config->user_config, &sApplication->userConfig);
    if (RT64::UserConfiguration::resolveGraphicsAPI(sApplication->userConfig.graphicsAPI) == RT64::UserConfiguration::GraphicsAPI::Metal) {
        fprintf(stderr, "TODO: METAL\n");
        rt64_close();
        return 0;
    }
    Update_Size_Config(&sApplication->userConfig);
    sApplication->enhancementConfig.presentation.mode = RT64::EnhancementConfiguration::Presentation::Mode::Console; // Fixes some oddity with SM64
    if (sApplication->setup(0) != RT64::Application::SetupResult::Success || sApplication->swapChain == nullptr) {
        fprintf(stderr, "sApplication->setup(0) != RT64::Application::SetupResult::Success || sApplication->swapChain == nullptr\n");
        rt64_close();
        return 0;
    }

    sApplication->state->pauseOnSelfBranch = true;

    return 1;
}

extern "C" void rt64_close() {
    if (sApplication != nullptr) {
        rt64_commit();
        sApplication->end();
        sApplication.reset();
    }
}

extern "C" void rt64_commit() {
    if (sApplication != nullptr && sApplication->renderInterface != nullptr && sApplication->chosenGraphicsAPI == RT64::UserConfiguration::GraphicsAPI::Vulkan) {
        volkLoadInstance(static_cast<plume::VulkanInterface*>(sApplication->renderInterface.get())->instance);
    }
}

extern "C" int32_t rt64_known_ucode(const uint8_t* rdram, uint32_t text, uint32_t data) {
    if (sApplication == nullptr) {
        return 0;
    }
    RT64::GBI* gbi = sApplication->interpreter->gbiManager.getGBIForUCode(const_cast<uint8_t*>(rdram), text & gAddressMask, data & gAddressMask);

    return gbi != nullptr && gbi->ucode != RT64::GBIUCode::Unknown;
}

static uint32_t Run_Display_List(uint32_t address) {
    RT64::State* state = sApplication->state.get();
    uint32_t flags;

    sInterrupts = 0;
    state->spinAddress = UINT32_MAX;
    sApplication->processDisplayLists(sCore.rdram, address & gAddressMask, 0, true);
    flags = sInterrupts;
    sInterrupts = 0;
    if (state->spinAddress != UINT32_MAX) {
        flags = (flags & ~RT64_INTERRUPT_SP) | RT64_TASK_WAITING;
    }
    return flags;
}

extern "C" uint32_t rt64_run_task(uint32_t text, uint32_t data, uint32_t data_ptr) {
    sApplication->interpreter->loadUCodeGBI(text & gAddressMask, data & gAddressMask, true);
    return Run_Display_List(data_ptr);
}

extern "C" uint32_t rt64_resume_task() {
    return Run_Display_List(sApplication->state->spinAddress);
}

extern "C" uint32_t rt64_waiting_address() {
    return sApplication->state->spinAddress;
}

extern "C" uint32_t rt64_process_rdp() {
    uint32_t* dpc = sCore.dpc_regs;
    uint32_t current = dpc[2] & gAddressMask;
    uint32_t end = dpc[1] & gAddressMask;
    uint32_t interrupts;

    if (end <= current) {
        return 0;
    }

    sInterrupts = 0;
    if ((dpc[3] & gDpStatusXbus) != 0) {
        sXbusCommands.resize((end - current) / 4);
        for (uint32_t index = 0; index < sXbusCommands.size(); index++) {
            const uint8_t* word = sCore.dmem + ((current + index * 4) & (gDmemMask | 4));
            sXbusCommands[index] = uint32_t(word[0]) << 24 | uint32_t(word[1]) << 16 | uint32_t(word[2]) << 8 | word[3];
        }
        sApplication->processDisplayLists(reinterpret_cast<uint8_t*>(sXbusCommands.data()), 0, end - current, false);
    }
    else {
        sApplication->processDisplayLists(sCore.rdram, current, end, false);
    }

    dpc[0] = dpc[1];
    dpc[2] = dpc[1];
    interrupts = sInterrupts;
    sInterrupts = 0;
    return interrupts;
}

extern "C" void rt64_update_screen() {
    sApplication->updateScreen();
}

extern "C" void rt64_discard() {
    if (sApplication != nullptr) {
        sApplication->updateUserConfig(true);
    }
}

extern "C" void rt64_set_user_config(const char* user_config) {
    uint32_t samples;

    if (sApplication == nullptr) {
        return;
    }

    samples = sApplication->userConfig.msaaSampleCount();
    Read_User_Config(user_config, &sApplication->userConfig);
    Update_Size_Config(&sApplication->userConfig);
    if (sApplication->userConfig.msaaSampleCount() != samples) {
        sApplication->updateMultisampling();
    }

    sApplication->updateUserConfig(true);
}
