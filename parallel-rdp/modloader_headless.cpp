#include "modloader_headless.h"
#include "presentation.hpp"
#include "logging.hpp"
#include <string.h>
#include <vector>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace {
struct Shared_Frames {
    modloader_shared_frame_callback callback;
    void* user;
    uint8_t uuid[16];
    bool active;
    Vulkan::ImageHandle images[3];
    Vulkan::ExternalHandle memory[3];
    uint32_t ids[3];
    uint64_t release_values[3];
    unsigned width;
    unsigned height;
    unsigned next;
    uint32_t next_id;
    Vulkan::Semaphore ready;
    Vulkan::Semaphore release;
    Vulkan::ExternalHandle ready_handle;
    Vulkan::ExternalHandle release_handle;
    uint64_t ready_value;
};

Shared_Frames sShared;
rdp_frame_callback sFrameCallback;
void* sFrameUser;
std::vector<RDP::RGBA> sPixels;

void Close_Handle(Vulkan::ExternalHandle& handle) {
#ifdef _WIN32
    if (handle.handle) {
        CloseHandle(handle.handle);
    }
    handle.handle = nullptr;
#else
    if (handle.handle >= 0) {
        close(handle.handle);
    }
    handle.handle = -1;
#endif
}

bool Init_Context(Vulkan::Context& context) {
    VkPhysicalDevice gpus[16];
    uint32_t count = 16;
    VkPhysicalDevice chosen = VK_NULL_HANDLE;

    if (!context.init_instance(nullptr, 0)) {
        return false;
    }

    if (vkEnumeratePhysicalDevices(context.get_instance(), &count, gpus) < VK_SUCCESS) {
        return false;
    }
    for (uint32_t index = 0; sShared.callback && index < count && !chosen; index++) {
        VkPhysicalDeviceIDProperties ids = { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES };
        VkPhysicalDeviceProperties2 properties = { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2 };

        properties.pNext = &ids;
        vkGetPhysicalDeviceProperties2(gpus[index], &properties);
        if (memcmp(ids.deviceUUID, sShared.uuid, VK_UUID_SIZE) == 0) {
            chosen = gpus[index];
        }
    }

    if (!chosen) {
        sShared.callback = nullptr;
        sShared.user = nullptr;
    }
    return context.init_device(chosen, VK_NULL_HANDLE, nullptr, 0);
}

void Init_Shared_Frames(Vulkan::Device& device) {
    auto handle_type = Vulkan::ExternalHandle::get_opaque_semaphore_handle_type();

    if (!sShared.callback || !device.get_device_features().supports_external) {
        return;
    }

    sShared.ready = device.request_semaphore_external(VK_SEMAPHORE_TYPE_TIMELINE, handle_type);
    sShared.release = device.request_semaphore_external(VK_SEMAPHORE_TYPE_TIMELINE, handle_type);
    if (!sShared.ready || !sShared.release) {
        return;
    }

    sShared.ready_handle = sShared.ready->export_to_handle();
    sShared.release_handle = sShared.release->export_to_handle();
    sShared.active = sShared.ready_handle && sShared.release_handle;
}

bool Make_Shared_Images(Vulkan::Device& device, unsigned width, unsigned height) {
    device.wait_idle();
    for (unsigned index = 0; index < 3; index++) {
        Vulkan::ImageCreateInfo info = {};

        sShared.images[index].reset();
        Close_Handle(sShared.memory[index]);
        info.width = width;
        info.height = height;
        info.depth = 1;
        info.levels = 1;
        info.layers = 1;
        info.format = VK_FORMAT_R8G8B8A8_UNORM;
        info.type = VK_IMAGE_TYPE_2D;
        info.usage = VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
        info.initial_layout = VK_IMAGE_LAYOUT_UNDEFINED;
        info.domain = Vulkan::ImageDomain::Physical;
        info.misc = Vulkan::IMAGE_MISC_EXTERNAL_MEMORY_BIT;
        sShared.images[index] = device.create_image(info);
        if (!sShared.images[index]) {
            return false;
        }

        sShared.memory[index] = sShared.images[index]->export_handle();
        if (!sShared.memory[index]) {
            return false;
        }
        sShared.ids[index] = ++sShared.next_id;
        sShared.release_values[index] = 0;
    }

    sShared.width = width;
    sShared.height = height;
    return true;
}

unsigned Free_Slot() {
    for (unsigned step = 0; step < 3; step++) {
        unsigned slot = (sShared.next + step) % 3;
        if (sShared.release_values[slot] == 0 || sShared.release->wait_timeline_timeout(sShared.release_values[slot], 0)) {
            return slot;
        }
    }
    return 3;
}

bool Share_Frame(Vulkan::Device& device, RDP::CommandProcessor& processor, const RDP::ScanoutOptions& options) {
    Vulkan::ImageHandle image = processor.scanout(options);
    ModLoader_Shared_Frame frame = {};
    unsigned width;
    unsigned height;
    unsigned slot;
    bool shared_before;

    if (!image) {
        return true;
    }

    width = image->get_width();
    height = image->get_height();
    if ((width != sShared.width || height != sShared.height) && !Make_Shared_Images(device, width, height)) {
        return false;
    }

    slot = Free_Slot();
    if (slot == 3) {
        return true;
    }

    sShared.next = (slot + 1) % 3;
    auto& target = *sShared.images[slot];

    shared_before = sShared.release_values[slot] != 0;
    if (shared_before) {
        auto wait = device.request_timeline_semaphore_as_binary(*sShared.release, sShared.release_values[slot]);
        wait->signal_external();
        device.add_wait_semaphore(Vulkan::CommandBuffer::Type::Generic, std::move(wait), VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT, true);
    }

    auto cmd = device.request_command_buffer();
    if (shared_before) {
        cmd->acquire_image_barrier(
            target,
            VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
            VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
            VK_PIPELINE_STAGE_2_COPY_BIT,
            VK_ACCESS_2_TRANSFER_WRITE_BIT
        );
    }
    cmd->image_barrier(
        target,
        shared_before ? VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL : VK_IMAGE_LAYOUT_UNDEFINED,
        VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        shared_before ? VK_PIPELINE_STAGE_2_COPY_BIT : VK_PIPELINE_STAGE_2_NONE,
        0,
        VK_PIPELINE_STAGE_2_COPY_BIT,
        VK_ACCESS_2_TRANSFER_WRITE_BIT
    );

    cmd->image_barrier(
        *image,
        VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT,
        VK_ACCESS_2_MEMORY_WRITE_BIT,
        VK_PIPELINE_STAGE_2_COPY_BIT,
        VK_ACCESS_2_TRANSFER_READ_BIT
    );
    cmd->copy_image(target, *image);
    cmd->image_barrier(
        *image,
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        VK_PIPELINE_STAGE_2_COPY_BIT,
        0,
        VK_PIPELINE_STAGE_2_FRAGMENT_SHADER_BIT | VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT,
        VK_ACCESS_2_SHADER_SAMPLED_READ_BIT
    );
    cmd->image_barrier(
        target,
        VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        VK_PIPELINE_STAGE_2_COPY_BIT,
        VK_ACCESS_2_TRANSFER_WRITE_BIT,
        VK_PIPELINE_STAGE_2_NONE,
        0
    );
    cmd->release_image_barrier(
        target,
        VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT,
        VK_ACCESS_2_MEMORY_WRITE_BIT
    );

    device.submit(cmd);
    auto signal = device.request_timeline_semaphore_as_binary(*sShared.ready, ++sShared.ready_value);
    device.submit_empty(Vulkan::CommandBuffer::Type::Generic, nullptr, signal.get());
    sShared.release_values[slot] = sShared.ready_value;

    memcpy(frame.device_uuid, sShared.uuid, sizeof(frame.device_uuid));
    frame.memory_handle = Renderer_Handle_Value(sShared.memory[slot].handle);
    frame.memory_size = target.get_allocation().get_size();
    frame.handle_type = MODLOADER_RENDERER_HANDLE_VULKAN;
    frame.image_id = sShared.ids[slot];
    frame.width = width;
    frame.height = height;
    frame.format = MODLOADER_RENDERER_RGBA8;
    frame.ready_handle = Renderer_Handle_Value(sShared.ready_handle.handle);
    frame.ready_value = sShared.ready_value;
    frame.release_handle = Renderer_Handle_Value(sShared.release_handle.handle);
    frame.release_value = sShared.ready_value;
    sShared.callback(sShared.user, &frame);
    return true;
}
} // namespace

extern "C" void rdp_set_frame_callback(rdp_frame_callback callback, void* user) {
    sFrameCallback = callback;
    sFrameUser = user;
}

extern "C" void rdp_set_shared_frames(const uint8_t* gpu_uuid, modloader_shared_frame_callback callback, void* user, uint64_t first_value) {
    sShared.callback = callback;
    sShared.user = user;
    sShared.ready_value = first_value > sShared.ready_value ? first_value : sShared.ready_value;
    if (gpu_uuid != nullptr) {
        memcpy(sShared.uuid, gpu_uuid, sizeof(sShared.uuid));
    }
    else {
        memset(sShared.uuid, 0, sizeof(sShared.uuid));
    }
    if (callback == nullptr) {
        sShared.active = false;
    }
}

JoystickEvent get_joystick_event() {
    return JoystickEvent{ 0, false };
}

void rdp_onscreen_message(const char* message, MESSAGE_LENGTH milliseconds) {
    LOGI("%s\n", message);
}

void rdp_set_fps(uint32_t fps, uint32_t vis) {
}

bool RDP_Presentation_Open(RDP_Presentation* state, void* window,
                           GFX_INFO* graphics, CALL_BACK* callback,
                           const void* font, size_t font_size) {
    if (!Vulkan::Context::init_loader(nullptr)) {
        LOGE("!Vulkan::Context::init_loader(nullptr)\n");
        return false;
    }

    auto* context = new Vulkan::Context;
    state->backend = context;
    if (!Init_Context(*context)) {
        LOGE("!Init_Context(*context)\n");
        return false;
    }

    state->device = new Vulkan::Device;
    state->device->set_context(*context);
    state->synchronizeOnRead = true;
    Init_Shared_Frames(*state->device);
    return true;
}

bool RDP_Presentation_Begin(RDP_Presentation* state) {
    return state->device != nullptr;
}

void RDP_Presentation_End(RDP_Presentation* state) {
}

void RDP_Presentation_Close(RDP_Presentation* state) {
    rdp_set_frame_callback(nullptr, nullptr);
    rdp_set_shared_frames(nullptr, nullptr, nullptr, 0);
    for (unsigned index = 0; index < 3; index++) {
        sShared.images[index].reset();
        Close_Handle(sShared.memory[index]);
    }

    sShared.ready.reset();
    sShared.release.reset();
    Close_Handle(sShared.ready_handle);
    Close_Handle(sShared.release_handle);
    sShared.active = false;
    sShared.width = 0;
    sShared.height = 0;
    delete state->device;
    delete static_cast<Vulkan::Context*>(state->backend);
    state->device = nullptr;
    state->backend = nullptr;
}

void RDP_Presentation_Render(RDP_Presentation* state,
                             RDP::CommandProcessor* processor,
                             const RDP::ScanoutOptions* options) {
    unsigned width = 0;
    unsigned height = 0;

    if (sShared.active) {
        if (Share_Frame(*state->device, *processor, *options)) {
            return;
        }
        LOGE("readback\n");
        sShared.active = false;
    }

    if (!sFrameCallback) {
        return;
    }

    processor->scanout_sync(sPixels, width, height, *options);
    if (width != 0 && height != 0) {
        sFrameCallback(sFrameUser, reinterpret_cast<const uint8_t*>(sPixels.data()), width, height, MODLOADER_RENDERER_RGBA8);
    }
}

void RDP_Presentation_Update(RDP_Presentation* state) {
    state->device->next_frame_context();
}
