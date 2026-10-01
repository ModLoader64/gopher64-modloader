#include "interface.hpp"
#include "presentation.hpp"
#include "rdp_device.hpp"
#include <algorithm>
#include <cstring>
#include <vector>

#ifdef _WIN32
extern "C" {
__declspec(dllexport) unsigned long NvOptimusEnablement = 0x00000001;
__declspec(dllexport) int AmdPowerXpressRequestHighPerformance = 1;
}
#endif

using namespace Vulkan;

#define DP_STATUS_XBUS_DMA 0x01
#define DP_STATUS_FREEZE 0x02
#define DP_STATUS_FLUSH 0x04
#define DP_STATUS_START_GCLK 0x008
#define DP_STATUS_TMEM_BUSY 0x010
#define DP_STATUS_PIPE_BUSY 0x020
#define DP_STATUS_CMD_BUSY 0x040
#define DP_STATUS_CBUF_READY 0x080
#define DP_STATUS_DMA_BUSY 0x100
#define DP_STATUS_END_VALID 0x200
#define DP_STATUS_START_VALID 0x400

enum dpc_registers {
  DPC_START_REG,
  DPC_END_REG,
  DPC_CURRENT_REG,
  DPC_STATUS_REG,
  DPC_CLOCK_REG,
  DPC_BUFBUSY_REG,
  DPC_PIPEBUSY_REG,
  DPC_TMEM_REG,
  DPC_REGS_COUNT
};

enum vi_registers {
  VI_STATUS_REG,
  VI_ORIGIN_REG,
  VI_WIDTH_REG,
  VI_V_INTR_REG,
  VI_CURRENT_REG,
  VI_BURST_REG,
  VI_V_SYNC_REG,
  VI_H_SYNC_REG,
  VI_LEAP_REG,
  VI_H_START_REG,
  VI_V_START_REG,
  VI_V_BURST_REG,
  VI_X_SCALE_REG,
  VI_Y_SCALE_REG,
  VI_REGS_COUNT
};

typedef struct {
  uint32_t depthbuffer_address;
  uint32_t framebuffer_address;
  uint32_t framebuffer_y_offset;
  uint32_t texture_address;
  uint32_t framebuffer_pixel_size;
  uint32_t framebuffer_width;
  uint32_t texture_pixel_size;
  uint32_t texture_width;
  uint32_t framebuffer_height;
  bool depth_buffer_enabled;
} FrameBufferInfo;

typedef struct {
  uint32_t cmd_data[0x00040000 >> 2];
  int cmd_cur;
  int cmd_ptr;
  uint32_t region;
  FrameBufferInfo frame_buffer_info;
} RDP_DEVICE;

static void *g_tmem = nullptr;
static void *g_hidden_rdram = nullptr;

static RDP::CommandProcessor *processor;
static RDP_Presentation presentation;
static RDP_DEVICE rdp_device;
static CALL_BACK callback;
static GFX_INFO gfx_info;
static std::vector<bool> rdram_dirty;
static uint64_t sync_signal;

static const unsigned cmd_len_lut[64] = {
    1, 1, 1, 1, 1, 1, 1, 1, 4, 6, 12, 14, 12, 14, 20, 22, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,  1,  1,  1,  2,  2,  1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,  1,  1,  1,  1,  1,  1, 1, 1, 1,
};

void rdp_idle() {
  if (!processor) {
    return;
  }
  processor->idle();
  sync_signal = 0;
  rdram_dirty.assign(gfx_info.RDRAM_SIZE >> 3, false);
}

static void rdp_new_processor() {
  RDP::CommandProcessorFlags flags =
      RDP::COMMAND_PROCESSOR_FLAG_HOST_VISIBLE_HIDDEN_RDRAM_BIT |
      RDP::COMMAND_PROCESSOR_FLAG_HOST_VISIBLE_TMEM_BIT;

  if (gfx_info.upscale == 2) {
    flags |= RDP::COMMAND_PROCESSOR_FLAG_SUPER_SAMPLED_DITHER_BIT;
    flags |= RDP::COMMAND_PROCESSOR_FLAG_UPSCALING_2X_BIT;
  } else if (gfx_info.upscale == 4) {
    flags |= RDP::COMMAND_PROCESSOR_FLAG_SUPER_SAMPLED_DITHER_BIT;
    flags |= RDP::COMMAND_PROCESSOR_FLAG_UPSCALING_4X_BIT;
  } else if (gfx_info.upscale == 8) {
    flags |= RDP::COMMAND_PROCESSOR_FLAG_SUPER_SAMPLED_DITHER_BIT;
    flags |= RDP::COMMAND_PROCESSOR_FLAG_UPSCALING_8X_BIT;
  } else {
    gfx_info.upscale = 1;
  }

  processor = new RDP::CommandProcessor(*presentation.device, gfx_info.RDRAM, 0,
                                        gfx_info.RDRAM_SIZE,
                                        gfx_info.RDRAM_SIZE / 2, flags);
  g_tmem = processor->get_tmem();
  if (!g_tmem) {
    LOGE("Failed to get tmem\n");
  }
  g_hidden_rdram = processor->begin_read_hidden_rdram();
  if (!g_hidden_rdram) {
    LOGE("Failed to get hidden_rdram\n");
  }

  sync_signal = 0;
  rdram_dirty.assign(gfx_info.RDRAM_SIZE >> 3, false);
}

void rdp_init(void *window, GFX_INFO _gfx_info, const void *font,
              size_t font_size, uint32_t save_state_slot) {
  memset(&rdp_device, 0, sizeof(RDP_DEVICE));
  memset(&callback, 0, sizeof(CALL_BACK));
  gfx_info = _gfx_info;
  presentation.cropLetterbox = false;
  presentation.synchronizeOnRead = false;
  if (!RDP_Presentation_Open(&presentation, window, &gfx_info, &callback,
                             font, font_size)) {
    rdp_close();
    return;
  }
  rdp_new_processor();
  if (!processor->device_is_supported() || !RDP_Presentation_Begin(&presentation)) {
    rdp_close();
    return;
  }
  callback.emu_running = true;
  callback.enable_speedlimiter = true;
  callback.save_state_slot = save_state_slot;
}

void rdp_close() {
  callback.emu_running = false;
  RDP_Presentation_End(&presentation);
  g_tmem = nullptr;
  g_hidden_rdram = nullptr;
  delete processor;
  processor = nullptr;
  RDP_Presentation_Close(&presentation);
  sync_signal = 0;
  rdram_dirty.clear();
}

static RDP::ScanoutOptions build_scanout_options() {
  RDP::ScanoutOptions options = {};
  options.persist_frame_on_invalid_input = true;
  options.blend_previous_frame = !gfx_info.ssaa;
  options.upscale_deinterlacing = gfx_info.ssaa;
  if (gfx_info.ssaa) {
    switch (gfx_info.upscale) {
    case 2:
      options.downscale_steps = 1;
      break;
    case 4:
      options.downscale_steps = 2;
      break;
    case 8:
      options.downscale_steps = 3;
      break;
    default:
      options.downscale_steps = 0;
      break;
    }
  }

  if (presentation.cropLetterbox && gfx_info.widescreen) {
    options.crop_rect.enable = true;
    if (gfx_info.PAL) {
      options.crop_rect.top = 36;
      options.crop_rect.bottom = 36;
    } else {
      options.crop_rect.top = 30;
      options.crop_rect.bottom = 30;
    }
  }

  return options;
}

void rdp_set_vi_register(uint32_t reg, uint32_t value) {
  if (processor) {
    processor->set_vi_register(RDP::VIRegister(reg), value);
  }
}

void rdp_render_frame() {
  if (processor) {
    RDP::ScanoutOptions options = build_scanout_options();
    RDP_Presentation_Render(&presentation, processor, &options);
  }
}

void rdp_update_screen() {
  if (presentation.device) {
    RDP_Presentation_Update(&presentation);
  }
}

CALL_BACK rdp_check_callback() {
  CALL_BACK return_value = callback;
  callback.save_state = false;
  callback.load_rewind = false;
  callback.load_state = false;
  callback.reset_game = false;
  callback.lower_volume = false;
  callback.raise_volume = false;
  callback.decrease_input_delay = false;
  callback.increase_input_delay = false;
  callback.frame_advance = false;
  return return_value;
}

void rdp_check_framebuffers(uint32_t address, uint32_t length) {
  if (!processor || (!sync_signal && !presentation.synchronizeOnRead)) {
    return;
  }
  address >>= 3;
  length = (length + 7) >> 3;
  if (address >= rdram_dirty.size()) {
    return;
  }

  uint32_t end_addr = std::min(address + length, static_cast<uint32_t>(rdram_dirty.size()));
  auto it = std::find(rdram_dirty.begin() + address, rdram_dirty.begin() + end_addr, true);
  if (it != rdram_dirty.begin() + end_addr) {
    if (presentation.synchronizeOnRead) {
      sync_signal = processor->signal_timeline();
    }
    processor->wait_for_timeline(sync_signal);
    std::fill(rdram_dirty.begin(), rdram_dirty.end(), false);
    sync_signal = 0;
  }
}

size_t rdp_state_size() {
  if (!processor) {
    return 0;
  }
  return sizeof(RDP_DEVICE) + 0x1000 + processor->get_hidden_rdram_size();
}

void rdp_save_state(uint8_t *state) {
  if (!processor) {
    return;
  }
  memcpy(state, &rdp_device, sizeof(RDP_DEVICE));

  if (g_tmem) {
    memcpy(state + sizeof(RDP_DEVICE), g_tmem, 0x1000);
  }

  if (g_hidden_rdram) {
    memcpy(state + sizeof(RDP_DEVICE) + 0x1000, g_hidden_rdram,
           processor->get_hidden_rdram_size());
  }
}

void rdp_load_state(GFX_INFO _gfx_info, const uint8_t *state) {
  if (!processor) {
    return;
  }
  gfx_info = _gfx_info;
  memcpy(&rdp_device, state, sizeof(RDP_DEVICE));

  if (g_tmem) {
    memcpy(g_tmem, state + sizeof(RDP_DEVICE), 0x1000);
  }

  if (g_hidden_rdram) {
    memcpy(g_hidden_rdram, state + sizeof(RDP_DEVICE) + 0x1000,
           processor->get_hidden_rdram_size());
  }
}

static uint32_t Read_Big_Endian_U32(const uint8_t *bytes) {
  return (uint32_t(bytes[0]) << 24) | (uint32_t(bytes[1]) << 16) |
         (uint32_t(bytes[2]) << 8) | uint32_t(bytes[3]);
}

uint32_t pixel_size(uint32_t pixel_type, uint32_t area) {
  switch (pixel_type) {
  case 0:
    return area / 2;
  case 1:
    return area;
  case 2:
    return area * 2;
  case 3:
    return area * 4;
  default:
    LOGE("Invalid pixel size: %u\n", pixel_type);
    return 0;
  }
}

uint64_t rdp_process_commands() {
  if (!processor) {
    return 0;
  }
  uint64_t interrupt_timer = 0;
  const uint32_t DP_CURRENT = *gfx_info.DPC_CURRENT_REG & ((gfx_info.RDRAM_SIZE - 1) & ~7u);
  const uint32_t DP_END = *gfx_info.DPC_END_REG & ((gfx_info.RDRAM_SIZE - 1) & ~7u);

  int length = DP_END - DP_CURRENT;
  if (length <= 0)
    return interrupt_timer;

  length = unsigned(length) >> 3;
  if ((rdp_device.cmd_ptr + length) & ~(0x0003FFFF >> 3))
    return interrupt_timer;

  uint32_t offset = DP_CURRENT;
  if (*gfx_info.DPC_STATUS_REG & DP_STATUS_XBUS_DMA) {
    do {
      offset &= 0xFF8;
      rdp_device.cmd_data[2 * rdp_device.cmd_ptr + 0] = Read_Big_Endian_U32(gfx_info.DMEM + offset);
      rdp_device.cmd_data[2 * rdp_device.cmd_ptr + 1] = Read_Big_Endian_U32(gfx_info.DMEM + offset + 4);
      offset += sizeof(uint64_t);
      rdp_device.cmd_ptr++;
    } while (--length > 0);
  } else {
    if (DP_END > 0x7ffffff || DP_CURRENT > 0x7ffffff) {
      return interrupt_timer;
    } else {
      do {
        offset &= (gfx_info.RDRAM_SIZE - 1) & ~7u;
        rdp_device.cmd_data[2 * rdp_device.cmd_ptr + 0] =
            *reinterpret_cast<const uint32_t *>(gfx_info.RDRAM + offset);
        rdp_device.cmd_data[2 * rdp_device.cmd_ptr + 1] =
            *reinterpret_cast<const uint32_t *>(gfx_info.RDRAM + offset + 4);
        offset += sizeof(uint64_t);
        rdp_device.cmd_ptr++;
      } while (--length > 0);
    }
  }

  while (rdp_device.cmd_cur - rdp_device.cmd_ptr < 0) {
    uint32_t w1 = rdp_device.cmd_data[2 * rdp_device.cmd_cur];
    uint32_t w2 = rdp_device.cmd_data[2 * rdp_device.cmd_cur + 1];
    uint32_t command = (w1 >> 24) & 63;
    int cmd_length = cmd_len_lut[command];

    if (rdp_device.cmd_ptr - rdp_device.cmd_cur - cmd_length < 0) {
      *gfx_info.DPC_START_REG = *gfx_info.DPC_CURRENT_REG =
          *gfx_info.DPC_END_REG;
      return interrupt_timer;
    }

    if (command >= 8)
      processor->enqueue_command(cmd_length * 2,
                                 &rdp_device.cmd_data[2 * rdp_device.cmd_cur]);

    switch (RDP::Op(command)) {
    case RDP::Op::FillTriangle:
    case RDP::Op::FillZBufferTriangle:
    case RDP::Op::TextureTriangle:
    case RDP::Op::TextureZBufferTriangle:
    case RDP::Op::ShadeTriangle:
    case RDP::Op::ShadeZBufferTriangle:
    case RDP::Op::ShadeTextureTriangle:
    case RDP::Op::ShadeTextureZBufferTriangle:
    case RDP::Op::TextureRectangle:
    case RDP::Op::TextureRectangleFlip:
    case RDP::Op::FillRectangle: {
      uint32_t offset_address =
          (rdp_device.frame_buffer_info.framebuffer_address +
           pixel_size(rdp_device.frame_buffer_info.framebuffer_pixel_size,
                      rdp_device.frame_buffer_info.framebuffer_y_offset *
                          rdp_device.frame_buffer_info.framebuffer_width)) >>
          3;
      if (offset_address < rdram_dirty.size() && !rdram_dirty[offset_address]) {
        uint32_t end_addr = std::min(
            offset_address +
                ((pixel_size(
                      rdp_device.frame_buffer_info.framebuffer_pixel_size,
                      rdp_device.frame_buffer_info.framebuffer_width *
                          rdp_device.frame_buffer_info.framebuffer_height) +
                  7) >>
                 3),
            static_cast<uint32_t>(rdram_dirty.size()));
        std::fill(rdram_dirty.begin() + offset_address,
                  rdram_dirty.begin() + end_addr, true);
      }

      if (rdp_device.frame_buffer_info.depth_buffer_enabled) {
        offset_address =
            (rdp_device.frame_buffer_info.depthbuffer_address +
             pixel_size(2,
                        rdp_device.frame_buffer_info.framebuffer_y_offset *
                            rdp_device.frame_buffer_info.framebuffer_width)) >>
            3;
        if (offset_address < rdram_dirty.size() &&
            !rdram_dirty[offset_address]) {
          uint32_t end_addr = std::min(
              offset_address +
                  ((pixel_size(
                        2,
                        rdp_device.frame_buffer_info.framebuffer_width *
                            rdp_device.frame_buffer_info.framebuffer_height) +
                    7) >>
                   3),
              static_cast<uint32_t>(rdram_dirty.size()));
          std::fill(rdram_dirty.begin() + offset_address,
                    rdram_dirty.begin() + end_addr, true);
        }
      }
    } break;
    case RDP::Op::LoadTLut:
    case RDP::Op::LoadTile: {
      uint32_t upper_left_t = (w1 & 0xFFF) >> 2;
      uint32_t offset_address =
          (rdp_device.frame_buffer_info.texture_address +
           pixel_size(rdp_device.frame_buffer_info.texture_pixel_size,
                      upper_left_t *
                          rdp_device.frame_buffer_info.texture_width)) >>
          3;
      if (offset_address < rdram_dirty.size() && !rdram_dirty[offset_address]) {
        uint32_t lower_right_t = (w2 & 0xFFF) >> 2;
        uint32_t end_addr = std::min(
            offset_address +
                ((pixel_size(rdp_device.frame_buffer_info.texture_pixel_size,
                             (lower_right_t - upper_left_t) *
                                 rdp_device.frame_buffer_info.texture_width) +
                  7) >>
                 3),
            static_cast<uint32_t>(rdram_dirty.size()));
        std::fill(rdram_dirty.begin() + offset_address,
                  rdram_dirty.begin() + end_addr, true);
      }
    } break;
    case RDP::Op::LoadBlock: {
      uint32_t upper_left_s = ((w1 >> 12) & 0xFFF);
      uint32_t upper_left_t = (w1 & 0xFFF);
      uint32_t offset_address =
          (rdp_device.frame_buffer_info.texture_address +
           pixel_size(rdp_device.frame_buffer_info.texture_pixel_size,
                      upper_left_s +
                          upper_left_t *
                              rdp_device.frame_buffer_info.texture_width)) >>
          3;
      if (offset_address < rdram_dirty.size() && !rdram_dirty[offset_address]) {
        uint32_t lower_right_s = ((w2 >> 12) & 0xFFF);
        uint32_t end_addr = std::min(
            offset_address +
                ((pixel_size(rdp_device.frame_buffer_info.texture_pixel_size,
                             lower_right_s - upper_left_s) +
                  7) >>
                 3),
            static_cast<uint32_t>(rdram_dirty.size()));
        std::fill(rdram_dirty.begin() + offset_address,
                  rdram_dirty.begin() + end_addr, true);
      }
    } break;
    case RDP::Op::SetColorImage:
      rdp_device.frame_buffer_info.framebuffer_address = (w2 & (gfx_info.RDRAM_SIZE - 1));
      rdp_device.frame_buffer_info.framebuffer_pixel_size = (w1 >> 19) & 0x3;
      rdp_device.frame_buffer_info.framebuffer_width = (w1 & 0x3FF) + 1;
      break;
    case RDP::Op::SetMaskImage:
      rdp_device.frame_buffer_info.depthbuffer_address = (w2 & (gfx_info.RDRAM_SIZE - 1));
      break;
    case RDP::Op::SetTextureImage:
      rdp_device.frame_buffer_info.texture_address = (w2 & (gfx_info.RDRAM_SIZE - 1));
      rdp_device.frame_buffer_info.texture_pixel_size = (w1 >> 19) & 0x3;
      rdp_device.frame_buffer_info.texture_width = (w1 & 0x3FF) + 1;
      break;
    case RDP::Op::SetScissor: {
      uint32_t upper_left_x = ((w1 >> 12) & 0xFFF) >> 2;
      uint32_t upper_left_y = (w1 & 0xFFF) >> 2;
      uint32_t lower_right_x = ((w2 >> 12) & 0xFFF) >> 2;
      uint32_t lower_right_y = (w2 & 0xFFF) >> 2;
      if (lower_right_x > upper_left_x && lower_right_y > upper_left_y) {
        rdp_device.region =
            (lower_right_x - upper_left_x) * (lower_right_y - upper_left_y);
      } else {
        rdp_device.region = 0;
      }

      rdp_device.frame_buffer_info.framebuffer_y_offset = upper_left_y;
      rdp_device.frame_buffer_info.framebuffer_height =
          lower_right_y - upper_left_y;
    } break;
    case RDP::Op::SetOtherModes: {
      uint8_t cycle_type = (w1 >> 20) & 3;
      uint8_t depth_read_write = (w2 >> 4) & 3;
      rdp_device.frame_buffer_info.depth_buffer_enabled =
          ((cycle_type & 2) == 0) && (depth_read_write != 0);
    } break;
    case RDP::Op::SyncFull:
      sync_signal = processor->signal_timeline();

      interrupt_timer = rdp_device.region / 2;
      if (interrupt_timer == 0)
        interrupt_timer = 5000;
      break;
    default:
      break;
    }

    rdp_device.cmd_cur += cmd_length;
  }

  rdp_device.cmd_ptr = 0;
  rdp_device.cmd_cur = 0;
  *gfx_info.DPC_CURRENT_REG = *gfx_info.DPC_END_REG;

  return interrupt_timer;
}
