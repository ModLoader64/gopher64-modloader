use crate::ui::storage;

#[derive(Default, Clone)]
pub struct Dirs {
    pub config_dir: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub cache_dir: std::path::PathBuf,
}

#[derive(Default)]
pub struct Storage {
    pub save_type: Vec<storage::SaveTypes>,
    pub paths: storage::Paths,
    pub saves: storage::Saves,
    pub save_state_slot: u32,
}

#[derive(Default)]
pub struct Usb {
    pub usb_tx: Option<tokio::sync::mpsc::UnboundedSender<UsbData>>,
    pub cart_rx: Option<tokio::sync::mpsc::UnboundedReceiver<UsbData>>,
}

#[derive(Clone)]
pub struct GameSettings {
    pub overclock: bool,
    pub disable_expansion_pak: bool,
    pub cheats: rustc_hash::FxHashMap<String, Option<String>>,
    pub load_savestate_slot: Option<u32>,
}

#[derive(Default, PartialEq, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub struct InputData {
    pub data: u32,
    pub pak_change_pressed: bool,
}

#[derive(Clone, Debug)]
pub struct UsbData {
    pub data: Vec<u8>,
    pub data_type: u32,
    pub data_size: u32,
}

pub fn audio_samples(memory: &[u8], address: usize, length: u64) -> Vec<i16> {
    let mut samples = vec![0; (length as usize / 4) * 2];
    for (index, frame) in samples.chunks_exact_mut(2).enumerate() {
        let byte = |offset| {
            address
                .checked_add(index * 4 + offset)
                .and_then(|address| memory.get(address))
                .copied()
                .unwrap_or(0)
        };
        // Left channel
        frame[0] = i16::from_le_bytes([byte(2), byte(3)]);
        // Right channel
        frame[1] = i16::from_le_bytes([byte(0), byte(1)]);
    }
    samples
}
