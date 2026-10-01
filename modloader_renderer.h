#pragma once

#include <stdint.h>

enum {
    MODLOADER_RENDERER_RGBA8 = 0,
    MODLOADER_RENDERER_RGBA16 = 1,
    MODLOADER_RENDERER_HANDLE_VULKAN = 0,
    MODLOADER_RENDERER_HANDLE_D3D12 = 1,
};

typedef struct ModLoader_Shared_Frame {
    uint8_t device_uuid[16];
    uint64_t memory_handle;
    uint64_t memory_size;
    uint32_t handle_type;
    uint32_t image_id;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint32_t reserved;
    uint64_t ready_handle;
    uint64_t ready_value;
    uint64_t release_handle;
    uint64_t release_value;
} ModLoader_Shared_Frame;

typedef void (*modloader_shared_frame_callback)(void* user, const ModLoader_Shared_Frame* frame);

#ifdef __cplusplus
#ifdef _WIN32
inline uint64_t Renderer_Handle_Value(void* handle) {
    return reinterpret_cast<uint64_t>(handle);
}
#else
inline uint64_t Renderer_Handle_Value(int handle) {
    return static_cast<uint64_t>(handle);
}
#endif
#endif
