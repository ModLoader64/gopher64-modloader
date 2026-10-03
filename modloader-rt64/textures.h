#pragma once

#include <stdint.h>

namespace RT64 {
struct Application;
}

int32_t Texture_Sources_Set(RT64::Application* application, const char* const* paths, uint32_t count, uint32_t flags);
void Texture_Sources_Open(RT64::Application* application);
void Texture_Sources_Close();
