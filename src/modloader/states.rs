use super::*;
use crate::savestates;
use std::io::Read;

pub(super) extern "C" fn save_state(
    adapter: *mut c_void,
    destination: *mut u8,
    inout_size: *mut u64,
) -> i32 {
    if adapter.is_null() || inout_size.is_null() {
        return -1;
    }
    let adapter = unsafe { &*(adapter as *const Adapter) };
    let current = adapter.shared.current_device.load(Ordering::Acquire);
    if current.is_null() {
        return -2;
    }

    if !adapter.shared.at_vertical_interrupt.load(Ordering::Acquire) {
        return -3;
    }

    let device = unsafe { &*current };
    if device
        .modloader
        .as_ref()
        .is_some_and(|context| context.tasks.hle_wait.is_some())
    {
        unsafe { *inout_size = 0 };
        return 2;
    }

    let Ok(mut saved) = adapter.saved_state.lock() else {
        return -1;
    };

    if destination.is_null() || saved.is_none() {
        let device = unsafe { &mut *current };
        let memory = adapter.shared.current_memory.load(Ordering::Acquire);
        *saved = snapshot(device, memory);
    }

    let Some(bytes) = saved.as_ref() else {
        return -1;
    };
    let capacity = unsafe { *inout_size };
    unsafe { *inout_size = bytes.len() as u64 };
    if destination.is_null() || capacity < bytes.len() as u64 {
        return 1;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len()) };
    *saved = None;
    0
}

pub(super) extern "C" fn load_state(adapter: *mut c_void, source: *const u8, size: u64) -> i32 {
    if adapter.is_null() || source.is_null() || size == 0 || size > isize::MAX as u64 {
        return -1;
    }

    let adapter = unsafe { &*(adapter as *const Adapter) };
    let bytes = unsafe { std::slice::from_raw_parts(source, size as usize) }.to_vec();
    match adapter.shared.pending_state.lock() {
        Ok(mut pending) => {
            *pending = Some(bytes);
            0
        }
        Err(_) => -1,
    }
}

fn snapshot(device: &mut device::Device, memory: *mut DcacheMemory) -> Option<Vec<u8>> {
    if memory.is_null() {
        return encode(device);
    }

    let memory = unsafe { &mut *memory };
    dcache_after_host(device, memory);
    let bytes = encode(device);
    dcache_before_host(device, memory);
    bytes
}

fn encode(device: &mut device::Device) -> Option<Vec<u8>> {
    ui::video::idle();
    let mut rdp_state: Vec<u8> = vec![0; ui::video::state_size()];
    ui::video::save_state(rdp_state.as_mut_ptr());
    let device_data = postcard::to_stdvec(&*device).ok()?;
    let saves_data = postcard::to_stdvec(&device.ui.storage.saves).ok()?;
    let task_data = postcard::to_stdvec(&device.modloader.as_ref()?.tasks).ok()?;
    ui::storage::compress_file(&[
        (&device_data, "device"),
        (&saves_data, "saves"),
        (&rdp_state, "rdp_state"),
        (&[], "ra_state"),
        (&task_data, "modloader_tasks"),
    ])
    .ok()
}

fn decode_tasks(bytes: &[u8]) -> Result<Option<tasks::TaskState>, ()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| ())?;
    let mut file = match archive.by_name("modloader_tasks") {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err(()),
    };
    if file.size() > 64 {
        return Err(());
    }
    let mut data = Vec::new();
    file.read_to_end(&mut data).map_err(|_| ())?;
    let (state, remaining) = postcard::take_from_bytes(&data).map_err(|_| ())?;
    if !remaining.is_empty() {
        return Err(());
    }
    Ok(Some(state))
}

pub(super) fn load_pending(device: &mut device::Device) {
    let pending = device
        .modloader
        .as_ref()
        .and_then(|context| context.shared.pending_state.lock().ok()?.take());
    if let Some(bytes) = pending {
        let Ok(task_state) = decode_tasks(&bytes) else {
            if let Some(context) = device.modloader.as_ref() {
                host_log(
                    &context.callbacks,
                    LOG_ERROR,
                    "gopher64: invalid task state metadata",
                );
            }
            return;
        };
        let state = savestates::decode_savestate(bytes);
        if savestates::apply_savestate(device, state, false) {
            tasks::restore(device, task_state);
            events::state_loaded(device);
        }
    }
}
