#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]
include!(concat!(env!("OUT_DIR"), "/parallel_bindings.rs"));
use crate::{device, modloader, ui};
use modloader_abi::{MODLOADER_FRAME_CPU, MODLOADER_FRAME_GPU_SHARED, MODLOADER_PIXELS_RGBA16};
use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const INTERRUPT_DP: u32 = RT64_INTERRUPT_DP as u32;
pub const TASK_WAITING: u32 = RT64_TASK_WAITING as u32;
const RENDERER_PARALLEL: u32 = 0;
const RENDERER_UNDECIDED: u32 = 1;
const RENDERER_RT64: u32 = 2;
const RENDERER_CLOSED: u32 = 3;
const UNDECIDED_SCREENS: u32 = 600;
const RT64_DP_CYCLES: u64 = 4000;
const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
const VK_FORMAT_R16G16B16A16_UNORM: u32 = 91;

static RENDERER: AtomicU32 = AtomicU32::new(RENDERER_CLOSED);
static UNDECIDED_COUNT: AtomicU32 = AtomicU32::new(0);
static FRAME_VALUE: AtomicU64 = AtomicU64::new(0);

include!(concat!(env!("OUT_DIR"), "/modloader_renderer_bindings.rs"));

pub struct FrameSink {
    callbacks: modloader::HostCallbacks,
    upscale: AtomicU32,
    widescreen: AtomicBool,
}

fn frame(sink: &FrameSink, width: u32, height: u32, format: u32) -> modloader::Frame {
    let upscale = sink.upscale.load(Ordering::Relaxed).max(1);
    let (numerator, denominator) = if sink.widescreen.load(Ordering::Relaxed) {
        (16, 9)
    } else {
        (4, 3)
    };
    modloader::Frame {
        kind: MODLOADER_FRAME_CPU as u32,
        width,
        height,
        displayAspectNumerator: numerator,
        displayAspectDenominator: denominator,
        sourceWidth: width / upscale,
        sourceHeight: height / upscale,
        pitch: width
            * if format == MODLOADER_PIXELS_RGBA16 as u32 {
                8
            } else {
                4
            },
        format,
        ..Default::default()
    }
}

unsafe extern "C" fn present_frame(
    user: *mut c_void,
    rgba: *const u8,
    width: u32,
    height: u32,
    format: u32,
) {
    let sink = unsafe { &*(user as *const FrameSink) };
    if let Some(present) = sink.callbacks.presentFrame {
        let frame = modloader::Frame {
            pixels: rgba,
            ..frame(sink, width, height, format)
        };
        unsafe { present(sink.callbacks.host, &frame) };
    }
}

unsafe fn present_shared(user: *mut c_void, shared: *const ModLoader_Shared_Frame, shaped: bool) {
    let sink = unsafe { &*(user as *const FrameSink) };
    let shared = unsafe { &*shared };
    FRAME_VALUE.fetch_max(shared.release_value, Ordering::Relaxed);
    if let Some(present) = sink.callbacks.presentFrame {
        let mut frame = frame(sink, shared.width, shared.height, shared.format);
        frame.kind = MODLOADER_FRAME_GPU_SHARED as u32;
        frame.gpu = modloader::GpuImage {
            deviceUuid: shared.device_uuid,
            memoryHandle: shared.memory_handle,
            memorySize: shared.memory_size,
            handleType: if shared.handle_type == MODLOADER_RENDERER_HANDLE_D3D12 as u32 {
                modloader::HANDLE_D3D12
            } else {
                modloader::HANDLE_OPAQUE
            },
            imageId: shared.image_id,
            vkFormat: if shared.format == MODLOADER_RENDERER_RGBA16 as u32 {
                VK_FORMAT_R16G16B16A16_UNORM
            } else {
                VK_FORMAT_R8G8B8A8_UNORM
            },
            readySemaphoreHandle: shared.ready_handle,
            readyValue: shared.ready_value,
            releaseSemaphoreHandle: shared.release_handle,
            releaseValue: shared.release_value,
            ..Default::default()
        };
        if shaped {
            let scale = shared.height.div_ceil(240).max(1);
            frame.displayAspectNumerator = shared.width;
            frame.displayAspectDenominator = shared.height;
            frame.sourceWidth = shared.width / scale;
            frame.sourceHeight = shared.height / scale;
        }
        unsafe { present(sink.callbacks.host, &frame) };
    }
}

unsafe extern "C" fn present_shared_frame(
    user: *mut c_void,
    shared: *const ModLoader_Shared_Frame,
) {
    unsafe { present_shared(user, shared, false) };
}

unsafe extern "C" fn present_rt64_frame(user: *mut c_void, shared: *const ModLoader_Shared_Frame) {
    unsafe { present_shared(user, shared, true) };
}

unsafe extern "C" fn rt64_output_size(user: *mut c_void, width: *mut u32, height: *mut u32) -> i32 {
    let sink = unsafe { &*(user as *const FrameSink) };
    match sink.callbacks.outputSize {
        Some(output_size) => {
            unsafe { output_size(sink.callbacks.host, width, height) };
            unsafe { (*width != 0 && *height != 0) as i32 }
        }
        None => 0,
    }
}

fn build_gfx_info(device: &mut device::Device) -> GFX_INFO {
    GFX_INFO {
        RDRAM: device.rdram.mem.as_mut_ptr(),
        DMEM: device.rsp.mem.as_mut_ptr(),
        RDRAM_SIZE: device.rdram.size,
        DPC_CURRENT_REG: &mut device.rdp.regs_dpc[device::rdp::DPC_CURRENT_REG],
        DPC_START_REG: &mut device.rdp.regs_dpc[device::rdp::DPC_START_REG],
        DPC_END_REG: &mut device.rdp.regs_dpc[device::rdp::DPC_END_REG],
        DPC_STATUS_REG: &mut device.rdp.regs_dpc[device::rdp::DPC_STATUS_REG],
        PAL: device.cart.pal,
        widescreen: device.ui.config.video.widescreen,
        fullscreen: false,
        vsync: false,
        integer_scaling: false,
        upscale: device.ui.config.video.upscale.max(1),
        ssaa: device.ui.config.video.ssaa,
        crt: false,
    }
}

fn sink_user(device: &device::Device) -> *mut c_void {
    device
        .ui
        .video
        .frame_sink
        .as_deref()
        .map_or(std::ptr::null_mut(), |sink| {
            sink as *const FrameSink as *mut c_void
        })
}

fn open_rt64(device: &mut device::Device) -> bool {
    let user = sink_user(device);
    let path = device.ui.dirs.cache_dir.join("rt64");
    let _ = std::fs::create_dir_all(&path);
    let Ok(data_path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return false;
    };
    let core = RT64_CORE {
        rdram: device.rdram.mem.as_mut_ptr(),
        dmem: device.rsp.mem.as_mut_ptr(),
        imem: device.rsp.mem[0x1000..].as_mut_ptr(),
        rom_header: device.cart.rom.as_mut_ptr(),
        dpc_regs: device.rdp.regs_dpc.as_mut_ptr(),
        vi_regs: device.vi.regs.as_mut_ptr(),
    };
    let config = RT64_CONFIG {
        data_path: data_path.as_ptr(),
        user_config: device.ui.config.video.rt64.as_ptr(),
        gpu_uuid: device.ui.gpu_uuid.as_ptr(),
        frame_callback: Some(present_rt64_frame),
        output_size: Some(rt64_output_size),
        user,
        first_value: FRAME_VALUE.load(Ordering::Relaxed) + 1,
    };
    unsafe { rt64_open(&core, &config) != 0 }
}

fn open_parallel(device: &mut device::Device) -> bool {
    let user = sink_user(device);
    unsafe {
        rdp_set_frame_callback(None, std::ptr::null_mut());
        rdp_set_shared_frames(std::ptr::null(), None, std::ptr::null_mut(), 0);
    }
    if device.ui.host.is_some() {
        unsafe { rdp_set_frame_callback(Some(present_frame), user) };
        if device.ui.gpu_uuid != [0; 16] {
            unsafe {
                rdp_set_shared_frames(
                    device.ui.gpu_uuid.as_ptr(),
                    Some(present_shared_frame),
                    user,
                    FRAME_VALUE.load(Ordering::Relaxed) + 1,
                )
            };
        }
    }

    let gfx_info = build_gfx_info(device);
    let initialized = unsafe {
        rdp_init(
            std::ptr::null_mut(),
            gfx_info,
            std::ptr::null(),
            0,
            device.ui.storage.save_state_slot,
        );
        rdp_check_callback().emu_running
    };
    if initialized {
        restore_vi_registers(device);
    }
    initialized
}

fn restore_vi_registers(device: &device::Device) {
    for register in 0..device::vi::VI_REGS_COUNT {
        unsafe { rdp_set_vi_register(register as u32, device.vi.regs[register]) };
    }
}

fn open_renderers(device: &mut device::Device, selected_rt64: bool) -> bool {
    let rt64 = device.ui.config.video.renderer == ui::config::Renderer::Rt64
        && device.ui.host.is_some()
        && device.ui.gpu_uuid != [0; 16]
        && open_rt64(device);
    let next = match (selected_rt64, rt64) {
        (true, true) => {
            unsafe { rt64_commit() };
            RENDERER_RT64
        }
        (_, true) => RENDERER_UNDECIDED,
        (_, false) => RENDERER_PARALLEL,
    };
    if next != RENDERER_RT64 && !open_parallel(device) {
        close(&device.ui);
        device.ui.video.failed = true;
        log(device, "gopher64: renderer initialization failed");
        return false;
    }
    RENDERER.store(next, Ordering::Relaxed);
    UNDECIDED_COUNT.store(0, Ordering::Relaxed);
    match next {
        RENDERER_RT64 => log(device, "gopher64: RT64 renders"),
        RENDERER_PARALLEL => log(device, "gopher64: Parallel-RDP renders"),
        _ => {}
    }
    true
}

fn reopen(device: &mut device::Device) {
    let selected_rt64 = RENDERER.load(Ordering::Relaxed) == RENDERER_RT64;
    close(&device.ui);
    if !open_renderers(device, selected_rt64) {
        device.cpu.running = false;
    }
}

pub fn setting_changed(device: &mut device::Device, key: &str) {
    let renderer = RENDERER.load(Ordering::Relaxed);
    if renderer == RENDERER_CLOSED {
        return;
    }
    let wants_rt64 = device.ui.config.video.renderer == ui::config::Renderer::Rt64;
    if let Some(sink) = device.ui.video.frame_sink.as_ref() {
        sink.upscale
            .store(device.ui.config.video.upscale.max(1), Ordering::Relaxed);
        sink.widescreen
            .store(device.ui.config.video.widescreen, Ordering::Relaxed);
    }
    match key {
        "video.renderer" if wants_rt64 == (renderer == RENDERER_PARALLEL) => reopen(device),
        "rt64.graphicsAPI" | "rt64.internalColorFormat" if renderer != RENDERER_PARALLEL => {
            reopen(device)
        }
        _ if key.starts_with("rt64.") && renderer != RENDERER_PARALLEL => unsafe {
            rt64_set_user_config(device.ui.config.video.rt64.as_ptr())
        },
        _ if key.starts_with("parallel.") && renderer != RENDERER_RT64 => reopen(device),
        _ => {}
    }
}

fn close_parallel() {
    unsafe { rdp_close() };
}

fn log(device: &device::Device, message: &str) {
    if let Some(callbacks) = device.ui.host.as_ref() {
        modloader::log_info(callbacks, message);
    }
}

pub fn init(device: &mut device::Device, _netplay: bool) {
    close(&device.ui);
    device.ui.video.failed = false;
    device.ui.video.fps_tx = Some(tokio::sync::mpsc::unbounded_channel().0);
    device.ui.video.vis_tx = Some(tokio::sync::mpsc::unbounded_channel().0);

    if let Some(callbacks) = device.ui.host {
        device.ui.video.frame_sink = Some(Box::new(FrameSink {
            callbacks,
            upscale: AtomicU32::new(device.ui.config.video.upscale.max(1)),
            widescreen: AtomicBool::new(device.ui.config.video.widescreen),
        }));
    }

    open_renderers(device, false);
}

pub fn hle_task(device: &mut device::Device, text: u32, data: u32, data_ptr: u32) -> Option<u32> {
    let renderer = RENDERER.load(Ordering::Relaxed);
    if renderer != RENDERER_UNDECIDED && renderer != RENDERER_RT64 {
        return None;
    }
    let known = unsafe { rt64_known_ucode(device.rdram.mem.as_ptr(), text, data) } != 0;
    if renderer == RENDERER_UNDECIDED {
        if known {
            close_parallel();
            unsafe { rt64_commit() };
            RENDERER.store(RENDERER_RT64, Ordering::Relaxed);
            log(device, "gopher64: RT64 renders");
        } else {
            unsafe { rt64_close() };
            RENDERER.store(RENDERER_PARALLEL, Ordering::Relaxed);
            log(device, "gopher64: Parallel-RDP renders");
            return None;
        }
    }
    known.then(|| unsafe { rt64_run_task(text, data, data_ptr) })
}

pub fn resume_task() -> u32 {
    if RENDERER.load(Ordering::Relaxed) == RENDERER_RT64 {
        unsafe { rt64_resume_task() }
    } else {
        0
    }
}

pub fn waiting_address() -> u32 {
    if RENDERER.load(Ordering::Relaxed) == RENDERER_RT64 {
        unsafe { rt64_waiting_address() }
    } else {
        u32::MAX
    }
}

pub fn close(_ui: &ui::Ui) {
    RENDERER.store(RENDERER_CLOSED, Ordering::Relaxed);
    unsafe { rt64_close() };
    close_parallel();
}

fn parallel_active() -> bool {
    matches!(
        RENDERER.load(Ordering::Relaxed),
        RENDERER_PARALLEL | RENDERER_UNDECIDED
    )
}

pub unsafe fn set_texture_sources(
    ui: &ui::Ui,
    paths: *const *const c_char,
    count: u32,
    flags: u32,
) -> i32 {
    use modloader_abi::{
        MODLOADER_TEXTURE_DISABLED, MODLOADER_TEXTURE_ERROR, MODLOADER_TEXTURE_PENDING,
        MODLOADER_TEXTURE_UNSUPPORTED,
    };
    let state = unsafe { rt64_set_texture_sources(paths, count, flags) };
    if state == MODLOADER_TEXTURE_DISABLED as i32 || state == MODLOADER_TEXTURE_ERROR as i32 {
        return state;
    }
    match RENDERER.load(Ordering::Relaxed) {
        RENDERER_RT64 => state,
        RENDERER_UNDECIDED => MODLOADER_TEXTURE_PENDING as i32,
        RENDERER_CLOSED if ui.config.video.renderer == ui::config::Renderer::Rt64 => {
            MODLOADER_TEXTURE_PENDING as i32
        }
        _ => MODLOADER_TEXTURE_UNSUPPORTED as i32,
    }
}

pub fn failed(ui: &ui::Ui) -> bool {
    ui.video.failed
}

pub fn update_screen() {
    match RENDERER.load(Ordering::Relaxed) {
        RENDERER_RT64 => unsafe { rt64_update_screen() },
        RENDERER_UNDECIDED => {
            unsafe { rdp_update_screen() };
            if UNDECIDED_COUNT.fetch_add(1, Ordering::Relaxed) + 1 >= UNDECIDED_SCREENS {
                unsafe { rt64_close() };
                RENDERER.store(RENDERER_PARALLEL, Ordering::Relaxed);
            }
        }
        RENDERER_PARALLEL => unsafe { rdp_update_screen() },
        _ => {}
    }
}

pub fn render_frame() {
    if parallel_active() {
        unsafe { rdp_render_frame() }
    }
}

pub fn state_size() -> usize {
    if !parallel_active() {
        return 0;
    }
    unsafe { rdp_state_size() }
}

pub fn save_state(rdp_state: *mut u8) {
    if parallel_active() {
        unsafe { rdp_save_state(rdp_state) }
    }
}

pub fn idle() {
    if parallel_active() {
        unsafe { rdp_idle() }
    }
}

pub fn load_state(device: &mut device::Device, rdp_state: *const u8) {
    if !parallel_active() {
        return;
    }
    let gfx_info = build_gfx_info(device);
    unsafe { rdp_load_state(gfx_info, rdp_state) };
    restore_vi_registers(device);
}

pub fn state_loaded() {
    if RENDERER.load(Ordering::Relaxed) == RENDERER_RT64 {
        unsafe { rt64_discard() }
    }
}

pub fn pause_loop(_ui: &mut ui::Ui, _frame_time: f64) {}

pub fn check_callback(_device: &mut device::Device) -> (bool, bool) {
    (false, false)
}

pub fn set_register(reg: u32, value: u32) {
    if parallel_active() {
        unsafe { rdp_set_vi_register(reg, value) }
    }
}

pub fn process_rdp_list() -> u64 {
    if parallel_active() {
        return unsafe { rdp_process_commands() };
    }
    if RENDERER.load(Ordering::Relaxed) == RENDERER_RT64
        && unsafe { rt64_process_rdp() } & INTERRUPT_DP != 0
    {
        RT64_DP_CYCLES
    } else {
        0
    }
}

pub fn check_framebuffers(address: u32, length: u32) {
    if parallel_active() {
        unsafe { rdp_check_framebuffers(address, length) }
    }
}

pub fn onscreen_message(message: &str, milliseconds: MESSAGE_LENGTH) {
    if let Ok(message) = std::ffi::CString::new(message) {
        unsafe { rdp_onscreen_message(message.as_ptr(), milliseconds) };
    }
}
