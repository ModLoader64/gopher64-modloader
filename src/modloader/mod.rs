mod breakpoints;
mod debug;
mod events;
mod hypercalls;
mod memory;
mod settings;
mod states;
mod tasks;

pub use breakpoints::{before_instruction, before_uncached_instruction, trap_line};
pub use debug::debug_output;
pub use events::{reset, vertical_interrupt};
pub use memory::{map_ram_alias, translate_ram_extension};
pub use modloader_abi::{ModLoader_Frame as Frame, ModLoader_Gpu_Image as GpuImage};
pub use tasks::{hle_task_waiting, rdp_full_sync, rsp_broke, rsp_task_starting};

use crate::{device, retroachievements, ui};
use abi::n64::{
    MODLOADER_EVENT_N64_RSP_TASK as EVENT_N64_RSP_TASK, ModLoader_N64_Cpu_State as CpuState,
    ModLoader_N64_Vi as ViPayload,
};
use abi::{
    MODLOADER_ADAPTER_ABI_VERSION as ABI_VERSION, MODLOADER_BREAK_EXECUTE as BREAK_EXECUTE,
    MODLOADER_BREAK_READ as BREAK_READ, MODLOADER_BREAK_WRITE as BREAK_WRITE,
    MODLOADER_CODEC_BIG_ENDIAN as CODEC_BIG_ENDIAN,
    MODLOADER_CODEC_BIG_ENDIAN_WORD_SWAPPED as CODEC_BIG_ENDIAN_WORD_SWAPPED,
    MODLOADER_EVENT_FRAME as EVENT_FRAME, MODLOADER_EVENT_REFRESH as EVENT_REFRESH,
    MODLOADER_EVENT_RESET as EVENT_RESET, MODLOADER_EVENT_STATE_LOADED as EVENT_STATE_LOADED,
    MODLOADER_LOG_ERROR as LOG_ERROR, MODLOADER_LOG_INFO as LOG_INFO,
    MODLOADER_LOG_WARNING as LOG_WARNING, MODLOADER_SETTING_BINDING as SETTING_BINDING,
    MODLOADER_SETTING_BOOL as SETTING_BOOL, MODLOADER_SETTING_CHOICE as SETTING_CHOICE,
    MODLOADER_SETTING_FLOAT as SETTING_FLOAT, MODLOADER_SETTING_HIDDEN as SETTING_HIDDEN,
    MODLOADER_SETTING_INT as SETTING_INT, MODLOADER_SETTING_RESTART as SETTING_RESTART,
    MODLOADER_SETTING_SPEED_LIMIT as SETTING_SPEED_LIMIT, MODLOADER_SPACE_MAPPED as SPACE_MAPPED,
    MODLOADER_SPACE_WRITABLE as SPACE_WRITABLE, MODLOADER_WINDOW_HOST as WINDOW_HOST,
    ModLoader_Adapter_Config as AdapterConfig, ModLoader_Break as Break,
    ModLoader_Breakpoint as Breakpoint, ModLoader_Platform_Adapter as PlatformAdapter,
    ModLoader_Processor_Descriptor as ProcessorDescriptor, ModLoader_Setting as Setting,
    ModLoader_Space_Descriptor as SpaceDescriptor,
};
use breakpoints::*;
use memory::*;
use modloader_abi as abi;
use states::*;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

const RDRAM_ALLOCATION: u32 = 0x8000000;
const ROM_PAGE_ALIGNMENT: usize = 0x10000;
const ROM_CAPACITY: usize = 0xFC00000;
const SPACE_RDRAM: u32 = 0;
const SPACE_ROM: u32 = 1;
const SPACE_SP_MEM: u32 = 2;

#[cfg(windows)]
pub(crate) const HANDLE_OPAQUE: u32 = abi::MODLOADER_HANDLE_OPAQUE_WIN32 as u32;
#[cfg(not(windows))]
pub(crate) const HANDLE_OPAQUE: u32 = abi::MODLOADER_HANDLE_OPAQUE_FD as u32;
pub(crate) const HANDLE_D3D12: u32 = abi::MODLOADER_HANDLE_D3D12 as u32;

#[derive(Clone, Copy)]
pub struct HostCallbacks(abi::ModLoader_Host_Callbacks);

impl std::ops::Deref for HostCallbacks {
    type Target = abi::ModLoader_Host_Callbacks;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

unsafe impl Send for HostCallbacks {}
unsafe impl Sync for HostCallbacks {}

struct AdapterTable(PlatformAdapter);
unsafe impl Sync for AdapterTable {}

static ADAPTER_TABLE: AdapterTable = AdapterTable(PlatformAdapter {
    abiVersion: ABI_VERSION,
    structSize: size_of::<PlatformAdapter>() as u32,
    platformIdentifier: c"n64".as_ptr(),
    adapterIdentifier: c"gopher64".as_ptr(),
    create: Some(create),
    destroy: Some(destroy),
    run: Some(run),
    requestQuit: Some(request_quit),
    spaceCount: Some(space_count),
    spaceDescribe: Some(space_describe),
    processorCount: Some(processor_count),
    processorDescribe: Some(processor_describe),
    invalidateCode: Some(invalidate_code),
    translateAddress: Some(translate_address),
    saveState: Some(save_state),
    loadState: Some(load_state),
    setBreakpoints: Some(set_breakpoints),
    resizeImage: Some(resize_image),
    settingCount: Some(setting_count),
    settingDescribe: Some(setting_describe),
    settingSet: Some(setting_set),
    peekMemory: Some(peek_memory),
    pokeMemory: Some(poke_memory),
    reset: Some(reset_game),
    platformCall: None,
    setTextureSources: Some(set_texture_sources),
});

#[unsafe(no_mangle)]
pub extern "C" fn ModLoader_Get_Platform_Adapter() -> *const PlatformAdapter {
    &ADAPTER_TABLE.0
}

#[derive(Clone)]
pub struct Context {
    callbacks: HostCallbacks,
    shared: Arc<SharedState>,
    breakpoints: Breakpoints,
    tasks: tasks::TaskState,
    debug_output: bool,
    debug_line: String,
    usb_rx: Option<Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<ui::usb::UsbData>>>>,
    save_directory: Option<std::path::PathBuf>,
    video_changes: Vec<String>,
}

#[derive(Default)]
struct SharedState {
    pending_invalidations: Mutex<Vec<(u64, u64)>>,
    current_device: AtomicPtr<device::Device>,
    pending_state: Mutex<Option<Vec<u8>>>,
    at_vertical_interrupt: AtomicBool,
    reset_requested: AtomicBool,
    quit_requested: AtomicBool,
    current_memory: AtomicPtr<DcacheMemory>,
    pending_breakpoints: Mutex<Option<(Vec<Breakpoint>, bool)>>,
    scratch_memory: Mutex<Option<Box<DcacheMemory>>>,
}

impl SharedState {
    fn device_pointer(&self, fallback: *mut device::Device) -> *mut device::Device {
        let current = self.current_device.load(Ordering::Acquire);
        if current.is_null() {
            return fallback;
        }
        current
    }
}

struct Adapter {
    device: Box<device::Device>,
    rom_size: u64,
    reported_size: u32,
    shared: Arc<SharedState>,
    saved_state: Mutex<Option<Vec<u8>>>,
    settings: settings::Settings,
    runtime: tokio::runtime::Runtime,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        debug::flush_output(&mut self.device);
        free_rdram(&mut self.device.rdram);
        free_rom(&mut self.device.cart.rom);
    }
}

pub fn map_instructions(device: &mut device::Device) {
    if device.modloader.is_some() {
        device.cpu.special_instrs[hypercalls::HYPERCALL_IMMEDIATE_FUNCT] =
            hypercalls::hypercall_immediate;
        device.cpu.special_instrs[hypercalls::HYPERCALL_REGISTER_FUNCT] =
            hypercalls::hypercall_register;
        breakpoints::watch_loads_and_stores(device);
    }
}

pub fn redirect_saves(device: &mut device::Device) {
    let Some(directory) = device
        .modloader
        .as_ref()
        .and_then(|context| context.save_directory.clone())
    else {
        return;
    };
    let paths = &mut device.ui.storage.paths;
    for path in [
        &mut paths.eep_file_path,
        &mut paths.sra_file_path,
        &mut paths.fla_file_path,
        &mut paths.pak_file_path,
        &mut paths.sdcard_file_path,
        &mut paths.romsave_file_path,
    ] {
        if let Some(name) = path.file_name() {
            *path = directory.join(name);
        }
    }
}

fn host_log(callbacks: &HostCallbacks, level: abi::ModLoader_Log_Level, message: &str) {
    if let (Some(log), Ok(text)) = (callbacks.log, CString::new(message)) {
        unsafe { log(callbacks.host, level as u32, text.as_ptr()) };
    }
}

pub(crate) fn log_info(callbacks: &HostCallbacks, message: &str) {
    host_log(callbacks, LOG_INFO, message);
}

fn video_config(video: &mut ui::config::Video, settings: &settings::Settings) {
    video.upscale = settings.value("parallel.upscale").parse().unwrap_or(1);
    video.ssaa = settings.flag("parallel.ssaa");
    video.widescreen = settings.flag("parallel.widescreen");
    video.renderer = if settings.value("video.renderer") == "parallel" {
        ui::config::Renderer::Parallel
    } else {
        ui::config::Renderer::Rt64
    };
    video.rt64 = settings.rt64();
}

fn game_settings() -> ui::GameSettings {
    ui::GameSettings {
        overclock: false,
        disable_expansion_pak: false,
        cheats: Default::default(),
        load_savestate_slot: None,
    }
}

extern "C" fn create(
    config: *const AdapterConfig,
    callbacks: *const abi::ModLoader_Host_Callbacks,
    out_adapter: *mut *mut c_void,
) -> i32 {
    if config.is_null() || callbacks.is_null() || out_adapter.is_null() {
        return -1;
    }
    unsafe { *out_adapter = std::ptr::null_mut() };
    let config = unsafe { &*config };
    let callbacks = HostCallbacks(unsafe { *callbacks });
    if config.windowMode != WINDOW_HOST as u32 {
        host_log(
            &callbacks,
            LOG_ERROR,
            "gopher64: config.windowMode != WINDOW_HOST",
        );
        return -1;
    }

    if config.imageData.is_null() || !(0x1000..=ROM_CAPACITY as u64).contains(&config.imageSize) {
        host_log(
            &callbacks,
            LOG_ERROR,
            "gopher64: ROM size must be between 4 KiB and 252 MiB",
        );
        return -1;
    }
    let rom = unsafe { std::slice::from_raw_parts(config.imageData, config.imageSize as usize) };
    if std::str::from_utf8(&rom[0x3B..0x3E]).is_err() {
        host_log(
            &callbacks,
            LOG_ERROR,
            "gopher64: ROM identifier is not valid UTF-8",
        );
        return -1;
    }

    let data_directory = if config.dataDirectory.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(config.dataDirectory) }
            .to_str()
            .ok()
    };
    let Some(data_directory) = data_directory.filter(|directory| !directory.is_empty()) else {
        host_log(&callbacks, LOG_ERROR, "gopher64: no data directory");
        return -1;
    };
    let data_directory = std::path::PathBuf::from(data_directory);
    let save_directory = (!config.saveDirectory.is_null())
        .then(|| {
            unsafe { CStr::from_ptr(config.saveDirectory) }
                .to_str()
                .ok()
        })
        .flatten()
        .filter(|directory| !directory.is_empty())
        .map(std::path::PathBuf::from);
    let dirs = ui::Dirs {
        config_dir: data_directory.join("config"),
        data_dir: data_directory.clone(),
        cache_dir: data_directory.join("cache"),
    };

    for directory in [
        dirs.config_dir.clone(),
        dirs.cache_dir.clone(),
        dirs.data_dir.join("saves"),
        dirs.data_dir.join("states"),
    ]
    .into_iter()
    .chain(save_directory.clone())
    {
        if std::fs::create_dir_all(&directory).is_err() {
            host_log(
                &callbacks,
                LOG_ERROR,
                "gopher64: cannot create its data directories",
            );
            return -1;
        }
    }
    let shared = Arc::new(SharedState::default());

    let mut device = device::Device::new(true);
    device.ui.dirs = dirs;
    device.ui.host = Some(callbacks);
    device.ui.gpu_uuid = config.gpuUuid;
    let mut problems = Vec::new();
    let settings = settings::Settings::load(
        device.ui.dirs.config_dir.join("gopher64.json"),
        &mut problems,
    );
    for problem in problems {
        host_log(&callbacks, LOG_WARNING, &format!("gopher64: {}", problem));
    }

    let controllers = settings.number("emulation.controllers") as usize;
    for (port, enabled) in device
        .ui
        .config
        .input
        .controller_enabled
        .iter_mut()
        .enumerate()
    {
        *enabled = port < controllers;
    }
    for (port, key) in ui::input::PAK_KEYS.iter().enumerate() {
        ui::input::configure_pak(&mut device.ui, port, settings.value(key));
    }

    video_config(&mut device.ui.config.video, &settings);
    device.rdram.size = RDRAM_ALLOCATION;
    device.speed_limiter.enabled = settings.flag("emulation.speed_limit");
    let (usb_tx, usb_rx) = tokio::sync::mpsc::unbounded_channel();
    device.ui.usb.usb_tx = Some(usb_tx);
    device.modloader = Some(Context {
        callbacks,
        shared: shared.clone(),
        breakpoints: Breakpoints::default(),
        tasks: tasks::TaskState::default(),
        debug_output: settings.flag("emulation.debug_output"),
        debug_line: String::new(),
        usb_rx: Some(Arc::new(Mutex::new(usb_rx))),
        save_directory,
        video_changes: Vec::new(),
    });

    device::prepare_game(&mut device, rom, &game_settings());
    let reported_size: u32 = if settings.flag("emulation.expansion_pak") {
        0x800000
    } else {
        0x400000
    };

    for offset in [0x318, 0x3F0] {
        device.rdram.mem[offset..offset + 4].copy_from_slice(&reported_size.to_ne_bytes());
    }
    device.cart.rom = allocate_rom(&device.cart.rom);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => {
            host_log(
                &callbacks,
                LOG_ERROR,
                "gopher64: cannot start its I/O runtime",
            );
            free_rdram(&mut device.rdram);
            free_rom(&mut device.cart.rom);
            return -1;
        }
    };

    let adapter = Box::new(Adapter {
        rom_size: rom.len() as u64,
        device,
        reported_size,
        shared,
        saved_state: Mutex::new(None),
        settings,
        runtime,
    });
    unsafe { *out_adapter = Box::into_raw(adapter) as *mut c_void };
    0
}

extern "C" fn destroy(adapter: *mut c_void) {
    if !adapter.is_null() {
        unsafe { ui::video::rt64_set_texture_sources(std::ptr::null(), 0, 0) };
        drop(unsafe { Box::from_raw(adapter as *mut Adapter) });
    }
}

extern "C" fn set_texture_sources(
    adapter: *mut c_void,
    paths: *const *const c_char,
    count: u32,
    flags: u32,
) -> i32 {
    if adapter.is_null() {
        return abi::MODLOADER_TEXTURE_ERROR as i32;
    }
    let adapter = unsafe { &mut *(adapter as *mut Adapter) };
    let device = unsafe { &*adapter.shared.device_pointer(&raw mut *adapter.device) };
    unsafe { ui::video::set_texture_sources(&device.ui, paths, count, flags) }
}

extern "C" fn run(adapter: *mut c_void) -> i32 {
    if adapter.is_null() {
        return -1;
    }
    let adapter = unsafe { &mut *(adapter as *mut Adapter) };
    let _runtime = adapter.runtime.enter();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        device::start_game(
            &mut adapter.device,
            &[],
            game_settings(),
            retroachievements::RAConfig::default(),
            None,
        );
    }));
    debug::flush_output(&mut adapter.device);

    match result {
        Ok(()) => {
            if adapter.device.ui.video.failed {
                -1
            } else {
                0
            }
        }
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| text.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown".to_string());
            if let Some(context) = adapter.device.modloader.as_ref() {
                host_log(
                    &context.callbacks,
                    LOG_ERROR,
                    &format!("gopher64 stopped: {}", message),
                );
            }
            -1
        }
    }
}

extern "C" fn request_quit(adapter: *mut c_void) {
    if adapter.is_null() {
        return;
    }

    let adapter = adapter as *const Adapter;
    let shared = unsafe { &(*adapter).shared };
    shared.quit_requested.store(true, Ordering::Release);
}

extern "C" fn reset_game(adapter: *mut c_void) -> i32 {
    if adapter.is_null() {
        return -1;
    }
    let adapter = unsafe { &*(adapter as *const Adapter) };
    adapter
        .shared
        .reset_requested
        .store(true, Ordering::Release);
    0
}

extern "C" fn space_count(_adapter: *mut c_void) -> u32 {
    3
}

extern "C" fn processor_count(_adapter: *mut c_void) -> u32 {
    1
}

extern "C" fn processor_describe(
    adapter: *mut c_void,
    processor: u32,
    out_descriptor: *mut ProcessorDescriptor,
) -> i32 {
    if adapter.is_null() || processor != 0 || out_descriptor.is_null() {
        return -1;
    }
    unsafe {
        *out_descriptor = ProcessorDescriptor {
            name: c"vr4300".as_ptr(),
            stateFormat: abi::n64::MODLOADER_N64_VR4300_STATE_FORMAT.as_ptr().cast(),
            stateSize: size_of::<CpuState>() as u64,
        }
    };
    0
}

extern "C" fn space_describe(
    adapter: *mut c_void,
    space_index: u32,
    out_descriptor: *mut SpaceDescriptor,
) -> i32 {
    if adapter.is_null() || out_descriptor.is_null() {
        return -1;
    }

    let adapter = unsafe { &mut *(adapter as *mut Adapter) };
    let descriptor = match space_index {
        SPACE_RDRAM => SpaceDescriptor {
            name: c"rdram".as_ptr(),
            size: adapter.device.rdram.mem.len() as u64,
            usableSize: adapter.device.rdram.mem.len() as u64,
            reportedSize: adapter.reported_size as u64,
            codec: CODEC_BIG_ENDIAN_WORD_SWAPPED as u32,
            flags: (SPACE_MAPPED | SPACE_WRITABLE) as u32,
            hostBase: adapter.device.rdram.mem.as_mut_ptr(),
        },
        SPACE_ROM => SpaceDescriptor {
            name: c"rom".as_ptr(),
            size: ROM_CAPACITY as u64,
            usableSize: adapter.rom_size,
            reportedSize: 0,
            codec: CODEC_BIG_ENDIAN as u32,
            flags: (SPACE_MAPPED | SPACE_WRITABLE) as u32,
            hostBase: adapter.device.cart.rom.as_mut_ptr(),
        },
        SPACE_SP_MEM => SpaceDescriptor {
            name: c"sp_mem".as_ptr(),
            size: adapter.device.rsp.mem.len() as u64,
            usableSize: adapter.device.rsp.mem.len() as u64,
            reportedSize: 0,
            codec: CODEC_BIG_ENDIAN as u32,
            flags: SPACE_MAPPED as u32,
            hostBase: adapter.device.rsp.mem.as_mut_ptr(),
        },
        _ => return -1,
    };
    unsafe { *out_descriptor = descriptor };
    0
}

// WAMR shared heaps need page-multiple sizes
fn mapped_rom_size(image_size: usize) -> usize {
    image_size.div_ceil(ROM_PAGE_ALIGNMENT) * ROM_PAGE_ALIGNMENT
}

fn allocate_rom(image: &[u8]) -> Vec<u8> {
    let layout = std::alloc::Layout::from_size_align(ROM_CAPACITY, ROM_PAGE_ALIGNMENT).unwrap();
    let pointer = unsafe { std::alloc::alloc_zeroed(layout) };
    if pointer.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    let mut rom = unsafe { Vec::from_raw_parts(pointer, 0, ROM_CAPACITY) };
    rom.extend_from_slice(&image[..image.len().min(ROM_CAPACITY)]);
    rom.resize(mapped_rom_size(rom.len()), 0);
    rom
}

fn free_rdram(rdram: &mut device::rdram::Rdram) {
    let mem = std::mem::ManuallyDrop::new(std::mem::take(&mut rdram.mem));
    if mem.capacity() != 0 {
        let layout = std::alloc::Layout::from_size_align(mem.capacity(), 0x10000).unwrap();
        unsafe { std::alloc::dealloc(mem.as_ptr() as *mut u8, layout) };
    }
}

fn free_rom(rom: &mut Vec<u8>) {
    let rom = std::mem::ManuallyDrop::new(std::mem::take(rom));
    if rom.capacity() == ROM_CAPACITY {
        let layout = std::alloc::Layout::from_size_align(ROM_CAPACITY, ROM_PAGE_ALIGNMENT).unwrap();
        unsafe { std::alloc::dealloc(rom.as_ptr() as *mut u8, layout) };
    } else {
        drop(std::mem::ManuallyDrop::into_inner(rom));
    }
}

extern "C" fn setting_count(adapter: *mut c_void) -> u32 {
    if adapter.is_null() {
        return 0;
    }
    unsafe { &*(adapter as *const Adapter) }.settings.count()
}

extern "C" fn setting_describe(adapter: *mut c_void, index: u32, out_setting: *mut Setting) -> i32 {
    if adapter.is_null() || out_setting.is_null() {
        return -1;
    }

    match unsafe { &*(adapter as *const Adapter) }
        .settings
        .describe(index)
    {
        Some(setting) => {
            unsafe { *out_setting = setting };
            0
        }
        None => -1,
    }
}

extern "C" fn setting_set(adapter: *mut c_void, key: *const c_char, value: *const c_char) -> i32 {
    if adapter.is_null() || key.is_null() || value.is_null() {
        return -1;
    }

    let adapter = unsafe { &mut *(adapter as *mut Adapter) };
    let (Ok(key), Ok(value)) = (
        unsafe { CStr::from_ptr(key) }.to_str(),
        unsafe { CStr::from_ptr(value) }.to_str(),
    ) else {
        return -1;
    };
    let mut problems = Vec::new();
    let applies = adapter.settings.set(key, value, &mut problems);
    let device = unsafe { &mut *adapter.shared.device_pointer(&raw mut *adapter.device) };

    if let Some(context) = device.modloader.as_ref() {
        for problem in problems {
            host_log(
                &context.callbacks,
                LOG_WARNING,
                &format!("gopher64: {}", problem),
            );
        }
    }

    match applies {
        Some(true) => match key {
            "emulation.speed_limit" => {
                device.speed_limiter.enabled = adapter.settings.flag(key);
            }
            "emulation.debug_output" => {
                debug::set_enabled(device, adapter.settings.flag(key));
            }
            _ if key.starts_with("video.")
                || key.starts_with("parallel.")
                || key.starts_with("rt64.") =>
            {
                video_config(&mut device.ui.config.video, &adapter.settings);
                if let Some(context) = device.modloader.as_mut() {
                    context.video_changes.push(key.to_string());
                }
            }
            _ => ui::input::setting_changed(device, key, adapter.settings.value(key)),
        },
        Some(false) => {}
        None => return -1,
    }
    0
}

extern "C" fn resize_image(adapter: *mut c_void, size: u64) -> i32 {
    if adapter.is_null() || size > ROM_CAPACITY as u64 {
        return -1;
    }

    let adapter = unsafe { &mut *(adapter as *mut Adapter) };
    let device = unsafe { &mut *adapter.shared.device_pointer(&raw mut *adapter.device) };
    resize_rom(&mut device.cart.rom, size as usize);
    adapter.rom_size = size;
    0
}

fn resize_rom(rom: &mut Vec<u8>, size: usize) {
    if size < rom.len() {
        rom[size..].fill(0);
    }
    rom.resize(mapped_rom_size(size), 0);
}
