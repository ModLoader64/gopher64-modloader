#include "swap_chain.h"

#include <stdio.h>
#include <string.h>

#include "plume_d3d12.h"

namespace {
ID3D12Fence* Create_Fence(ID3D12Device* device, uint64_t* out_handle) {
    ID3D12Fence* fence = nullptr;
    HANDLE handle = nullptr;

    *out_handle = 0;

    if (FAILED(device->CreateFence(0, D3D12_FENCE_FLAG_SHARED, IID_PPV_ARGS(&fence)))) {
        return nullptr;
    }

    if (SUCCEEDED(device->CreateSharedHandle(fence, nullptr, GENERIC_ALL, nullptr, &handle))) {
        *out_handle = reinterpret_cast<uint64_t>(handle);
    }
    return fence;
}

void Close_Handle(uint64_t handle) {
    if (handle != 0) {
        CloseHandle(reinterpret_cast<HANDLE>(handle));
    }
}

struct D3D12_Swap_Chain : ModLoader_Swap_Chain {
    plume::D3D12Device* device;
    plume::D3D12CommandQueue* queue;
    DXGI_FORMAT dxgiFormat;
    plume::D3D12Texture textures[gSwapChainTextures];
    uint64_t memoryHandles[gSwapChainTextures];
    uint64_t memorySizes[gSwapChainTextures];
    ID3D12Fence* ready;
    ID3D12Fence* release;
    uint64_t readyHandle;
    uint64_t releaseHandle;

    ~D3D12_Swap_Chain() override {
        Wait_Idle();
        Release_Images();
        if (ready != nullptr) {
            ready->Release();
        }
        if (release != nullptr) {
            release->Release();
        }
        Close_Handle(readyHandle);
        Close_Handle(releaseHandle);
    }

    void Wait_Idle() {
        ID3D12Fence* fence = nullptr;
        HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);

        if (SUCCEEDED(device->d3d->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)))) {
            queue->d3d->Signal(fence, 1);
            fence->SetEventOnCompletion(1, event);
            WaitForSingleObject(event, INFINITE);
            fence->Release();
        }

        CloseHandle(event);
    }

    void Release_Images() {
        for (uint32_t index = 0; index < gSwapChainTextures; index++) {
            if (textures[index].d3d != nullptr) {
                textures[index].d3d->Release();
                textures[index].d3d = nullptr;
            }
            Close_Handle(memoryHandles[index]);
            memoryHandles[index] = 0;
            state.releaseValues[index] = 0;
        }
    }

    bool Create_Images() {
        for (uint32_t index = 0; index < gSwapChainTextures; index++) {
            D3D12_HEAP_PROPERTIES heap = {};
            D3D12_RESOURCE_DESC resource = {};
            D3D12_RESOURCE_ALLOCATION_INFO allocation;
            HANDLE handle = nullptr;
            plume::D3D12Texture& texture = textures[index];

            heap.Type = D3D12_HEAP_TYPE_DEFAULT;
            resource.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
            resource.Width = state.width;
            resource.Height = state.height;
            resource.DepthOrArraySize = 1;
            resource.MipLevels = 1;
            resource.Format = dxgiFormat;
            resource.SampleDesc.Count = 1;
            resource.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
            resource.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
            if (FAILED(device->d3d->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &resource, D3D12_RESOURCE_STATE_COMMON, nullptr, IID_PPV_ARGS(&texture.d3d))) ||
                FAILED(device->d3d->CreateSharedHandle(texture.d3d, nullptr, GENERIC_ALL, nullptr, &handle))) {
                return false;
            }
            allocation = device->d3d->GetResourceAllocationInfo(0, 1, &resource);
            memoryHandles[index] = reinterpret_cast<uint64_t>(handle);
            memorySizes[index] = allocation.SizeInBytes;
            texture.device = device;
            texture.desc = plume::RenderTextureDesc::Texture2D(
                state.width, state.height, 1, state.format, plume::RenderTextureFlag::RENDER_TARGET);
            texture.resourceStates = D3D12_RESOURCE_STATE_PRESENT;
            texture.layout = plume::RenderTextureLayout::PRESENT;
            state.ids[index] = ++gSwapChainNextId;
        }
        state.next = 0;
        return true;
    }

    bool present(uint32_t textureIndex, plume::RenderCommandSemaphore** waitSemaphores, uint32_t waitSemaphoreCount) override {
        ModLoader_Shared_Frame frame = {};

        for (uint32_t index = 0; index < waitSemaphoreCount; index++) {
            plume::D3D12CommandSemaphore* semaphore = static_cast<plume::D3D12CommandSemaphore*>(waitSemaphores[index]);
            queue->d3d->Wait(semaphore->d3d, semaphore->semaphoreValue);
        }

        if (textureIndex == gSwapChainImages) {
            return true;
        }

        state.readyValue++;
        if (FAILED(queue->d3d->Signal(ready, state.readyValue))) {
            return false;
        }

        frame.memory_handle = memoryHandles[textureIndex];
        frame.memory_size = memorySizes[textureIndex];
        frame.handle_type = MODLOADER_RENDERER_HANDLE_D3D12;
        frame.ready_handle = readyHandle;
        frame.release_handle = releaseHandle;
        Swap_Chain_Present(&state, textureIndex, &frame);

        return true;
    }

    bool acquireTexture(plume::RenderCommandSemaphore* signalSemaphore, uint32_t* textureIndex) override {
        uint32_t index = Swap_Chain_Free_Image(state.releaseValues, release->GetCompletedValue(), state.next);

        if (index < gSwapChainImages && state.releaseValues[index] != 0 && FAILED(queue->d3d->Wait(release, state.releaseValues[index]))) {
            return false;
        }

        textures[index].resourceStates = D3D12_RESOURCE_STATE_PRESENT;
        textures[index].layout = plume::RenderTextureLayout::PRESENT;
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
};
} // namespace

std::unique_ptr<plume::RenderSwapChain> D3D12_Swap_Chain_Create(
    plume::RenderDevice* device,
    plume::RenderCommandQueue* queue,
    plume::RenderSwapChainDesc& desc,
    bool uses_hdr,
    const ModLoader_Swap_Chain_Config& config
) {
    std::unique_ptr<D3D12_Swap_Chain> chain = std::make_unique<D3D12_Swap_Chain>();
    LUID luid;

    chain->device = static_cast<plume::D3D12Device*>(device);
    chain->queue = static_cast<plume::D3D12CommandQueue*>(queue);
    chain->state.config = config;
    chain->state.readyValue = config.firstValue;
    chain->state.format = uses_hdr ? plume::RenderFormat::R16G16B16A16_UNORM : plume::RenderFormat::R8G8B8A8_UNORM;
    chain->dxgiFormat = uses_hdr ? DXGI_FORMAT_R16G16B16A16_UNORM : DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.format = chain->state.format;
    luid = chain->device->d3d->GetAdapterLuid();
    memset(chain->state.deviceUuid, 0, sizeof(chain->state.deviceUuid));
    memcpy(chain->state.deviceUuid, &luid, sizeof(luid));
    chain->ready = Create_Fence(chain->device->d3d, &chain->readyHandle);
    chain->release = Create_Fence(chain->device->d3d, &chain->releaseHandle);
    Swap_Chain_Size(config, &chain->state.width, &chain->state.height);

    if (!config.clientLuidValid || memcmp(&luid, config.clientLuid, sizeof(config.clientLuid)) != 0) {
        fprintf(stderr, "ModLoader: !config.clientLuidValid || memcmp(&luid, config.clientLuid, sizeof(config.clientLuid)) != 0\n");
        return nullptr;
    }

    if (chain->readyHandle == 0 || chain->releaseHandle == 0 || !chain->Create_Images()) {
        fprintf(stderr, "ModLoader: chain->readyHandle == 0 || chain->releaseHandle == 0 || !chain->Create_Images()\n");
        return nullptr;
    }
    return chain;
}
