#pragma once

#include "interface.hpp"
#include "rdp_device.hpp"

struct RDP_Presentation {
  Vulkan::Device *device = nullptr;
  void *backend = nullptr;
  bool cropLetterbox = false;
  bool synchronizeOnRead = false;
};

bool RDP_Presentation_Open(RDP_Presentation *state, void *window,
                           GFX_INFO *graphics, CALL_BACK *callback,
                           const void *font, size_t font_size);
bool RDP_Presentation_Begin(RDP_Presentation *state);
void RDP_Presentation_End(RDP_Presentation *state);
void RDP_Presentation_Close(RDP_Presentation *state);
void RDP_Presentation_Render(RDP_Presentation *state,
                             RDP::CommandProcessor *processor,
                             const RDP::ScanoutOptions *options);
void RDP_Presentation_Update(RDP_Presentation *state);
