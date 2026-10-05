use super::*;

pub(super) struct Description {
    pub(super) key: &'static CStr,
    pub(super) label: &'static CStr,
    pub(super) page: &'static CStr,
    pub(super) help: &'static CStr,
    pub(super) choices: &'static CStr,
    pub(super) default: &'static str,
    pub(super) kind: abi::ModLoader_Setting_Type,
    pub(super) flags: abi::ModLoader_Setting_Flags,
    pub(super) minimum: f64,
    pub(super) maximum: f64,
}

const fn describe(
    key: &'static CStr,
    label: &'static CStr,
    page: &'static CStr,
    help: &'static CStr,
    kind: abi::ModLoader_Setting_Type,
    default: &'static str,
    restart: bool,
) -> Description {
    Description {
        key,
        label,
        page,
        help,
        choices: c"",
        default,
        kind,
        flags: if restart { SETTING_RESTART } else { 0 },
        minimum: 0.0,
        maximum: 0.0,
    }
}

const fn choice(
    key: &'static CStr,
    label: &'static CStr,
    page: &'static CStr,
    help: &'static CStr,
    choices: &'static CStr,
    default: &'static str,
    restart: bool,
) -> Description {
    Description {
        choices,
        ..describe(key, label, page, help, SETTING_CHOICE, default, restart)
    }
}

const fn number(
    key: &'static CStr,
    label: &'static CStr,
    page: &'static CStr,
    kind: abi::ModLoader_Setting_Type,
    default: &'static str,
    restart: bool,
    minimum: f64,
    maximum: f64,
) -> Description {
    Description {
        minimum,
        maximum,
        ..describe(key, label, page, c"", kind, default, restart)
    }
}

const fn binding(key: &'static CStr, label: &'static CStr, default: &'static str) -> Description {
    describe(
        key,
        label,
        c"Input/Controller 1",
        c"",
        SETTING_BINDING,
        default,
        false,
    )
}

const fn pak(key: &'static CStr, page: &'static CStr) -> Description {
    choice(
        key,
        c"Accessory",
        page,
        c"Changing the accessory briefly disconnects it from the controller",
        c"auto\tGame default\nmemory\tController Pak\nrumble\tRumble Pak",
        "auto",
        false,
    )
}

const RT64: &CStr = c"Video/RT64";
const PARALLEL: &CStr = c"Video/Parallel-RDP";

pub(super) static DESCRIPTIONS: &[Description] = &[
    choice(
        c"video.renderer",
        c"Renderer",
        c"Video/N64",
        c"RT64 when it knows the game's graphics microcode, else Parallel-RDP",
        c"rt64\tRT64\nparallel\tParallel-RDP",
        "rt64",
        false,
    ),
    choice(
        c"parallel.upscale",
        c"Upscale",
        PARALLEL,
        c"",
        c"1\t1x\n2\t2x\n4\t4x\n8\t8x",
        "1",
        false,
    ),
    describe(
        c"parallel.ssaa",
        c"Supersampling",
        PARALLEL,
        c"Renders upscaled and shows the N64's resolution",
        SETTING_BOOL,
        "false",
        false,
    ),
    describe(
        c"parallel.widescreen",
        c"Widescreen",
        PARALLEL,
        c"Stretches the picture to 16:9",
        SETTING_BOOL,
        "false",
        false,
    ),
    choice(
        c"rt64.graphicsAPI",
        c"Graphics API",
        RT64,
        c"",
        c"Automatic\nD3D12\nVulkan",
        "Automatic",
        false,
    ),
    describe(
        c"rt64.windowSize",
        c"Resolution: Use Window Size",
        RT64,
        c"Renders as large as the window; off: at the resolution scale",
        SETTING_BOOL,
        "true",
        false,
    ),
    number(
        c"rt64.resolutionMultiplier",
        c"Resolution scale",
        RT64,
        SETTING_FLOAT,
        "2",
        false,
        1.0,
        8.0,
    ),
    choice(
        c"rt64.antialiasing",
        c"Antialiasing",
        RT64,
        c"",
        c"None\nMSAA2X\tMSAA 2x\nMSAA4X\tMSAA 4x\nMSAA8X\tMSAA 8x",
        "None",
        false,
    ),
    choice(
        c"rt64.aspectRatio",
        c"Aspect ratio",
        RT64,
        c"",
        c"Original\nExpand\nManual",
        "Original",
        false,
    ),
    number(
        c"rt64.aspectTarget",
        c"Manual aspect ratio",
        RT64,
        SETTING_FLOAT,
        "1.7777777777777777",
        false,
        1.0,
        4.0,
    ),
    describe(
        c"rt64.threePointFiltering",
        c"Three-point filtering",
        RT64,
        c"",
        SETTING_BOOL,
        "true",
        false,
    ),
    choice(
        c"rt64.refreshRate",
        c"Refresh rate",
        RT64,
        c"",
        c"Original\nDisplay\nManual",
        "Original",
        false,
    ),
    number(
        c"rt64.refreshRateTarget",
        c"Manual refresh rate",
        RT64,
        SETTING_INT,
        "60",
        false,
        30.0,
        360.0,
    ),
    choice(
        c"rt64.internalColorFormat",
        c"Color format",
        RT64,
        c"High: 16 bits per channel",
        c"Automatic\nStandard\nHigh",
        "Automatic",
        false,
    ),
    Description {
        flags: SETTING_SPEED_LIMIT,
        ..describe(
            c"emulation.speed_limit",
            c"Speed limit",
            c"Emulation",
            c"Off: as fast as the computer can",
            SETTING_BOOL,
            "true",
            false,
        )
    },
    number(
        c"emulation.controllers",
        c"Controllers",
        c"Emulation",
        SETTING_INT,
        "1",
        true,
        0.0,
        4.0,
    ),
    describe(
        c"emulation.expansion_pak",
        c"Expansion Pak",
        c"Emulation",
        c"8 MiB of RDRAM, else 4",
        SETTING_BOOL,
        "true",
        true,
    ),
    describe(
        c"emulation.debug_output",
        c"Debug output",
        c"Emulation",
        c"IS-Viewer and SummerCart64 text in ModLoader's console",
        SETTING_BOOL,
        "false",
        false,
    ),
    pak(c"controller1.pak", c"Input/Controller 1"),
    pak(c"controller2.pak", c"Input/Controller 2"),
    pak(c"controller3.pak", c"Input/Controller 3"),
    pak(c"controller4.pak", c"Input/Controller 4"),
    binding(c"controller1.a", c"A", "key:X|pad:south"),
    binding(c"controller1.b", c"B", "key:C|pad:west"),
    binding(c"controller1.z", c"Z", "key:Z|axis:lefttrigger+"),
    binding(c"controller1.start", c"Start", "key:Return|pad:start"),
    binding(c"controller1.l", c"L", "key:A|pad:leftshoulder"),
    binding(
        c"controller1.r",
        c"R",
        "key:S|pad:rightshoulder|axis:righttrigger+",
    ),
    binding(c"controller1.c_up", c"C up", "key:I|pad:north|axis:righty-"),
    binding(
        c"controller1.c_down",
        c"C down",
        "key:K|pad:east|axis:righty+",
    ),
    binding(c"controller1.c_left", c"C left", "key:J|axis:rightx-"),
    binding(c"controller1.c_right", c"C right", "key:L|axis:rightx+"),
    binding(c"controller1.dpad_up", c"D-pad up", "key:T|pad:dpup"),
    binding(c"controller1.dpad_down", c"D-pad down", "key:G|pad:dpdown"),
    binding(c"controller1.dpad_left", c"D-pad left", "key:F|pad:dpleft"),
    binding(
        c"controller1.dpad_right",
        c"D-pad right",
        "key:H|pad:dpright",
    ),
    binding(c"controller1.stick_up", c"Stick up", "key:Up|axis:lefty-"),
    binding(
        c"controller1.stick_down",
        c"Stick down",
        "key:Down|axis:lefty+",
    ),
    binding(
        c"controller1.stick_left",
        c"Stick left",
        "key:Left|axis:leftx-",
    ),
    binding(
        c"controller1.stick_right",
        c"Stick right",
        "key:Right|axis:leftx+",
    ),
];
