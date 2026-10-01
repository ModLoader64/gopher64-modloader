use super::*;

pub const HYPERCALL_IMMEDIATE_FUNCT: usize = 0x28;
pub const HYPERCALL_REGISTER_FUNCT: usize = 0x29;

pub(super) fn immediate_hypercall_id(opcode: u32) -> u32 {
    (opcode >> 6) & 0xFFFFF
}

pub(super) fn uses_fgr32(device: &device::Device, index: usize) -> bool {
    device.cpu.cop0.regs[device::cop0::COP0_STATUS_REG] & device::cop0::COP0_STATUS_FR == 0
        && index % 2 == 0
}

pub(super) fn capture_state(device: &device::Device) -> CpuState {
    let mut fpr = [0u64; 32];
    for (index, value) in fpr.iter_mut().enumerate() {
        *value = if uses_fgr32(device, index) {
            u32::from_ne_bytes(device.cpu.cop1.fgr32[index]) as u64
                | (u32::from_ne_bytes(device.cpu.cop1.fgr32[index + 1]) as u64) << 32
        } else {
            u64::from_ne_bytes(device.cpu.cop1.fgr64[index])
        };
    }

    CpuState {
        gpr: device.cpu.gpr,
        hi: device.cpu.hi,
        lo: device.cpu.lo,
        pc: device.cpu.pc,
        fpr,
        fcr31: device.cpu.cop1.fcr31,
        status: device.cpu.cop0.regs[device::cop0::COP0_STATUS_REG] as u32,
        cause: device.cpu.cop0.regs[device::cop0::COP0_CAUSE_REG] as u32,
        count: (device.cpu.cop0.regs[device::cop0::COP0_COUNT_REG] >> 1) as u32,
    }
}

pub(super) fn apply_state(device: &mut device::Device, state: &CpuState) {
    device.cpu.gpr = state.gpr;
    device.cpu.gpr[0] = 0;
    device.cpu.hi = state.hi;
    device.cpu.lo = state.lo;
    device.cpu.cop1.fcr31 = state.fcr31;
    for (index, value) in state.fpr.iter().enumerate() {
        if uses_fgr32(device, index) {
            device.cpu.cop1.fgr32[index] = (*value as u32).to_ne_bytes();
            device.cpu.cop1.fgr32[index + 1] = ((*value >> 32) as u32).to_ne_bytes();
        } else {
            device.cpu.cop1.fgr64[index] = value.to_ne_bytes();
        }
    }

    if state.pc != device.cpu.pc {
        device.cpu.pc = state.pc;
        device.cpu.branch_state.state = device::cpu::State::Exception;
    }
}

pub(super) fn hypercall(device: &mut device::Device, opcode: u32, hypercall_id: u32) {
    if device.modloader.is_none() || device::cpu::in_delay_slot(device) {
        if let Some(context) = device.modloader.as_ref() {
            host_log(
                &context.callbacks,
                LOG_ERROR,
                "hypercall in a branch delay slot",
            );
        }
        return device::cop0::reserved(device, opcode);
    }

    let mut state = capture_state(device);
    let handled = with_host(device, |callbacks| match callbacks.hypercall {
        Some(handler) => unsafe {
            handler(
                callbacks.host,
                0,
                hypercall_id,
                &mut state as *mut CpuState as *mut c_void,
                size_of::<CpuState>() as u64,
            )
        },
        None => 0,
    });

    if handled.unwrap_or(0) == 0 {
        return device::cop0::reserved(device, opcode);
    }
    apply_state(device, &state);
}

// SPECIAL funct 0x28, ID in bits 6-25
pub fn hypercall_immediate(device: &mut device::Device, opcode: u32) {
    hypercall(device, opcode, immediate_hypercall_id(opcode));
}

// SPECIAL funct 0x29, ID in rs
pub fn hypercall_register(device: &mut device::Device, opcode: u32) {
    let hypercall_id = device.cpu.gpr[device::cpu_instructions::rs(opcode) as usize] as u32;
    hypercall(device, opcode, hypercall_id);
}
