use crate::{device, ui};

pub struct NetplayConfig {
    pub server_addr: String,
    pub player_number: usize,
    pub number_of_players: usize,
    pub input_delay: usize,
    pub ice_config_path: std::path::PathBuf,
}

#[derive(Clone, Copy, PartialEq)]
pub enum InputStatus {
    Confirmed,
}

pub struct Session;

impl Session {
    pub fn frames_ahead(&self) -> i32 {
        0
    }
}

pub struct Netplay {
    pub session: Session,
    pub player_number: usize,
    pub connected: [bool; 4],
    pub input_delay: usize,
    pub inputs: Vec<(ui::input::InputData, InputStatus)>,
}

pub fn init(
    _device: &mut device::Device,
    _netplay_config: &NetplayConfig,
    _pal: bool,
) -> Option<Netplay> {
    None
}

pub fn close(_netplay: &mut Netplay) {}

pub fn in_rollback(_netplay: Option<&Netplay>) -> bool {
    false
}

pub fn process_requests(_device: &mut device::Device) -> Vec<(ui::input::InputData, InputStatus)> {
    vec![(ui::input::InputData::default(), InputStatus::Confirmed); 4]
}

pub fn send_rng(_netplay: &mut Netplay, _seed: u64) {}

pub fn receive_rng(_netplay: &mut Netplay) -> u64 {
    0
}

pub fn send_rtc(_netplay: &mut Netplay, _rtc: i64) {}

pub fn receive_rtc(_netplay: &mut Netplay) -> i64 {
    0
}

pub fn send_save(_netplay: &mut Netplay, _save_type: &str, _save_data: &[u8]) {}

pub fn receive_save(_netplay: &mut Netplay, _save_type: &str, _save_data: &mut Vec<u8>) {}
