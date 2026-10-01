use crate::{device, ui};

pub fn init(_device: &mut device::Device) {}

pub fn close(_ui: &mut ui::Ui) {}

pub fn update_freq(_device: &mut device::Device) {}

pub fn resume_game_audio(_ui: &mut ui::Ui) {}

pub fn pause_game_audio(_ui: &mut ui::Ui) {}

pub fn play_audio(device: &device::Device, dram_addr: usize, length: u64) {
    let Some(callbacks) = device.ui.host else {
        return;
    };
    let Some(push_audio) = callbacks.pushAudio else {
        return;
    };
    let samples = crate::ui_common::audio_samples(&device.rdram.mem, dram_addr, length);
    if samples.is_empty() {
        return;
    }

    unsafe {
        push_audio(
            callbacks.host,
            samples.as_ptr(),
            (samples.len() / 2) as u32,
            device.ai.freq as u32,
        )
    };
}
