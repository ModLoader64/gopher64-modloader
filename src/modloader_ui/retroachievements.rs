#[derive(Default)]
pub struct RAConfig {
    pub enabled: bool,
    pub rich_presence: bool,
}

pub fn load_game(
    _rom: &[u8],
    _rom_size: usize,
    _discord_rich_presence: bool,
) -> (
    Option<tokio::sync::watch::Sender<()>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    (None, None)
}

pub fn unload_game(
    _discord_watch_tx: Option<tokio::sync::watch::Sender<()>>,
    _discord_handle: Option<tokio::task::JoinHandle<()>>,
) {
}

pub fn set_rdram(_rdram: *const u8, _rdram_size: usize) {}

pub fn do_frame() {}

pub fn get_hardcore() -> bool {
    false
}

pub fn state_size() -> usize {
    0
}

pub fn save_state(_state: *mut u8, _state_size: usize) {}

pub fn load_state(_state: *const u8, _state_size: usize) {}
