use super::*;

const VI_STATUS_REG: usize = 0;
const VI_ORIGIN_REG: usize = 1;
const VI_WIDTH_REG: usize = 2;

fn vi_payload(device: &device::Device) -> ViPayload {
    ViPayload {
        status: device.vi.regs[VI_STATUS_REG],
        origin: device.vi.regs[VI_ORIGIN_REG],
        width: device.vi.regs[VI_WIDTH_REG],
        reserved: 0,
    }
}

pub(super) fn raise_frame(device: &mut device::Device, at_vertical_interrupt: bool) {
    let payload = vi_payload(device);
    let time = emulated_time(device, at_vertical_interrupt);
    raise_event(
        device,
        EVENT_FRAME,
        time,
        &payload as *const ViPayload as *const c_void,
        size_of::<ViPayload>() as u64,
    );
}

pub(super) fn emulated_time(device: &device::Device, at_vertical_interrupt: bool) -> u64 {
    let mut seconds = device.vi.elapsed_time;
    if !at_vertical_interrupt
        && device.cpu.clock_rate != 0
        && let Some(next) = device::events::get_event(device, device::events::EVENT_TYPE_VI)
    {
        let count = device.cpu.cop0.regs[device::cop0::COP0_COUNT_REG];
        let since = device
            .vi
            .delay
            .saturating_sub(next.count.saturating_sub(count));
        seconds += since as f64 / device.cpu.clock_rate as f64;
    }
    (seconds * 1e9) as u64
}

pub(super) fn raise_event(
    device: &mut device::Device,
    event: abi::ModLoader_Event,
    time: u64,
    data: *const c_void,
    size: u64,
) {
    with_host(device, |callbacks| {
        if let Some(handler) = callbacks.event {
            unsafe { handler(callbacks.host, event as u32, time, data, size) };
        }
    });
}

pub fn vertical_interrupt(device: &mut device::Device) {
    let Some(flag) = device
        .modloader
        .as_ref()
        .map(|context| context.shared.clone())
    else {
        return;
    };
    debug::drain_usb(device);
    let payload = vi_payload(device);
    let time = emulated_time(device, true);
    flag.at_vertical_interrupt.store(true, Ordering::Release);
    raise_event(
        device,
        EVENT_REFRESH,
        time,
        &payload as *const ViPayload as *const c_void,
        size_of::<ViPayload>() as u64,
    );
    flag.at_vertical_interrupt.store(false, Ordering::Release);
    let context = device.modloader.as_mut().unwrap();
    if context.shared.reset_requested.swap(false, Ordering::AcqRel) {
        press_reset(device);
    }
    let context = device.modloader.as_mut().unwrap();
    if !context.tasks.libultra && context.tasks.full_sync {
        context.tasks.full_sync = false;
        raise_frame(device, true);
    }

    let changes = device
        .modloader
        .as_mut()
        .filter(|context| context.tasks.hle_wait.is_none())
        .map(|context| std::mem::take(&mut context.video_changes))
        .unwrap_or_default();
    for key in changes {
        ui::video::setting_changed(device, &key);
    }
    load_pending(device);
}

pub(super) fn state_loaded(device: &mut device::Device) {
    debug::clear_output(device);
    decode_icache(device);
    ui::video::state_loaded();
    let time = emulated_time(device, false);
    raise_event(device, EVENT_STATE_LOADED, time, std::ptr::null(), 0);
}

fn press_reset(device: &mut device::Device) {
    device.cpu.cop0.regs[device::cop0::COP0_CAUSE_REG] |= device::cop0::COP0_CAUSE_IP4;
    device.cpu.cop0.regs[device::cop0::COP0_CAUSE_REG] &= !device::cop0::COP0_CAUSE_EXCCODE_MASK;
    device::events::create_event(
        device,
        device::events::EVENT_TYPE_NMI,
        device.cpu.clock_rate,
    );
}

pub fn reset(device: &mut device::Device) {
    debug::flush_output(device);
    let time = emulated_time(device, false);
    raise_event(device, EVENT_RESET, time, std::ptr::null(), 0);
}
