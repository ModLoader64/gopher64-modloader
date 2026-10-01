use crate::{device, ui};
use device::controller::{PakHandler, PakType, mempak, rumble};
use device::events;

pub use crate::ui_common::InputData;

pub const PAK_KEYS: [&str; 4] = [
    "controller1.pak",
    "controller2.pak",
    "controller3.pak",
    "controller4.pak",
];

#[derive(Clone, Copy, Default)]
pub enum PakSelection {
    #[default]
    Auto,
    Memory,
    Rumble,
}

pub fn configure_pak(ui: &mut ui::Ui, port: usize, value: &str) {
    ui.input.paks[port] = match value {
        "memory" => PakSelection::Memory,
        "rumble" => PakSelection::Rumble,
        _ => PakSelection::Auto,
    };
}

fn handler(pak_type: PakType) -> PakHandler {
    match pak_type {
        PakType::RumblePak => PakHandler {
            read: rumble::read,
            write: rumble::write,
            pak_type,
        },
        _ => PakHandler {
            read: mempak::read,
            write: mempak::write,
            pak_type: PakType::MemPak,
        },
    }
}

fn selected_handler(device: &device::Device, port: usize) -> PakHandler {
    match device.ui.input.paks[port] {
        PakSelection::Auto => device::pif::get_default_handler(device),
        PakSelection::Memory => handler(PakType::MemPak),
        PakSelection::Rumble => handler(PakType::RumblePak),
    }
}

pub fn apply_paks(device: &mut device::Device) {
    for port in 0..4 {
        if device.ui.config.input.controller_enabled[port]
            && !(port == 3 && device.ui.config.input.emulate_vru)
        {
            device.pif.channels[port].pak_handler = Some(selected_handler(device, port));
        }
    }
}

pub fn setting_changed(device: &mut device::Device, key: &str, value: &str) {
    let Some(port) = PAK_KEYS.iter().position(|candidate| *candidate == key) else {
        return;
    };
    configure_pak(&mut device.ui, port, value);
    if device.pif.channels[port].process.is_none()
        || (port == 3 && device.ui.config.input.emulate_vru)
    {
        return;
    }
    let selected = selected_handler(device, port).pak_type;
    let channel = &mut device.pif.channels[port];
    if channel
        .pak_handler
        .as_ref()
        .is_some_and(|current| current.pak_type == selected)
    {
        return;
    }
    channel.pak_handler = None;
    channel.change_pak = selected;
    set_rumble(&device.ui, port, 0);
    if events::get_event(device, events::EVENT_TYPE_PAK).is_none() {
        events::create_event(device, events::EVENT_TYPE_PAK, device.cpu.clock_rate / 2);
    }
}

pub fn finish_pak_switch(device: &mut device::Device) {
    for channel in device.pif.channels.iter_mut().take(4) {
        if channel.change_pak != PakType::None {
            channel.pak_handler = Some(handler(channel.change_pak));
            channel.change_pak = PakType::None;
        }
    }
}

pub fn init(_ui: &mut ui::Ui) {}

pub fn close(ui: &mut ui::Ui) {
    for port in 0..4 {
        set_rumble(ui, port, 0);
    }
}

pub fn get(ui: &mut ui::Ui, channel: usize) -> InputData {
    let mut state = 0u32;
    if let Some(callbacks) = ui.host
        && let Some(poll_input) = callbacks.pollInput
        && unsafe {
            poll_input(
                callbacks.host,
                channel as u32,
                std::ptr::from_mut(&mut state).cast(),
                size_of::<u32>() as u64,
            )
        } > 0
    {
        return InputData {
            data: state,
            pak_change_pressed: false,
        };
    }
    InputData::default()
}

pub fn set_rumble(ui: &ui::Ui, channel: usize, rumble: u8) {
    if let Some(callbacks) = ui.host
        && let Some(set_rumble) = callbacks.setRumble
    {
        unsafe { set_rumble(callbacks.host, channel as u32, rumble as u32) };
    }
}
