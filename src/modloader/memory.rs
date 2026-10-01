use super::*;

pub(super) const RAM_ALIAS_BASE: u64 = 0x20000000;
pub(super) const RAM_EXTENSION_START: u64 = 0x03F00000;
const RAM_EXTENSION_END: u64 = device::rdram::RDRAM_MASK as u64 + 1;

pub fn translate_ram_extension(address: u64) -> Option<(u64, bool, bool)> {
    let physical = address & 0x1FFFFFFF;
    (address & 0x20000000 == 0 && (RAM_EXTENSION_START..RAM_EXTENSION_END).contains(&physical))
        .then_some((RAM_ALIAS_BASE | physical, true, false))
}

pub fn map_ram_alias(device: &mut device::Device) {
    for page in (RAM_ALIAS_BASE >> 16) as usize..device::memory::MEMORY_MAP_PAGES {
        device.memory.fast_read[page] = device::rdram::read_mem_fast;
        device.memory.memory_map_read[page] = device::rdram::read_mem;
        device.memory.memory_map_write[page] = device::rdram::write_mem;
    }
}

pub(super) extern "C" fn invalidate_code(
    adapter: *mut c_void,
    space_index: u32,
    address: u64,
    size: u64,
) -> i32 {
    if adapter.is_null() || space_index != SPACE_RDRAM {
        return -1;
    }

    let adapter = adapter as *mut Adapter;
    let shared = unsafe { &(*adapter).shared };
    if let Ok(mut pending) = shared.pending_invalidations.lock() {
        pending.push((address, size));
    }

    if shared.current_device.load(Ordering::Acquire).is_null() {
        apply_invalidations(unsafe { &mut (*adapter).device });
    }
    0
}

pub(super) extern "C" fn translate_address(
    adapter: *mut c_void,
    processor: u32,
    address: u64,
    out_space_index: *mut u32,
    out_offset: *mut u64,
) -> i32 {
    if adapter.is_null() || processor != 0 || out_space_index.is_null() || out_offset.is_null() {
        return -1;
    }

    let adapter = unsafe { &*(adapter as *const Adapter) };
    let device = unsafe {
        &*adapter
            .shared
            .device_pointer((&raw const *adapter.device).cast_mut())
    };

    let Some(physical) = physical_address(device, address) else {
        return -1;
    };
    let rom_start = device::memory::MM_CART_ROM as u64;
    let sp_start: u64 = 0x04000000;
    let (space_index, offset) = if let Some(offset) = ram_offset(device, physical) {
        (SPACE_RDRAM, offset)
    } else if physical >= rom_start && physical - rom_start < device.cart.rom.len() as u64 {
        (SPACE_ROM, physical - rom_start)
    } else if physical >= sp_start && physical - sp_start < device.rsp.mem.len() as u64 {
        (SPACE_SP_MEM, physical - sp_start)
    } else {
        return -1;
    };

    unsafe {
        *out_space_index = space_index;
        *out_offset = offset;
    }
    0
}

pub(super) fn apply_invalidations(device: &mut device::Device) {
    let ranges = match device.modloader.as_ref() {
        Some(context) => match context.shared.pending_invalidations.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(_) => return,
        },
        None => return,
    };

    for (address, size) in ranges {
        let start = address & !0x1F;
        let end = address.saturating_add(size);
        for line in device.memory.icache.iter_mut() {
            let line_address = ((line.tag | line.index as u32) as u64) & !RAM_ALIAS_BASE;
            if line.valid && line_address >= start && line_address < end {
                line.valid = false;
            }
        }
    }
}

pub(super) const DCACHE_LINES: usize = 512;

pub(super) struct DcacheMemory {
    held: [[u32; 4]; DCACHE_LINES],
    shown: [[u32; 4]; DCACHE_LINES],
}

impl Default for DcacheMemory {
    fn default() -> Self {
        Self {
            held: [[0; 4]; DCACHE_LINES],
            shown: [[0; 4]; DCACHE_LINES],
        }
    }
}

pub(super) fn dcache_line_ram(device: &device::Device, line_index: usize) -> Option<usize> {
    let line = &device.memory.dcache[line_index];
    if !line.valid {
        return None;
    }

    let address = ((line.tag | line.index as u32) & device::cache::TAG_MASK) as u64;
    let offset = ram_offset(device, address)? as usize;
    (offset + 16 <= device.rdram.mem.len()).then_some(offset)
}

pub(super) fn ram_word(device: &device::Device, offset: usize) -> u32 {
    u32::from_ne_bytes(device.rdram.mem[offset..offset + 4].try_into().unwrap())
}

pub(super) fn set_ram_word(device: &mut device::Device, offset: usize, value: u32) {
    device.rdram.mem[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

pub(super) fn dcache_before_host(device: &mut device::Device, memory: &mut DcacheMemory) {
    for line_index in 0..DCACHE_LINES {
        let Some(offset) = dcache_line_ram(device, line_index) else {
            continue;
        };
        let dirty = device.memory.dcache[line_index].dirty;
        if dirty {
            ui::video::check_framebuffers(offset as u32, 16);
        }

        for word in 0..4 {
            memory.held[line_index][word] = ram_word(device, offset + word * 4);
            if dirty {
                set_ram_word(
                    device,
                    offset + word * 4,
                    device.memory.dcache[line_index].words[word],
                );
            }
            memory.shown[line_index][word] = ram_word(device, offset + word * 4);
        }
    }
}

pub(super) fn dcache_after_host(device: &mut device::Device, memory: &DcacheMemory) {
    for line_index in 0..DCACHE_LINES {
        let Some(offset) = dcache_line_ram(device, line_index) else {
            continue;
        };

        for word in 0..4 {
            let value = ram_word(device, offset + word * 4);
            if value != memory.shown[line_index][word] {
                device.memory.dcache[line_index].words[word] = value;
            } else if value != memory.held[line_index][word] {
                set_ram_word(device, offset + word * 4, memory.held[line_index][word]);
            }
        }
    }
}

pub(super) fn with_host<T>(
    device: &mut device::Device,
    call: impl FnOnce(&HostCallbacks) -> T,
) -> Option<T> {
    let context = device.modloader.as_ref()?;
    let callbacks = context.callbacks;
    let shared = context.shared.clone();

    if !shared.current_device.load(Ordering::Acquire).is_null() {
        return Some(call(&callbacks));
    }
    let mut memory = shared
        .scratch_memory
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .unwrap_or_default();
    dcache_before_host(device, &mut memory);
    let device_pointer: *mut device::Device = device;
    shared
        .current_device
        .store(device_pointer, Ordering::Release);
    shared.current_memory.store(&mut *memory, Ordering::Release);
    let access = HostAccess {
        shared,
        device: device_pointer,
        memory: Some(memory),
    };
    let result = call(&callbacks);
    drop(access);
    Some(result)
}

struct HostAccess {
    shared: Arc<SharedState>,
    device: *mut device::Device,
    memory: Option<Box<DcacheMemory>>,
}

impl Drop for HostAccess {
    fn drop(&mut self) {
        self.shared
            .current_memory
            .store(std::ptr::null_mut(), Ordering::Release);
        self.shared
            .current_device
            .store(std::ptr::null_mut(), Ordering::Release);
        let device = unsafe { &mut *self.device };
        let memory = self.memory.take().unwrap();
        dcache_after_host(device, &memory);
        *self
            .shared
            .scratch_memory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(memory);
        apply_invalidations(device);
        apply_breakpoints(device);
    }
}

fn physical_address(device: &device::Device, mut address: u64) -> Option<u64> {
    if address & 0xC0000000 != 0x80000000 && address & 0xFFFFFFFF00000000 == 0x9000000000000000 {
        address |= 0xA0000000;
    }
    let address = address & 0xFFFFFFFF;
    match address & 0xE0000000 {
        0x80000000 => Some(
            translate_ram_extension(address)
                .map_or(address & 0x1FFFFFFF, |(physical, _, _)| physical),
        ),
        0xA0000000 => Some(address & 0x1FFFFFFF),
        _ => device
            .cpu
            .cop0
            .tlb_lut_r
            .get(&(address >> 12))
            .map(|entry| (entry.address & 0x1FFFF000) | (address & 0xFFF)),
    }
}

fn ram_offset(device: &device::Device, physical: u64) -> Option<u64> {
    let offset = if physical & RAM_ALIAS_BASE != 0 {
        physical & !RAM_ALIAS_BASE
    } else if physical < RAM_EXTENSION_START {
        physical
    } else {
        return None;
    };
    (offset < device.rdram.mem.len() as u64).then_some(offset)
}

fn register(registers: &[u32], physical: u64) -> Option<u32> {
    registers.get(((physical & 0xFFFF) >> 2) as usize).copied()
}

fn big_endian_word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn peek_word(device: &device::Device, physical: u64) -> Option<u32> {
    let word = physical & !3;
    let rom = device::memory::MM_CART_ROM as u64;
    let pif = device::memory::MM_PIF_MEM as u64;
    let access = device::memory::AccessSize::Word;

    if let Some(offset) = ram_offset(device, word) {
        return Some(device::rdram::read_mem_fast(device, offset, access));
    }

    match word >> 16 {
        0x03F0..=0x03FF => device.rdram.regs[((word >> 13) & 3) as usize]
            .get(((word & 0x3FF) >> 2) as usize)
            .copied(),
        0x0400..=0x0403 => big_endian_word(&device.rsp.mem, (word & 0x1FFF) as usize),
        0x0404 => register(&device.rsp.regs, word),
        0x0408 => register(&device.rsp.regs2, word),
        0x0410 => register(&device.rdp.regs_dpc, word),
        0x0420 => register(&device.rdp.regs_dps, word),
        0x0430 => register(&device.mi.regs, word),
        0x0440 => register(&device.vi.regs, word),
        0x0450 => register(&device.ai.regs, word),
        0x0460 => register(&device.pi.regs, word),
        0x0470 => register(&device.ri.regs, word),
        0x0480 => register(&device.si.regs, word),
        0x0800..=0x0801 => {
            big_endian_word(&device.ui.storage.saves.sram.data, (word & 0xFFFF) as usize)
        }
        _ if word >= rom && word - rom < device.cart.rom.len() as u64 => {
            Some(device::cart::rom::read_mem_fast(device, word, access))
        }
        _ if word >= pif && word - pif < 0x7C0 => {
            big_endian_word(&device.pif.rom, (word - pif) as usize)
        }
        _ if word >= pif + 0x7C0 && word - pif < 0x800 => {
            big_endian_word(&device.pif.ram, (word - pif - 0x7C0) as usize)
        }
        _ => None,
    }
}

fn write_big_endian_word(bytes: &mut [u8], offset: usize, value: u32, mask: u32) -> bool {
    let Some(bytes) = bytes.get_mut(offset..offset + 4) else {
        return false;
    };
    let mut word = u32::from_be_bytes(bytes.try_into().unwrap());
    device::memory::masked_write_32(&mut word, value, mask);
    bytes.copy_from_slice(&word.to_be_bytes());
    true
}

fn poke_word(device: &mut device::Device, physical: u64, value: u32, mask: u32) -> bool {
    let rom = device::memory::MM_CART_ROM as u64;
    let pif = device::memory::MM_PIF_MEM as u64;

    if let Some(offset) = ram_offset(device, physical) {
        let offset = offset as usize;
        if offset + 4 > device.rdram.mem.len() {
            return false;
        }
        let mut word = ram_word(device, offset);
        device::memory::masked_write_32(&mut word, value, mask);
        set_ram_word(device, offset, word);
        return true;
    }

    match physical >> 16 {
        0x0400..=0x0403 => {
            let mut word = device::rsp_interface::read_mem_fast(
                device,
                physical,
                device::memory::AccessSize::Word,
            );
            device::memory::masked_write_32(&mut word, value, mask);
            device::rsp_interface::write_mem(device, physical, word, u32::MAX);
            return true;
        }
        0x0800..=0x0801 => {
            return write_big_endian_word(
                &mut device.ui.storage.saves.sram.data,
                (physical & 0xFFFF) as usize,
                value,
                mask,
            );
        }
        _ if physical >= rom && physical - rom < device.cart.rom.len() as u64 => {
            return write_big_endian_word(
                &mut device.cart.rom,
                (physical - rom) as usize,
                value,
                mask,
            );
        }
        _ if physical >= pif && physical - pif < 0x7C0 => {
            return write_big_endian_word(
                &mut device.pif.rom,
                (physical - pif) as usize,
                value,
                mask,
            );
        }
        _ if physical >= pif + 0x7C0 && physical - pif < 0x800 => {
            return write_big_endian_word(
                &mut device.pif.ram,
                (physical - pif - 0x7C0) as usize,
                value,
                mask,
            );
        }
        _ => {}
    }

    if peek_word(device, physical).is_none() {
        return false;
    }

    let write = device.memory.memory_map_write[(physical >> 16) as usize];
    write(device, physical, value, mask);
    true
}

pub(super) extern "C" fn peek_memory(
    adapter: *mut c_void,
    processor: u32,
    address: u64,
    buffer: *mut u8,
    size: u64,
) -> u64 {
    if adapter.is_null() || processor != 0 || buffer.is_null() || size > isize::MAX as u64 {
        return 0;
    }
    let adapter = unsafe { &*(adapter as *const Adapter) };
    let device = unsafe {
        &*adapter
            .shared
            .device_pointer((&raw const *adapter.device).cast_mut())
    };
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer, size as usize) };
    peek_bytes(device, address, buffer) as u64
}

fn peek_bytes(device: &device::Device, address: u64, buffer: &mut [u8]) -> usize {
    let mut done = 0;
    while done < buffer.len() {
        let Some(physical) = address
            .checked_add(done as u64)
            .and_then(|address| physical_address(device, address))
        else {
            break;
        };
        let Some(word) = peek_word(device, physical) else {
            break;
        };
        let start = (physical & 3) as usize;
        let count = (4 - start).min(buffer.len() - done);
        for index in 0..count {
            buffer[done + index] = (word >> (24 - 8 * (start + index))) as u8;
        }
        done += count;
    }
    done
}

pub(super) extern "C" fn poke_memory(
    adapter: *mut c_void,
    processor: u32,
    address: u64,
    data: *const u8,
    size: u64,
) -> u64 {
    if adapter.is_null() || processor != 0 || data.is_null() || size > isize::MAX as u64 {
        return 0;
    }
    let adapter = unsafe { &mut *(adapter as *mut Adapter) };
    let device = unsafe { &mut *adapter.shared.device_pointer(&raw mut *adapter.device) };
    let data = unsafe { std::slice::from_raw_parts(data, size as usize) };
    poke_bytes(device, address, data) as u64
}

fn poke_bytes(device: &mut device::Device, address: u64, data: &[u8]) -> usize {
    let mut done = 0;
    let mut sram_written = false;
    while done < data.len() {
        let Some(physical) = address
            .checked_add(done as u64)
            .and_then(|address| physical_address(device, address))
        else {
            break;
        };
        let start = (physical & 3) as usize;
        let count = (4 - start).min(data.len() - done);
        let mut value = 0;
        let mut mask = 0;
        for index in 0..count {
            let shift = 24 - 8 * (start + index);
            value |= (data[done + index] as u32) << shift;
            mask |= 0xFF << shift;
        }
        if !poke_word(device, physical & !3, value, mask) {
            break;
        }
        sram_written |= matches!(physical >> 16, 0x0800..=0x0801);
        done += count;
    }
    if sram_written {
        ui::storage::schedule_save(device, ui::storage::SaveTypes::Sram);
    }
    done
}
