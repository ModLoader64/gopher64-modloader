use super::*;
use hypercalls::{apply_state, capture_state};

#[derive(Clone, Default)]
pub struct Breakpoints {
    list: Vec<Breakpoint>,
    step: bool,
    mapped_execution: bool,
    watching: bool,
    executed: (u64, u64),
    accessed: (u64, u64),
    stepped_at: Option<u64>,
    traps: Vec<(u64, u64)>,
    original_handlers: Option<[fn(&mut device::Device, u32); 64]>,
}

// Opcode, transfer width, access kind
const ACCESSES: [(usize, u64, abi::ModLoader_Break_Access); 27] = [
    (26, 8, BREAK_READ),  // LDL
    (27, 8, BREAK_READ),  // LDR
    (32, 1, BREAK_READ),  // LB
    (33, 2, BREAK_READ),  // LH
    (34, 4, BREAK_READ),  // LWL
    (35, 4, BREAK_READ),  // LW
    (36, 1, BREAK_READ),  // LBU
    (37, 2, BREAK_READ),  // LHU
    (38, 4, BREAK_READ),  // LWR
    (39, 4, BREAK_READ),  // LWU
    (40, 1, BREAK_WRITE), // SB
    (41, 2, BREAK_WRITE), // SH
    (42, 4, BREAK_WRITE), // SWL
    (43, 4, BREAK_WRITE), // SW
    (44, 8, BREAK_WRITE), // SDL
    (45, 8, BREAK_WRITE), // SDR
    (46, 4, BREAK_WRITE), // SWR
    (48, 4, BREAK_READ),  // LL
    (49, 4, BREAK_READ),  // LWC1
    (52, 8, BREAK_READ),  // LLD
    (53, 8, BREAK_READ),  // LDC1
    (55, 8, BREAK_READ),  // LD
    (56, 4, BREAK_WRITE), // SC
    (57, 4, BREAK_WRITE), // SWC1
    (60, 8, BREAK_WRITE), // SCD
    (61, 8, BREAK_WRITE), // SDC1
    (63, 8, BREAK_WRITE), // SD
];

const ACCESS_BY_OPCODE: [Option<(u64, abi::ModLoader_Break_Access)>; 64] = {
    let mut table = [None; 64];
    let mut index = 0;
    while index < ACCESSES.len() {
        let (opcode, size, access) = ACCESSES[index];
        table[opcode] = Some((size, access));
        index += 1;
    }
    table
};

fn normalize(address: u64) -> u64 {
    let address = address & 0xFFFFFFFF;
    if address & 0xC0000000 == 0x80000000 {
        address & !0x20000000
    } else {
        address
    }
}

fn find(
    breakpoints: &Breakpoints,
    address: u64,
    size: u64,
    access: abi::ModLoader_Break_Access,
) -> Option<u32> {
    let start = normalize(address);
    let end = start + size;
    let (low, high) = if access == BREAK_EXECUTE {
        breakpoints.executed
    } else {
        breakpoints.accessed
    };

    if start >= high || end <= low {
        return None;
    }

    breakpoints
        .list
        .iter()
        .find(|breakpoint| {
            breakpoint.access & access as u32 != 0
                && start < breakpoint.end
                && breakpoint.start < end
        })
        .map(|breakpoint| breakpoint.id)
}

fn in_kseg(address: u64) -> bool {
    address & 0xE0000000 == 0x80000000
}

fn merge(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn meets(ranges: &[(u64, u64)], start: u64, end: u64) -> bool {
    let index = ranges.partition_point(|range| range.1 <= start);
    index < ranges.len() && ranges[index].0 < end
}

fn span(list: &[Breakpoint], access: abi::ModLoader_Break_Access) -> (u64, u64) {
    list.iter()
        .filter(|breakpoint| breakpoint.access & access as u32 != 0)
        .fold((u64::MAX, 0), |(low, high), breakpoint| {
            (low.min(breakpoint.start), high.max(breakpoint.end))
        })
}

pub(super) extern "C" fn set_breakpoints(
    adapter: *mut c_void,
    breakpoints: *const Breakpoint,
    count: u32,
    step_processor: u32,
) -> i32 {
    if adapter.is_null()
        || (breakpoints.is_null() && count != 0)
        || (step_processor != u32::MAX && step_processor != 0)
    {
        return -1;
    }

    let adapter = adapter as *mut Adapter;
    let list = if count == 0 {
        Vec::new()
    } else {
        let breakpoints = unsafe { std::slice::from_raw_parts(breakpoints, count as usize) };
        if breakpoints
            .iter()
            .any(|breakpoint| breakpoint.processor != 0)
        {
            return -1;
        }
        breakpoints
            .iter()
            .map(|breakpoint| {
                let start = normalize(breakpoint.start);
                Breakpoint {
                    start,
                    end: start + breakpoint.end.saturating_sub(breakpoint.start),
                    ..*breakpoint
                }
            })
            .collect()
    };

    let shared = unsafe { &(*adapter).shared };
    if let Ok(mut pending) = shared.pending_breakpoints.lock() {
        *pending = Some((list, step_processor != u32::MAX));
    }

    if shared.current_device.load(Ordering::Acquire).is_null() {
        apply_breakpoints(unsafe { &mut (*adapter).device });
    }
    0
}

pub(super) fn apply_breakpoints(device: &mut device::Device) {
    let Some(context) = device.modloader.as_mut() else {
        return;
    };
    let Some((list, step)) = context
        .shared
        .pending_breakpoints
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
    else {
        return;
    };

    let watching = list
        .iter()
        .any(|breakpoint| breakpoint.access & (BREAK_READ | BREAK_WRITE) as u32 != 0);
    let remap = watching != context.breakpoints.watching;
    let traps = merge(
        list.iter()
            .filter(|breakpoint| {
                breakpoint.access & BREAK_EXECUTE as u32 != 0 && in_kseg(breakpoint.start)
            })
            .map(|breakpoint| (breakpoint.start, breakpoint.end))
            .collect(),
    );
    let retrap = !traps.is_empty() || !context.breakpoints.traps.is_empty() || remap;
    context.breakpoints.mapped_execution = list.iter().any(|breakpoint| {
        breakpoint.access & BREAK_EXECUTE as u32 != 0 && !in_kseg(breakpoint.start)
    });
    context.breakpoints.step = step;
    context.breakpoints.watching = watching;
    context.breakpoints.executed = span(&list, BREAK_EXECUTE);
    context.breakpoints.accessed = span(&list, BREAK_READ | BREAK_WRITE);
    context.breakpoints.traps = traps;
    context.breakpoints.list = list;

    if remap {
        device::cpu::map_instructions(device);
    }

    if retrap {
        decode_icache(device);
    }
}

pub(super) fn decode_icache(device: &mut device::Device) {
    for line in 0..device.memory.icache.len() {
        if !device.memory.icache[line].valid {
            continue;
        }

        for word in 0..8 {
            let decoded =
                device::cpu::decode_opcode(device, device.memory.icache[line].words[word]);
            device.memory.icache[line].instruction[word] = decoded;
        }

        trap_line(device, line);
    }
}

pub fn trap_line(device: &mut device::Device, line_index: usize) {
    let Some(context) = device.modloader.as_ref() else {
        return;
    };
    if context.breakpoints.traps.is_empty() {
        return;
    }

    let line = &mut device.memory.icache[line_index];
    let start = 0x80000000 | ((line.tag | line.index as u32) as u64 & 0x1FFFFFE0);
    if !meets(&context.breakpoints.traps, start, start + 32) {
        return;
    }

    for word in 0..8 {
        let address = start + word as u64 * 4;
        if meets(&context.breakpoints.traps, address, address + 4) {
            line.instruction[word] = break_trap;
        }
    }
}

fn break_trap(device: &mut device::Device, opcode: u32) {
    if stop_at_pc(device) {
        return;
    }
    device::cpu::decode_opcode(device, opcode)(device, opcode)
}

pub fn before_uncached_instruction(device: &mut device::Device) -> bool {
    if !device
        .modloader
        .as_ref()
        .is_some_and(|context| !context.breakpoints.traps.is_empty())
    {
        return false;
    }

    if stop_at_pc(device) {
        device.cpu.branch_state.state = device::cpu::State::Step;
        return true;
    }
    false
}

fn trapped_breakpoint(breakpoints: &mut Breakpoints, pc: u64) -> Option<u32> {
    if !in_kseg(normalize(pc)) || breakpoints.stepped_at.take() == Some(pc) {
        return None;
    }
    find(breakpoints, pc, 4, BREAK_EXECUTE)
}

fn stop_at_pc(device: &mut device::Device) -> bool {
    let pc = device.cpu.pc;
    let Some(context) = device.modloader.as_mut() else {
        return false;
    };
    match trapped_breakpoint(&mut context.breakpoints, pc) {
        Some(id) => stop(device, id, BREAK_EXECUTE, pc, 4, 0),
        None => false,
    }
}

pub(super) fn watch_loads_and_stores(device: &mut device::Device) {
    let Some(context) = device.modloader.as_mut() else {
        return;
    };
    if context.breakpoints.watching {
        context.breakpoints.original_handlers = Some(device.cpu.instrs);
        for (opcode, _, _) in ACCESSES {
            device.cpu.instrs[opcode] = watch_access;
        }
    } else {
        context.breakpoints.original_handlers = None;
    }
}

#[inline(always)]
pub fn before_instruction(device: &mut device::Device) {
    let Some(context) = device.modloader.as_ref() else {
        return;
    };
    if context.shared.quit_requested.load(Ordering::Acquire) {
        device.cpu.running = false;
        return;
    }
    if context.breakpoints.step || context.breakpoints.mapped_execution {
        check_instruction(device);
    }
}

fn check_instruction(device: &mut device::Device) {
    let pc = device.cpu.pc;
    let Some(context) = device.modloader.as_mut() else {
        return;
    };
    let id = if context.breakpoints.step {
        context.breakpoints.step = false;
        context.breakpoints.stepped_at = Some(pc);
        Some(0)
    } else if in_kseg(normalize(pc)) {
        None
    } else {
        find(&context.breakpoints, pc, 4, BREAK_EXECUTE)
    };

    if let Some(id) = id
        && stop(device, id, BREAK_EXECUTE, pc, 4, 0)
    {
        device.cpu.branch_state.state = device::cpu::State::Step;
        if let Some(context) = device.modloader.as_mut() {
            context.breakpoints.stepped_at = None;
        }
    }
}

fn access_range(
    opcode: usize,
    base: u64,
    size: u64,
    value: u64,
    linked: bool,
) -> Option<(u64, u64, u64)> {
    if matches!(opcode, 56 | 60) && !linked {
        return None;
    }
    let offset = base & (size - 1);
    let (address, count, value) = match opcode {
        26 | 34 | 42 | 44 => (base, size - offset, value >> (offset * 8)),
        27 | 38 | 45 | 46 => (base - offset, offset + 1, value),
        _ => (base, size, value),
    };
    let value = if count == 8 {
        value
    } else {
        value & ((1u64 << (count * 8)) - 1)
    };
    Some((address, count, value))
}

fn watch_access(device: &mut device::Device, opcode: u32) {
    let instruction = (opcode >> 26) as usize;
    let Some((width, access)) = ACCESS_BY_OPCODE[instruction] else {
        return device::cop0::reserved(device, opcode);
    };
    let Some(run) = device
        .modloader
        .as_ref()
        .and_then(|context| context.breakpoints.original_handlers.as_ref())
        .map(|handlers| handlers[instruction])
    else {
        return device::cop0::reserved(device, opcode);
    };
    let base = device.cpu.gpr[device::cpu_instructions::rs(opcode) as usize].wrapping_add(
        device::cpu_instructions::se16(device::cpu_instructions::imm(opcode) as i16),
    );
    let value = if access == BREAK_WRITE {
        stored_value(device, opcode, width)
    } else {
        0
    };
    if let Some((address, size, value)) =
        access_range(instruction, base, width, value, device.cpu.llbit)
    {
        let hit = device
            .modloader
            .as_ref()
            .and_then(|context| find(&context.breakpoints, address, size, access));
        if let Some(id) = hit
            && stop(device, id, access, address, size, value)
        {
            return;
        }
    }
    run(device, opcode)
}

fn stored_value(device: &device::Device, opcode: u32, size: u64) -> u64 {
    let register = device::cpu_instructions::rt(opcode) as usize;
    match opcode >> 26 {
        57 => device::cop1::get_fpr_single(device, register).to_bits() as u64,
        61 => device::cop1::get_fpr_double(device, register).to_bits(),
        _ if size == 8 => device.cpu.gpr[register],
        _ => device.cpu.gpr[register] & ((1u64 << (size * 8)) - 1),
    }
}

fn stop(
    device: &mut device::Device,
    id: u32,
    access: abi::ModLoader_Break_Access,
    address: u64,
    size: u64,
    value: u64,
) -> bool {
    let hit = Break {
        id,
        access: access as u32,
        address: address & 0xFFFFFFFF,
        value,
        size: size as u32,
        processor: 0,
    };
    let mut state = capture_state(device);
    let pc = state.pc;
    with_host(device, |callbacks| {
        if let Some(handler) = callbacks.breakpoint {
            unsafe {
                handler(
                    callbacks.host,
                    &hit,
                    &mut state as *mut CpuState as *mut c_void,
                    size_of::<CpuState>() as u64,
                )
            };
        }
    });
    apply_state(device, &state);
    state.pc != pc
}
