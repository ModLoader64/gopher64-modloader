#include "presentation.hpp"
#include "../retroachievements/retroachievements.h"
#include "spirv.hpp"
#include "spirv_crt.hpp"
#include "wsi.hpp"
#include "wsi_platform.hpp"
#include <SDL3/SDL_vulkan.h>
#include <SDL3_ttf/SDL_ttf.h>
#include <cmath>
#include <format>
#include <map>

using namespace Vulkan;

enum user_event_codes {
  USER_EVENT_SAVE_STATE = 1,
  USER_EVENT_LOAD_STATE = 2,
  USER_EVENT_EXIT_GAME = 3,
  USER_EVENT_FAST_FORWARD = 4,
  USER_EVENT_LOAD_REWIND = 5,
};

typedef struct {
  uint32_t fps;
  uint32_t vis;
} FPS_DATA;

typedef struct {
  std::string message;
  uint64_t milliseconds;
  uint64_t expiresAt;
  Vulkan::ImageHandle image;
} Message;

static RDP_Presentation *presentation;
static GFX_INFO *gfx_info;
static CALL_BACK *callback;
static SDL_Window *window;
static SDL_WSIPlatform *wsi_platform;
static WSI *wsi;
static bool frame_active;
static const uint32_t *fragment_spirv;
static size_t fragment_size;

static TTF_Font *message_font;
static std::queue<Message> messages;

static float message_font_size = 25.0;
static float achievement_challenge_indicator_font_size = 12.0;

static TTF_Font *achievement_challenge_indicator_font;
static std::vector<const char *> achievement_challenge_indicators;
static Vulkan::ImageHandle achievement_challenge_indicator_image;
static Vulkan::ImageHandle achievement_progress_indicator_image;
static std::map<uint32_t, std::string> leaderboard_trackers;
static bool display_challenge_indicator;

static bool display_fps;
static Vulkan::ImageHandle fps_image;

static std::queue<JoystickEvent> joystick_events;

typedef struct {
  float SourceSize[4];
  float OutputSize[4];
} Push;

static void add_joystick_event(void *userdata) {
  JoystickEvent *joystick_event = (JoystickEvent *)userdata;
  joystick_events.push(*joystick_event);
  delete joystick_event;
}

bool sdl_event_filter(void *userdata, SDL_Event *event) {
  if (event->type == SDL_EVENT_WINDOW_CLOSE_REQUESTED) {
    callback->paused = false;
    callback->emu_running = false;
  } else if (event->type == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED &&
             callback->emu_running) {
    wsi_platform->do_resize();

    if (message_font) {
      TTF_SetFontSize(message_font,
                      message_font_size * SDL_GetWindowDisplayScale(window));
    }
    if (achievement_challenge_indicator_font) {
      TTF_SetFontSize(achievement_challenge_indicator_font,
                      achievement_challenge_indicator_font_size *
                          SDL_GetWindowDisplayScale(window));
    }
  } else if (event->type == SDL_EVENT_WINDOW_MINIMIZED) {
    callback->paused = true;
  } else if (event->type == SDL_EVENT_WINDOW_RESTORED) {
    callback->paused = false;
  } else if (event->type == SDL_EVENT_KEY_DOWN && !event->key.repeat) {
    SDL_Event user_event;
    switch (event->key.scancode) {
    case SDL_SCANCODE_RETURN:
      if (event->key.mod & SDL_KMOD_ALT) {
        gfx_info->fullscreen = !gfx_info->fullscreen;
        SDL_SetWindowFullscreen(window, gfx_info->fullscreen);
      }
      break;
    case SDL_SCANCODE_F:
      if (event->key.mod & SDL_KMOD_ALT) {
        SDL_zero(user_event);
        user_event.type = SDL_EVENT_USER;
        user_event.user.code = USER_EVENT_FAST_FORWARD;
        SDL_PushEvent(&user_event);
      }
      break;
    case SDL_SCANCODE_P:
      if (event->key.mod & SDL_KMOD_ALT) {
        callback->paused = !callback->paused;
      }
      break;
    case SDL_SCANCODE_AC_BACK:
    case SDL_SCANCODE_ESCAPE:
      if (gfx_info->fullscreen) {
        SDL_zero(user_event);
        user_event.type = SDL_EVENT_USER;
        user_event.user.code = USER_EVENT_EXIT_GAME;
        SDL_PushEvent(&user_event);
      }
      break;
    case SDL_SCANCODE_F1:
      display_fps = !display_fps;
      break;
    case SDL_SCANCODE_F4:
      presentation->cropLetterbox = !presentation->cropLetterbox;
      break;
    case SDL_SCANCODE_F5:
      SDL_zero(user_event);
      user_event.type = SDL_EVENT_USER;
      user_event.user.code = USER_EVENT_SAVE_STATE;
      SDL_PushEvent(&user_event);
      break;
    case SDL_SCANCODE_F6:
      SDL_zero(user_event);
      user_event.type = SDL_EVENT_USER;
      user_event.user.code = USER_EVENT_LOAD_REWIND;
      SDL_PushEvent(&user_event);
      break;
    case SDL_SCANCODE_F7:
      SDL_zero(user_event);
      user_event.type = SDL_EVENT_USER;
      user_event.user.code = USER_EVENT_LOAD_STATE;
      SDL_PushEvent(&user_event);
      break;
    case SDL_SCANCODE_F8:
      if (messages.empty())
        SDL_RunOnMainThread(ra_display_inprogress_achievements, nullptr, false);
      break;
    case SDL_SCANCODE_F9:
      display_challenge_indicator = !display_challenge_indicator;
      rdp_onscreen_message(
          std::format("Challenge indicators: {}",
                      display_challenge_indicator ? "ON" : "OFF")
              .c_str(),
          MESSAGE_VERY_SHORT);
      break;
    case SDL_SCANCODE_F12:
      callback->reset_game = true;
      break;
    case SDL_SCANCODE_LEFTBRACKET:
      if (event->key.mod & SDL_KMOD_ALT) {
        callback->decrease_input_delay = true;
      } else {
        callback->lower_volume = true;
      }
      break;
    case SDL_SCANCODE_RIGHTBRACKET:
      if (event->key.mod & SDL_KMOD_ALT) {
        callback->increase_input_delay = true;
      } else {
        callback->raise_volume = true;
      }
      break;
    case SDL_SCANCODE_SLASH:
      callback->frame_advance = true;
      break;
    case SDL_SCANCODE_0:
    case SDL_SCANCODE_1:
    case SDL_SCANCODE_2:
    case SDL_SCANCODE_3:
    case SDL_SCANCODE_4:
    case SDL_SCANCODE_5:
    case SDL_SCANCODE_6:
    case SDL_SCANCODE_7:
    case SDL_SCANCODE_8:
    case SDL_SCANCODE_9:
      if (event->key.mod & SDL_KMOD_ALT) {
        if (event->key.scancode == SDL_SCANCODE_0)
          callback->save_state_slot = 0;
        else
          callback->save_state_slot = event->key.scancode - SDL_SCANCODE_1 + 1;
      }
      break;
    default:
      break;
    }
  } else if (event->type == SDL_EVENT_USER) {
    switch (event->user.code) {
    case USER_EVENT_SAVE_STATE:
      callback->save_state = true;
      break;
    case USER_EVENT_LOAD_REWIND:
      callback->load_rewind = true;
      break;
    case USER_EVENT_LOAD_STATE:
      callback->load_state = true;
      break;
    case USER_EVENT_EXIT_GAME:
      callback->emu_running = false;
      break;
    case USER_EVENT_FAST_FORWARD:
      callback->enable_speedlimiter = !callback->enable_speedlimiter;
      break;
    default:
      break;
    }
  } else if (event->type == SDL_EVENT_JOYSTICK_ADDED) {
    JoystickEvent *joystick_event = new JoystickEvent;
    joystick_event->joystick_id = event->jdevice.which;
    joystick_event->connected = true;
    SDL_RunOnMainThread(add_joystick_event, joystick_event, false);
  } else if (event->type == SDL_EVENT_JOYSTICK_REMOVED) {
    JoystickEvent *joystick_event = new JoystickEvent;
    joystick_event->joystick_id = event->jdevice.which;
    joystick_event->connected = false;
    SDL_RunOnMainThread(add_joystick_event, joystick_event, false);
  } else if (event->type == SDL_EVENT_WILL_ENTER_BACKGROUND && wsi) {
    RDP_Presentation_End(presentation);
    wsi->deinit_surface_and_swapchain();
  } else if (event->type == SDL_EVENT_RENDER_DEVICE_RESET && wsi) {
    wsi->init_surface_swapchain();
    RDP_Presentation_Begin(presentation);
  }

  return 0;
}

JoystickEvent get_joystick_event() {
  if (joystick_events.empty())
    return JoystickEvent{0, false};
  JoystickEvent joystick_event = joystick_events.front();
  joystick_events.pop();
  return joystick_event;
}

static ImageHandle create_message_image(Vulkan::Device &device, int width,
                                        TTF_Font *font, const char *message) {
  if (strstr(message, "\n"))
    width = 0;

  SDL_Color fg = {255, 255, 255, 255};
  SDL_Color bg = {0, 0, 0, 0};
  SDL_Surface *surface =
      TTF_RenderText_LCD_Wrapped(font, message, 0, fg, bg, width);
  ImageCreateInfo info = ImageCreateInfo::immutable_2d_image(
      surface->w, surface->h, VK_FORMAT_B8G8R8A8_UNORM, false);
  ImageInitialData initial_data = {};
  initial_data.data = surface->pixels;
  initial_data.row_length = surface->pitch / 4;
  initial_data.image_height = surface->h;

  ImageHandle handle = device.create_image(info, &initial_data);
  SDL_DestroySurface(surface);
  return handle;
}


bool RDP_Presentation_Open(RDP_Presentation *state, void *_window,
                           GFX_INFO *graphics, CALL_BACK *callbacks,
                           const void *font, size_t font_size) {
  presentation = state;
  gfx_info = graphics;
  callback = callbacks;
  frame_active = false;
  window = (SDL_Window *)_window;
  SDL_SyncWindow(window);
  bool result = SDL_AddEventWatch(sdl_event_filter, nullptr);
  if (!result) {
    LOGE("Could not add event watch.\n");
    return false;
  }

  if (gfx_info->crt) {
    fragment_spirv = crt_fragment_spirv;
    fragment_size = sizeof(crt_fragment_spirv);
  } else {
    fragment_spirv = plain_fragment_spirv;
    fragment_size = sizeof(plain_fragment_spirv);
  }

  wsi = new WSI;
  state->backend = wsi;
  wsi_platform = new SDL_WSIPlatform;
  wsi_platform->set_window(window);
  wsi->set_platform(wsi_platform);
  if (gfx_info->vsync) {
    // VK_PRESENT_MODE_MAILBOX_KHR, fallback to VK_PRESENT_MODE_FIFO_KHR
    wsi->set_present_mode(PresentMode::UnlockedNoTearing);
  } else {
    // VK_PRESENT_MODE_MAILBOX_KHR, fallback to VK_PRESENT_MODE_IMMEDIATE_KHR
    wsi->set_present_mode(PresentMode::UnlockedMaybeTear);
  }
  wsi->set_backbuffer_srgb(false);
  Context::SystemHandles handles = {};
  if (!::Vulkan::Context::init_loader(
          (PFN_vkGetInstanceProcAddr)SDL_Vulkan_GetVkGetInstanceProcAddr())) {
    return false;
  }
  if (!wsi->init_simple(1, handles)) {
    return false;
  }

  message_font =
      TTF_OpenFontIO(SDL_IOFromConstMem(font, font_size), true,
                     message_font_size * SDL_GetWindowDisplayScale(window));
  achievement_challenge_indicator_font =
      TTF_OpenFontIO(SDL_IOFromConstMem(font, font_size), true,
                     achievement_challenge_indicator_font_size *
                         SDL_GetWindowDisplayScale(window));
  if (!message_font || !achievement_challenge_indicator_font) {
    return false;
  }

  messages = std::queue<Message>();

  display_challenge_indicator = true;
  achievement_challenge_indicators.clear();
  leaderboard_trackers.clear();
  achievement_challenge_indicator_image = Vulkan::ImageHandle();
  achievement_progress_indicator_image = Vulkan::ImageHandle();
  fps_image = Vulkan::ImageHandle();
  display_fps = false;
  state->device = &wsi->get_device();
  return true;
}

bool RDP_Presentation_Begin(RDP_Presentation *state) {
  auto *backend = static_cast<WSI *>(state->backend);
  frame_active = backend->begin_frame();
  return frame_active;
}

void RDP_Presentation_End(RDP_Presentation *state) {
  if (frame_active) {
    static_cast<WSI *>(state->backend)->end_frame();
    frame_active = false;
  }
}

void RDP_Presentation_Close(RDP_Presentation *state) {
  SDL_RemoveEventWatch(sdl_event_filter, nullptr);
  display_fps = false;

  messages = std::queue<Message>();
  achievement_challenge_indicator_image = Vulkan::ImageHandle();
  achievement_progress_indicator_image = Vulkan::ImageHandle();
  fps_image = Vulkan::ImageHandle();

  if (message_font) {
    TTF_CloseFont(message_font);
    message_font = nullptr;
  }
  if (achievement_challenge_indicator_font) {
    TTF_CloseFont(achievement_challenge_indicator_font);
    achievement_challenge_indicator_font = nullptr;
  }
  if (wsi) {
    delete wsi;
    wsi = nullptr;
  }
  if (wsi_platform) {
    delete wsi_platform;
    wsi_platform = nullptr;
  }

  state->backend = nullptr;
  state->device = nullptr;
  presentation = nullptr;
  gfx_info = nullptr;
  callback = nullptr;
}

static void calculate_viewport(float *x, float *y, float *width, float *height,
                               uint32_t display_height) {
  uint32_t display_width =
      gfx_info->widescreen ? display_height * 16 / 9 : display_height * 4 / 3;

  int w, h;
  SDL_GetWindowSizeInPixels(window, &w, &h);

  if (gfx_info->integer_scaling) {
    // Integer scaling path
    int scale_x = w / display_width;
    int scale_y = h / display_height;
    int scale = (scale_x < scale_y) ? scale_x : scale_y;
    if (scale < 1)
      scale = 1;

    // Calculate scaled dimensions
    int scaled_width = display_width * scale;
    int scaled_height = display_height * scale;

    *width = scaled_width;
    *height = scaled_height;

    // Center the viewport
    int integer_x = (w - *width) / 2.0f;
    int integer_y = (h - *height) / 2.0f;

    *x = integer_x;
    *y = integer_y;
  } else {
    // Regular scaling path - maintain aspect ratio
    float scale_x = w / (float)display_width;
    float scale_y = h / (float)display_height;
    float scale = (scale_x < scale_y) ? scale_x : scale_y;

    *width = display_width * scale;
    *height = display_height * scale;

    // Center the viewport
    *x = (w - *width) / 2.0f;
    *y = (h - *height) / 2.0f;
  }
}

static void draw_indicator(CommandBufferHandle cmd,
                           Vulkan::ImageHandle indicator_image, VkViewport vp) {
  cmd->set_texture(0, 0, indicator_image->get_view(),
                   Vulkan::StockSampler::NearestClamp);
  vp.x = vp.x + vp.width - indicator_image->get_width();
  vp.y = vp.y + vp.height - indicator_image->get_height();
  vp.height = indicator_image->get_height();
  vp.width = indicator_image->get_width();
  cmd->set_viewport(vp);

  cmd->draw(3);
}

static void draw_fps(CommandBufferHandle cmd, VkViewport vp) {
  cmd->set_texture(0, 0, fps_image->get_view(),
                   Vulkan::StockSampler::NearestClamp);
  vp.y = vp.y + vp.height - fps_image->get_height();
  vp.height = fps_image->get_height();
  vp.width = fps_image->get_width();
  cmd->set_viewport(vp);

  cmd->draw(3);
}

void RDP_Presentation_Render(RDP_Presentation *state,
                             RDP::CommandProcessor *processor,
                             const RDP::ScanoutOptions *options) {
  if (!frame_active) {
    return;
  }
  if (!messages.empty() && messages.front().image &&
      SDL_GetTicks() >= messages.front().expiresAt) {
    messages.pop();
  }
  Vulkan::Device &device = *state->device;
  Vulkan::ImageHandle image = processor->scanout(*options);

  Vulkan::ResourceLayout vertex_layout = {};
  Vulkan::ResourceLayout fragment_layout = {};
  fragment_layout.output_mask = 1 << 0;
  fragment_layout.sets[0].sampled_image_mask = 1 << 0;
  if (gfx_info->crt)
    fragment_layout.push_constant_size = sizeof(Push);

  // This request is cached.
  auto *program =
      device.request_program(vertex_spirv, sizeof(vertex_spirv), fragment_spirv,
                             fragment_size, &vertex_layout, &fragment_layout);

  // Blit image on screen.
  auto cmd = device.request_command_buffer();
  {
    auto rp = device.get_swapchain_render_pass(
        Vulkan::SwapchainRenderPass::ColorOnly);
    cmd->begin_render_pass(rp);

    cmd->set_program(program);

    // Basic default render state.
    cmd->set_opaque_state();
    cmd->set_depth_test(false, false);
    cmd->set_cull_mode(VK_CULL_MODE_NONE);

    VkViewport vp = cmd->get_viewport();
    // If we don't have an image, we just get a cleared screen in the render
    // pass.
    if (image) {
      calculate_viewport(&vp.x, &vp.y, &vp.width, &vp.height,
                         image->get_height() / gfx_info->upscale);

      if (gfx_info->crt) {
        // Set shader parameters
        Push push = {
            {float(image->get_width()), float(image->get_height()),
             1.0f / float(image->get_width()),
             1.0f / float(image->get_height())},
            {vp.width, vp.height, 1.0f / vp.width, 1.0f / vp.height},
        };
        cmd->push_constants(&push, 0, sizeof(push));
      }

      cmd->set_texture(0, 0, image->get_view(),
                       Vulkan::StockSampler::NearestClamp);
      cmd->set_viewport(vp);
      // The vertices are constants in the shader.
      // Draws fullscreen quad using oversized triangle.
      cmd->draw(3);
    }
    if (!messages.empty()) {
      Message *message = &messages.front();
      if (!message->image) {
        message->image = create_message_image(device, vp.width, message_font,
                                              message->message.c_str());
        message->expiresAt = SDL_GetTicks() + message->milliseconds;
      }
      cmd->set_texture(0, 0, message->image->get_view(),
                       Vulkan::StockSampler::NearestClamp);
      vp.x = floor(vp.x + (vp.width - message->image->get_width()) / 2);
      vp.y = vp.y + vp.height - message->image->get_height();
      vp.height = message->image->get_height();
      vp.width = message->image->get_width();
      cmd->set_viewport(vp);

      cmd->draw(3);
    } else if (achievement_progress_indicator_image) {
      draw_indicator(cmd, achievement_progress_indicator_image, vp);
    } else if (achievement_challenge_indicator_image &&
               display_challenge_indicator) {
      draw_indicator(cmd, achievement_challenge_indicator_image, vp);
    }

    if (messages.empty() && display_fps && fps_image) {
      draw_fps(cmd, vp);
    }

    cmd->end_render_pass();
  }
  device.submit(cmd);
}

void RDP_Presentation_Update(RDP_Presentation *state) {
  if (SDL_GetWindowFlags(window) & SDL_WINDOW_MINIMIZED) {
    return;
  }
  if (frame_active) {
    frame_active = false;
    if (!wsi->end_frame()) {
      LOGE("End frame failed\n");
      SDL_PumpEvents(); // For Android to trigger pause event
    }
  }
  RDP_Presentation_Begin(state);
}

static void push_onscreen_message(void *data) {
  Message *message = (Message *)data;
  messages.push(*message);
  delete message;
}

void rdp_onscreen_message(const char *message, MESSAGE_LENGTH milliseconds) {
  Message *data = new Message;
  data->message = message;
  data->milliseconds = static_cast<uint64_t>(milliseconds);
  data->expiresAt = 0;
  data->image = Vulkan::ImageHandle();
  SDL_RunOnMainThread(push_onscreen_message, data, false);
}

static void rdp_set_fps_callback(void *userdata) {
  FPS_DATA *data = (FPS_DATA *)userdata;
  if (!wsi) {
    delete data;
    return;
  }
  fps_image = create_message_image(
      wsi->get_device(), 0, achievement_challenge_indicator_font,
      std::format("FPS: {} VI/S: {}", data->fps, data->vis).c_str());
  delete data;
}

void rdp_set_fps(uint32_t fps, uint32_t vis) {
  if (display_fps) {
    FPS_DATA *data = new FPS_DATA;
    data->fps = fps;
    data->vis = vis;
    SDL_RunOnMainThread(rdp_set_fps_callback, data, false);
  }
}

static void update_challenge_indicator() {
  std::string message;
  for (const auto &leaderboard_tracker : leaderboard_trackers) {
    message += leaderboard_tracker.second;
    message += '\n';
  }

  if (!leaderboard_trackers.empty() &&
      !achievement_challenge_indicators.empty()) {
    message += "---\n";
  }

  const auto &v = achievement_challenge_indicators;
  for (size_t i = 0; i < std::min<size_t>(v.size(), 5); ++i) {
    message += v[i];
    message += '\n';
  }

  if (message.empty()) {
    achievement_challenge_indicator_image = Vulkan::ImageHandle();
  } else {
    achievement_challenge_indicator_image = create_message_image(
        wsi->get_device(), 0, achievement_challenge_indicator_font,
        message.c_str());
  }
}

void achievement_challenge_indicator_add(const char *achievement_title) {
  achievement_challenge_indicators.push_back(achievement_title);
  update_challenge_indicator();
}

void achievement_challenge_indicator_remove(const char *achievement_title) {
  std::erase(achievement_challenge_indicators, achievement_title);
  update_challenge_indicator();
}

void achievement_progress_add(const char *achievement_title,
                              const char *progress) {
  std::string message = std::format("{}: {}", achievement_title, progress);
  achievement_progress_indicator_image =
      create_message_image(wsi->get_device(), 0, message_font, message.c_str());
}

void achievement_progress_remove() {
  achievement_progress_indicator_image = Vulkan::ImageHandle();
}

void leaderboard_tracker_add(uint32_t id, const char *title,
                             const char *display) {
  if (title) {
    leaderboard_trackers[id] = std::format("{}: {}", title, display);
    update_challenge_indicator();
  }
}

void leaderboard_tracker_remove(uint32_t id) {
  leaderboard_trackers.erase(id);
  update_challenge_indicator();
}
