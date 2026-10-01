#include "swap_chain.h"

#include <mutex>
#include <stdio.h>
#include <string.h>
#if !defined(_WIN32)
#include <unistd.h>
#endif

#include "plume_vulkan.h"

namespace {
#if defined(_WIN32)
constexpr VkExternalMemoryHandleTypeFlagBits gMemoryHandleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_WIN32_BIT;
constexpr VkExternalSemaphoreHandleTypeFlagBits gSemaphoreHandleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_WIN32_BIT;
#else
constexpr VkExternalMemoryHandleTypeFlagBits gMemoryHandleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT;
constexpr VkExternalSemaphoreHandleTypeFlagBits gSemaphoreHandleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_FD_BIT;
#endif

constexpr uint32_t gMaxWaits = 8;

struct Exported_Handle {
    uint64_t value = UINT64_MAX;
};

uint64_t Export_Memory(VkDevice device, VkDeviceMemory memory) {
#if defined(_WIN32)
    VkMemoryGetWin32HandleInfoKHR info = { VK_STRUCTURE_TYPE_MEMORY_GET_WIN32_HANDLE_INFO_KHR };
    HANDLE handle = nullptr;

    info.memory = memory;
    info.handleType = gMemoryHandleType;
    return vkGetMemoryWin32HandleKHR(device, &info, &handle) == VK_SUCCESS ? Renderer_Handle_Value(handle) : UINT64_MAX;
#else
    VkMemoryGetFdInfoKHR info = { VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR };
    int fd = -1;

    info.memory = memory;
    info.handleType = gMemoryHandleType;
    return vkGetMemoryFdKHR(device, &info, &fd) == VK_SUCCESS ? Renderer_Handle_Value(fd) : UINT64_MAX;
#endif
}

uint64_t Export_Semaphore(VkDevice device, VkSemaphore semaphore) {
#if defined(_WIN32)
    VkSemaphoreGetWin32HandleInfoKHR info = { VK_STRUCTURE_TYPE_SEMAPHORE_GET_WIN32_HANDLE_INFO_KHR };
    HANDLE handle = nullptr;

    info.semaphore = semaphore;
    info.handleType = gSemaphoreHandleType;
    return vkGetSemaphoreWin32HandleKHR(device, &info, &handle) == VK_SUCCESS ? Renderer_Handle_Value(handle) : UINT64_MAX;
#else
    VkSemaphoreGetFdInfoKHR info = { VK_STRUCTURE_TYPE_SEMAPHORE_GET_FD_INFO_KHR };
    int fd = -1;

    info.semaphore = semaphore;
    info.handleType = gSemaphoreHandleType;
    return vkGetSemaphoreFdKHR(device, &info, &fd) == VK_SUCCESS ? Renderer_Handle_Value(fd) : UINT64_MAX;
#endif
}

void Close_Handle(Exported_Handle* handle) {
    if (handle->value == UINT64_MAX) {
        return;
    }

#if defined(_WIN32)
    CloseHandle(reinterpret_cast<HANDLE>(handle->value));
#else
    close(static_cast<int>(handle->value));
#endif
    handle->value = UINT64_MAX;
}

VkSemaphore Create_Timeline(VkDevice device) {
    VkSemaphoreTypeCreateInfo type = { VK_STRUCTURE_TYPE_SEMAPHORE_TYPE_CREATE_INFO };
    VkExportSemaphoreCreateInfo export_info = { VK_STRUCTURE_TYPE_EXPORT_SEMAPHORE_CREATE_INFO };
    VkSemaphoreCreateInfo info = { VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO };
    VkSemaphore semaphore = VK_NULL_HANDLE;

    type.semaphoreType = VK_SEMAPHORE_TYPE_TIMELINE;
    export_info.pNext = &type;
    export_info.handleTypes = gSemaphoreHandleType;
    info.pNext = &export_info;
    vkCreateSemaphore(device, &info, nullptr, &semaphore);
    return semaphore;
}

struct Vulkan_Swap_Chain : ModLoader_Swap_Chain {
    plume::VulkanDevice* device;
    plume::VulkanCommandQueue* queue;
    VkFormat vkFormat;
    plume::VulkanTexture textures[gSwapChainTextures];
    VkDeviceMemory memories[gSwapChainTextures];
    Exported_Handle memoryHandles[gSwapChainTextures];
    uint64_t memorySizes[gSwapChainTextures];
    VkCommandBuffer handoffs[gSwapChainTextures];
    VkCommandPool pool;
    VkSemaphore ready;
    VkSemaphore release;
    Exported_Handle readyHandle;
    Exported_Handle releaseHandle;

    void Wait_Idle() {
        std::scoped_lock lock(*queue->queue->mutex);

        vkQueueWaitIdle(queue->queue->vk);
    }

    ~Vulkan_Swap_Chain() override {
        Wait_Idle();
        Release_Images();
        vkDestroyCommandPool(device->vk, pool, nullptr);
        vkDestroySemaphore(device->vk, ready, nullptr);
        vkDestroySemaphore(device->vk, release, nullptr);
        Close_Handle(&readyHandle);
        Close_Handle(&releaseHandle);
    }

    void Release_Images() {
        for (uint32_t index = 0; index < gSwapChainTextures; index++) {
            plume::VulkanTexture& texture = textures[index];

            if (texture.imageView != VK_NULL_HANDLE) {
                vkDestroyImageView(device->vk, texture.imageView, nullptr);
                texture.imageView = VK_NULL_HANDLE;
            }

            if (texture.vk != VK_NULL_HANDLE) {
                vkDestroyImage(device->vk, texture.vk, nullptr);
                texture.vk = VK_NULL_HANDLE;
            }

            if (memories[index] != VK_NULL_HANDLE) {
                vkFreeMemory(device->vk, memories[index], nullptr);
                memories[index] = VK_NULL_HANDLE;
            }

            Close_Handle(&memoryHandles[index]);
            state.releaseValues[index] = 0;
        }
    }

    bool Memory_Type(uint32_t type_bits, uint32_t* out_type) const {
        VkPhysicalDeviceMemoryProperties properties = {};

        vkGetPhysicalDeviceMemoryProperties(device->physicalDevice, &properties);
        for (uint32_t index = 0; index < properties.memoryTypeCount; index++) {
            if ((type_bits & (1u << index)) != 0 && (properties.memoryTypes[index].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) != 0) {
                *out_type = index;
                return true;
            }
        }
        return false;
    }

    void Record_Handoff(uint32_t index) {
        VkCommandBufferBeginInfo begin = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
        VkImageMemoryBarrier barrier = { VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER };

        begin.flags = VK_COMMAND_BUFFER_USAGE_SIMULTANEOUS_USE_BIT;
        vkBeginCommandBuffer(handoffs[index], &begin);
        barrier.image = textures[index].vk;
        barrier.subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 };
        barrier.srcAccessMask = VK_ACCESS_MEMORY_WRITE_BIT;
        barrier.oldLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
        barrier.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
        barrier.srcQueueFamilyIndex = queue->familyIndex;
        barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL;

        vkCmdPipelineBarrier(
            handoffs[index],
            VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
            VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
            0,
            0,
            nullptr,
            0,
            nullptr,
            1,
            &barrier
        );
        vkEndCommandBuffer(handoffs[index]);
    }

    bool Create_Images() {
        for (uint32_t index = 0; index < gSwapChainTextures; index++) {
            VkExternalMemoryImageCreateInfo external = { VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO };
            VkImageCreateInfo image = { VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO };
            VkMemoryRequirements requirements = {};
            VkMemoryDedicatedAllocateInfo dedicated = { VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO };
            VkExportMemoryAllocateInfo export_info = { VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO };
            VkMemoryAllocateInfo allocation = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO };
            plume::VulkanTexture& texture = textures[index];

            external.handleTypes = gMemoryHandleType;
            image.pNext = &external;
            image.imageType = VK_IMAGE_TYPE_2D;
            image.format = vkFormat;
            image.extent = { state.width, state.height, 1 };
            image.mipLevels = 1;
            image.arrayLayers = 1;
            image.samples = VK_SAMPLE_COUNT_1_BIT;
            image.tiling = VK_IMAGE_TILING_OPTIMAL;
            image.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
            image.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;

            if (vkCreateImage(device->vk, &image, nullptr, &texture.vk) != VK_SUCCESS) {
                return false;
            }

            vkGetImageMemoryRequirements(device->vk, texture.vk, &requirements);
            dedicated.image = texture.vk;
            export_info.pNext = &dedicated;
            export_info.handleTypes = gMemoryHandleType;
            allocation.pNext = &export_info;
            allocation.allocationSize = requirements.size;
            if (!Memory_Type(requirements.memoryTypeBits, &allocation.memoryTypeIndex) ||
                vkAllocateMemory(device->vk, &allocation, nullptr, &memories[index]) != VK_SUCCESS ||
                vkBindImageMemory(device->vk, texture.vk, memories[index], 0) != VK_SUCCESS) {
                return false;
            }

            memoryHandles[index].value = Export_Memory(device->vk, memories[index]);
            memorySizes[index] = requirements.size;
            if (memoryHandles[index].value == UINT64_MAX) {
                return false;
            }

            texture.device = device;
            texture.desc = plume::RenderTextureDesc::Texture2D(
                state.width, state.height, 1, state.format, plume::RenderTextureFlag::RENDER_TARGET);
            texture.textureLayout = plume::RenderTextureLayout::UNKNOWN;
            texture.fillSubresourceRange();
            texture.createImageView(vkFormat);
            if (texture.imageView == VK_NULL_HANDLE) {
                return false;
            }
            state.ids[index] = ++gSwapChainNextId;
            Record_Handoff(index);
        }

        state.next = 0;
        return true;
    }

    bool present(uint32_t textureIndex, plume::RenderCommandSemaphore** waitSemaphores, uint32_t waitSemaphoreCount) override {
        VkSemaphore waits[gMaxWaits];
        VkPipelineStageFlags stages[gMaxWaits];
        uint64_t wait_values[gMaxWaits] = {};
        uint64_t signal_value = state.readyValue + 1;
        uint32_t wait_count = waitSemaphoreCount < gMaxWaits ? waitSemaphoreCount : gMaxWaits;
        VkTimelineSemaphoreSubmitInfo timeline = { VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO };
        VkSubmitInfo submit = { VK_STRUCTURE_TYPE_SUBMIT_INFO };
        ModLoader_Shared_Frame frame = {};
        VkResult result;

        for (uint32_t index = 0; index < wait_count; index++) {
            waits[index] = static_cast<plume::VulkanCommandSemaphore*>(waitSemaphores[index])->vk;
            stages[index] = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
        }

        timeline.waitSemaphoreValueCount = wait_count;
        timeline.pWaitSemaphoreValues = wait_values;
        timeline.signalSemaphoreValueCount = 1;
        timeline.pSignalSemaphoreValues = &signal_value;
        submit.pNext = &timeline;
        submit.waitSemaphoreCount = wait_count;
        submit.pWaitSemaphores = waits;
        submit.pWaitDstStageMask = stages;

        if (textureIndex == gSwapChainImages) {
            timeline.signalSemaphoreValueCount = 0;
            std::scoped_lock lock(*queue->queue->mutex);

            return vkQueueSubmit(queue->queue->vk, 1, &submit, VK_NULL_HANDLE) == VK_SUCCESS;
        }

        submit.commandBufferCount = 1;
        submit.pCommandBuffers = &handoffs[textureIndex];
        submit.signalSemaphoreCount = 1;
        submit.pSignalSemaphores = &ready;
        {
            std::scoped_lock lock(*queue->queue->mutex);

            result = vkQueueSubmit(queue->queue->vk, 1, &submit, VK_NULL_HANDLE);
        }

        if (result != VK_SUCCESS) {
            return false;
        }

        state.readyValue = signal_value;
        frame.memory_handle = memoryHandles[textureIndex].value;
        frame.memory_size = memorySizes[textureIndex];
        frame.handle_type = MODLOADER_RENDERER_HANDLE_VULKAN;
        frame.ready_handle = readyHandle.value;
        frame.release_handle = releaseHandle.value;
        Swap_Chain_Present(&state, textureIndex, &frame);

        return true;
    }

    bool acquireTexture(plume::RenderCommandSemaphore* signalSemaphore, uint32_t* textureIndex) override {
        uint64_t released = 0;
        uint32_t index;
        uint64_t wait_value;
        uint64_t signal_value = 0;
        VkPipelineStageFlags stage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
        VkTimelineSemaphoreSubmitInfo timeline = { VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO };
        VkSubmitInfo submit = { VK_STRUCTURE_TYPE_SUBMIT_INFO };
        VkSemaphore signal = static_cast<plume::VulkanCommandSemaphore*>(signalSemaphore)->vk;
        VkResult result;

        vkGetSemaphoreCounterValue(device->vk, release, &released);
        index = Swap_Chain_Free_Image(state.releaseValues, released, state.next);
        wait_value = index < gSwapChainImages ? state.releaseValues[index] : 0;
        timeline.waitSemaphoreValueCount = wait_value != 0 ? 1 : 0;
        timeline.pWaitSemaphoreValues = &wait_value;
        timeline.signalSemaphoreValueCount = 1;
        timeline.pSignalSemaphoreValues = &signal_value;
        submit.pNext = &timeline;
        submit.waitSemaphoreCount = wait_value != 0 ? 1 : 0;
        submit.pWaitSemaphores = &release;
        submit.pWaitDstStageMask = &stage;
        submit.signalSemaphoreCount = 1;
        submit.pSignalSemaphores = &signal;
        {
            std::scoped_lock lock(*queue->queue->mutex);
            result = vkQueueSubmit(queue->queue->vk, 1, &submit, VK_NULL_HANDLE);
        }

        if (result != VK_SUCCESS) {
            return false;
        }

        textures[index].textureLayout = plume::RenderTextureLayout::UNKNOWN;
        textures[index].barrierStages = plume::RenderBarrierStage::NONE;
        state.next = index < gSwapChainImages ? (index + 1) % gSwapChainImages : state.next;
        *textureIndex = index;
        return true;
    }

    bool resize() override {
        Wait_Idle();
        Release_Images();
        Swap_Chain_Size(state.config, &state.width, &state.height);
        return Create_Images();
    }

    plume::RenderTexture* getTexture(uint32_t textureIndex) override {
        return &textures[textureIndex];
    }

    plume::RenderTextureLayout getPresentLayout() const override {
        return plume::RenderTextureLayout::SHADER_READ;
    }

};
} // namespace

std::unique_ptr<plume::RenderSwapChain> Vulkan_Swap_Chain_Create(
    plume::RenderDevice* device,
    plume::RenderCommandQueue* queue,
    plume::RenderSwapChainDesc& desc,
    bool uses_hdr,
    const ModLoader_Swap_Chain_Config& config
) {
    auto* vulkan_device = static_cast<plume::VulkanDevice*>(device);
    if (!vulkan_device->timelineSemaphoreSupported) {
        return nullptr;
    }
#if defined(_WIN32)
    if (vkGetMemoryWin32HandleKHR == nullptr || vkGetSemaphoreWin32HandleKHR == nullptr) {
        return nullptr;
    }
#else
    if (vkGetMemoryFdKHR == nullptr || vkGetSemaphoreFdKHR == nullptr) {
        return nullptr;
    }
#endif
    std::unique_ptr<Vulkan_Swap_Chain> chain = std::make_unique<Vulkan_Swap_Chain>();
    VkCommandPoolCreateInfo pool = { VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO };
    VkCommandBufferAllocateInfo buffers = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO };
    VkPhysicalDeviceIDProperties id = { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES };
    VkPhysicalDeviceProperties2 properties = { VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2 };

    chain->device = static_cast<plume::VulkanDevice*>(device);
    chain->queue = static_cast<plume::VulkanCommandQueue*>(queue);
    chain->state.config = config;
    chain->state.readyValue = config.firstValue;
    chain->state.format = uses_hdr ? plume::RenderFormat::R16G16B16A16_UNORM : plume::RenderFormat::R8G8B8A8_UNORM;
    chain->vkFormat = uses_hdr ? VK_FORMAT_R16G16B16A16_UNORM : VK_FORMAT_R8G8B8A8_UNORM;
    desc.format = chain->state.format;
    properties.pNext = &id;
    vkGetPhysicalDeviceProperties2(chain->device->physicalDevice, &properties);
    memcpy(chain->state.deviceUuid, id.deviceUUID, sizeof(chain->state.deviceUuid));
    chain->ready = Create_Timeline(chain->device->vk);
    chain->release = Create_Timeline(chain->device->vk);
    if (chain->ready == VK_NULL_HANDLE || chain->release == VK_NULL_HANDLE) {
        return nullptr;
    }
    chain->readyHandle.value = Export_Semaphore(chain->device->vk, chain->ready);
    chain->releaseHandle.value = Export_Semaphore(chain->device->vk, chain->release);
    pool.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pool.queueFamilyIndex = chain->queue->familyIndex;
    if (vkCreateCommandPool(chain->device->vk, &pool, nullptr, &chain->pool) != VK_SUCCESS) {
        return nullptr;
    }
    buffers.commandPool = chain->pool;
    buffers.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    buffers.commandBufferCount = gSwapChainTextures;
    if (vkAllocateCommandBuffers(chain->device->vk, &buffers, chain->handoffs) != VK_SUCCESS) {
        return nullptr;
    }
    Swap_Chain_Size(config, &chain->state.width, &chain->state.height);

    if (memcmp(chain->state.deviceUuid, config.clientUuid, sizeof(chain->state.deviceUuid)) != 0) {
        fprintf(stderr, "ModLoader: memcmp(chain->state.deviceUuid, config.clientUuid, sizeof(chain->state.deviceUuid)) != 0\n");
        return nullptr;
    }

    if (chain->readyHandle.value == UINT64_MAX || chain->releaseHandle.value == UINT64_MAX || !chain->Create_Images()) {
        fprintf(stderr, "ModLoader: chain->readyHandle.value == UINT64_MAX || chain->releaseHandle.value == UINT64_MAX || !chain->Create_Images()\n");
        return nullptr;
    }

    return chain;
}
