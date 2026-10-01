#pragma once
#include "../modloader_renderer.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*rdp_frame_callback)(void* user, const uint8_t* pixels, uint32_t width, uint32_t height, uint32_t format);

void rdp_set_frame_callback(rdp_frame_callback callback, void* user);
void rdp_set_shared_frames(const uint8_t* gpu_uuid, modloader_shared_frame_callback callback, void* user, uint64_t first_value);

#ifdef __cplusplus
}
#endif
