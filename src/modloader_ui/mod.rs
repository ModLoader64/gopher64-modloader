pub mod audio;
pub mod config;
pub mod input;
#[path = "../ui/storage.rs"]
pub mod storage;
pub mod usb;
pub mod video;
pub mod vru;

pub use crate::ui_common::{Dirs, GameSettings, Storage, Usb};

#[derive(Default)]
pub struct Input {
    pub paks: [input::PakSelection; 4],
}

#[derive(Default)]
pub struct Video {
    pub fps_tx: Option<tokio::sync::mpsc::UnboundedSender<bool>>,
    pub vis_tx: Option<tokio::sync::mpsc::UnboundedSender<bool>>,
    pub frame_sink: Option<Box<video::FrameSink>>,
    pub failed: bool,
}

#[derive(Default)]
pub struct Ui {
    pub dirs: Dirs,
    pub config: config::Config,
    pub game_id: String,
    pub game_hash: String,
    pub input: Input,
    pub storage: Storage,
    pub video: Video,
    pub usb: Usb,
    pub host: Option<crate::modloader::HostCallbacks>,
    pub gpu_uuid: [u8; 16],
}

impl Ui {
    pub fn new() -> Ui {
        let mut ui = Ui::default();
        ui.storage.saves.write_to_disk = true;
        ui
    }
}
