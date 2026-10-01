use super::events::{emulated_time, raise_event};
use super::*;

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct TaskState {
    pub(super) libultra: bool,
    pub(super) graphics_task: bool,
    pub(super) full_sync: bool,
    #[serde(skip)]
    pub(super) hle_wait: Option<(usize, [u8; 8])>,
}

const OS_TASK_OFFSET: usize = 0xFC0;
const OS_TASK_YIELDED: u32 = 0x0001;
const M_GFXTASK: u32 = 1;
const M_HVQMTASK: u32 = 7;
const SP_STATUS_SIG1: u32 = 1 << 8;
const SP_STATUS_SIG2: u32 = 1 << 9;
const HLE_SP_CYCLES: u64 = 1000;
const HLE_DP_CYCLES: u64 = 4000;
const HLE_POLL_CYCLES: u64 = 2000;

fn dmem_word(device: &device::Device, offset: usize) -> u32 {
    u32::from_be_bytes(device.rsp.mem[offset..offset + 4].try_into().unwrap())
}

fn libultra_task(device: &device::Device) -> Option<(u32, u32)> {
    let task_type = dmem_word(device, OS_TASK_OFFSET);
    let flags = dmem_word(device, OS_TASK_OFFSET + 4);
    let boot = (dmem_word(device, OS_TASK_OFFSET + 8) & 0x1FFFFFFF) as usize;
    let boot_size = dmem_word(device, OS_TASK_OFFSET + 12) as usize;
    if !(M_GFXTASK..=M_HVQMTASK).contains(&task_type)
        || boot_size == 0
        || boot_size > 0x1000
        || boot_size % 4 != 0
        || boot + boot_size > device.rdram.mem.len()
    {
        return None;
    }

    for offset in (0..boot_size).step_by(4) {
        let rdram = u32::from_ne_bytes(
            device.rdram.mem[boot + offset..boot + offset + 4]
                .try_into()
                .unwrap(),
        );
        if dmem_word(device, 0x1000 + offset) != rdram {
            return None;
        }
    }
    Some((task_type, flags))
}

pub fn rsp_task_starting(device: &mut device::Device) -> bool {
    if device.modloader.is_none() || device.rsp.regs2[device::rsp_interface::SP_PC_REG] != 0 {
        return false;
    }

    let task = libultra_task(device);
    let context = device.modloader.as_mut().unwrap();
    context.tasks.graphics_task = matches!(task, Some((M_GFXTASK, _)));
    if task.is_some() && !context.tasks.libultra {
        context.tasks.libultra = true;
        host_log(&context.callbacks, LOG_INFO, "gopher64: libultra tasks");
    }

    if matches!(task, Some((_, flags)) if flags & OS_TASK_YIELDED != 0) {
        return false;
    }

    let time = emulated_time(device, false);
    raise_event(device, EVENT_N64_RSP_TASK, time, std::ptr::null(), 0);
    task.is_some_and(|(task_type, _)| task_type == M_GFXTASK) && hle_graphics_task(device)
}

fn hle_graphics_task(device: &mut device::Device) -> bool {
    let text = dmem_word(device, OS_TASK_OFFSET + 16);
    let data = dmem_word(device, OS_TASK_OFFSET + 24);
    let data_ptr = dmem_word(device, OS_TASK_OFFSET + 48);
    let Some(flags) = ui::video::hle_task(device, text, data, data_ptr) else {
        return false;
    };
    hle_task_progress(device, flags);
    true
}

fn hle_task_progress(device: &mut device::Device, flags: u32) {
    if flags & ui::video::INTERRUPT_DP != 0 {
        device::events::create_event(device, device::events::EVENT_TYPE_DP, HLE_DP_CYCLES);
    }

    if flags & ui::video::TASK_WAITING != 0 {
        let address = ui::video::waiting_address() as usize;
        let command = device.rdram.mem[address..address + 8].try_into().unwrap();
        device.modloader.as_mut().unwrap().tasks.hle_wait = Some((address, command));
        device::events::create_event(device, device::events::EVENT_TYPE_SP, HLE_POLL_CYCLES);
        return;
    }

    device.rsp.regs[device::rsp_interface::SP_STATUS_REG] |= SP_STATUS_SIG2;
    device.rsp.cpu.broken = true;
    device::events::create_event(device, device::events::EVENT_TYPE_SP, HLE_SP_CYCLES);
}

pub fn hle_task_waiting(device: &mut device::Device) -> bool {
    let Some((address, command)) = device
        .modloader
        .as_ref()
        .and_then(|context| context.tasks.hle_wait)
    else {
        return false;
    };

    if device.rdram.mem[address..address + 8] == command {
        device::events::create_event(device, device::events::EVENT_TYPE_SP, HLE_POLL_CYCLES);
        return true;
    }

    device.modloader.as_mut().unwrap().tasks.hle_wait = None;
    let flags = ui::video::resume_task();
    hle_task_progress(device, flags);
    true
}

pub fn rsp_broke(device: &mut device::Device) {
    let status = device.rsp.regs[device::rsp_interface::SP_STATUS_REG];
    let Some(context) = device.modloader.as_mut() else {
        return;
    };

    if !context.tasks.graphics_task || status & SP_STATUS_SIG1 != 0 || status & SP_STATUS_SIG2 == 0
    {
        return;
    }

    context.tasks.graphics_task = false;
    events::raise_frame(device, false);
}

pub fn rdp_full_sync(device: &mut device::Device) {
    if let Some(context) = device.modloader.as_mut() {
        context.tasks.full_sync = true;
    }
}

pub(super) fn restore(device: &mut device::Device, saved: Option<TaskState>) {
    let tasks = saved.unwrap_or_else(|| {
        let task = libultra_task(device);
        TaskState {
            libultra: task.is_some(),
            graphics_task: matches!(task, Some((M_GFXTASK, _)))
                && device::events::get_event(device, device::events::EVENT_TYPE_SP).is_some(),
            full_sync: false,
            hle_wait: None,
        }
    });
    if let Some(context) = device.modloader.as_mut() {
        context.tasks = tasks;
        context.tasks.hle_wait = None;
    }
}
