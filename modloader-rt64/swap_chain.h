#pragma once

#include <memory>
#include <string.h>

#include "modloader_rt64.h"
#include "plume_render_interface.h"

struct ModLoader_Swap_Chain_Config {
    modloader_shared_frame_callback frameCallback;
    rt64_output_size_callback outputSize;
    void* user;
    uint8_t clientUuid[16];
    uint8_t clientLuid[8];
    bool clientLuidValid;
    uint64_t firstValue;
};

constexpr uint32_t gSwapChainImages = 3;
constexpr uint32_t gSwapChainTextures = gSwapChainImages + 1;
inline uint32_t gSwapChainNextId = 0x80000000u;

struct Swap_Chain_State {
    ModLoader_Swap_Chain_Config config;
    plume::RenderFormat format;
    uint8_t deviceUuid[16];
    uint32_t width;
    uint32_t height;
    uint32_t next;
    uint32_t ids[gSwapChainTextures];
    uint64_t readyValue;
    uint64_t releaseValues[gSwapChainTextures];
};

std::unique_ptr<plume::RenderSwapChain> Vulkan_Swap_Chain_Create(
    plume::RenderDevice* device,
    plume::RenderCommandQueue* queue,
    plume::RenderSwapChainDesc& desc,
    bool uses_hdr,
    const ModLoader_Swap_Chain_Config& config
);

std::unique_ptr<plume::RenderSwapChain> D3D12_Swap_Chain_Create(
    plume::RenderDevice* device,
    plume::RenderCommandQueue* queue,
    plume::RenderSwapChainDesc& desc,
    bool uses_hdr,
    const ModLoader_Swap_Chain_Config& config
);

inline uint32_t Swap_Chain_Free_Image(const uint64_t* release_values, uint64_t released, uint32_t next) {
    for (uint32_t step = 0; step < gSwapChainImages; step++) {
        uint32_t index = (next + step) % gSwapChainImages;

        if (release_values[index] <= released) {
            return index;
        }
    }
    return gSwapChainImages;
}

void Swap_Chain_Size(const ModLoader_Swap_Chain_Config& config, uint32_t* width, uint32_t* height);

struct ModLoader_Swap_Chain : plume::RenderSwapChain {
    Swap_Chain_State state;

    bool needsResize() const override {
        uint32_t width;
        uint32_t height;

        Swap_Chain_Size(state.config, &width, &height);
        return width != state.width || height != state.height;
    }

    void wait() override {
    }

    void setVsyncEnabled(bool vsyncEnabled) override {
    }

    bool isVsyncEnabled() const override {
        return false;
    }

    uint32_t getWidth() const override {
        return state.width;
    }

    uint32_t getHeight() const override {
        return state.height;
    }

    uint32_t getTextureCount() const override {
        return gSwapChainTextures;
    }

    plume::RenderWindow getWindow() const override {
        return {};
    }

    bool isEmpty() const override {
        return state.width == 0 || state.height == 0;
    }

    uint32_t getRefreshRate() const override {
        return 0;
    }
};

inline void Swap_Chain_Present(Swap_Chain_State* state, uint32_t index, ModLoader_Shared_Frame* frame) {
    state->releaseValues[index] = state->readyValue;
    memcpy(frame->device_uuid, state->deviceUuid, sizeof(frame->device_uuid));
    frame->image_id = state->ids[index];
    frame->width = state->width;
    frame->height = state->height;
    frame->format = state->format == plume::RenderFormat::R16G16B16A16_UNORM ? MODLOADER_RENDERER_RGBA16 : MODLOADER_RENDERER_RGBA8;
    frame->ready_value = state->readyValue;
    frame->release_value = state->readyValue;
    state->config.frameCallback(state->config.user, frame);
}
