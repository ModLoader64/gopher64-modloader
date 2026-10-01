#pragma once

#include "../modloader_renderer.h"

#ifdef __cplusplus
extern "C" {
#endif

enum {
    RT64_INTERRUPT_SP = 0x01, // as MI_INTR
    RT64_INTERRUPT_DP = 0x20,
    RT64_TASK_WAITING = 0x100, // waiting on a CPU write
};

typedef int32_t (*rt64_output_size_callback)(void* user, uint32_t* width, uint32_t* height);

typedef struct RT64_CORE {
    uint8_t* rdram;
    uint8_t* dmem;
    uint8_t* imem;
    uint8_t* rom_header;
    uint32_t* dpc_regs;
    uint32_t* vi_regs;
} RT64_CORE;

typedef struct RT64_CONFIG {
    const char* data_path;
    const char* user_config;
    const uint8_t* gpu_uuid;
    modloader_shared_frame_callback frame_callback;
    rt64_output_size_callback output_size;
    void* user;
    uint64_t first_value;
} RT64_CONFIG;

int32_t rt64_open(const RT64_CORE* core, const RT64_CONFIG* config);
void rt64_close();
void rt64_commit();
int32_t rt64_known_ucode(const uint8_t* rdram, uint32_t text, uint32_t data);
uint32_t rt64_run_task(uint32_t text, uint32_t data, uint32_t data_ptr);
uint32_t rt64_resume_task();
uint32_t rt64_waiting_address();
uint32_t rt64_process_rdp();
void rt64_update_screen();
void rt64_discard();
void rt64_set_user_config(const char* user_config);

#ifdef __cplusplus
}
#endif
