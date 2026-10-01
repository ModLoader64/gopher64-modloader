#[derive(Default)]
pub struct Input {
    pub emulate_vru: bool,
    pub controller_enabled: [bool; 4],
}

#[derive(Default)]
pub struct Emulation {
    pub rewind: bool,
}

#[derive(Clone, Copy, Default, PartialEq)]
pub enum Renderer {
    #[default]
    Rt64,
    Parallel,
}

#[derive(Default)]
pub struct Video {
    pub upscale: u32,
    pub ssaa: bool,
    pub widescreen: bool,
    pub renderer: Renderer,
    pub rt64: std::ffi::CString,
}

#[derive(Default)]
pub struct Config {
    pub input: Input,
    pub emulation: Emulation,
    pub video: Video,
}
