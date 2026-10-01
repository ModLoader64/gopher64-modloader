use super::*;

const UNFLOADER_TEXT: u32 = 1;
const MAX_DEBUG_LINE: usize = 4096;

fn append_output(line: &mut String, text: &str, mut emit: impl FnMut(&str)) {
    for part in text.split_inclusive('\n') {
        let newline = part.ends_with('\n');
        let mut remaining = part.strip_suffix('\n').unwrap_or(part);
        while !remaining.is_empty() {
            let count = remaining.floor_char_boundary(MAX_DEBUG_LINE - line.len());
            if count == 0 {
                emit(line.trim_end_matches('\r'));
                line.clear();
                continue;
            }
            line.push_str(&remaining[..count]);
            remaining = &remaining[count..];
        }
        if newline {
            emit(line.trim_end_matches('\r'));
            line.clear();
        }
    }
}

pub fn debug_output(text: String, device: &mut device::Device) {
    let Some(context) = device.modloader.as_mut() else {
        return;
    };
    if context.debug_output {
        let callbacks = context.callbacks;
        append_output(&mut context.debug_line, &text, |line| {
            host_log(&callbacks, LOG_INFO, line);
        });
    }
}

pub(super) fn flush_output(device: &mut device::Device) {
    if let Some(context) = device.modloader.as_mut()
        && !context.debug_line.is_empty()
    {
        host_log(
            &context.callbacks,
            LOG_INFO,
            context.debug_line.trim_end_matches('\r'),
        );
        context.debug_line.clear();
    }
}

pub(super) fn clear_output(device: &mut device::Device) {
    if let Some(context) = device.modloader.as_mut() {
        context.debug_line.clear();
    }
}

pub(super) fn set_enabled(device: &mut device::Device, enabled: bool) {
    if !enabled {
        flush_output(device);
    }
    if let Some(context) = device.modloader.as_mut() {
        context.debug_output = enabled;
    }
}

pub(super) fn drain_usb(device: &mut device::Device) {
    let receiver = device
        .modloader
        .as_ref()
        .and_then(|context| context.usb_rx.clone());
    let Some(receiver) = receiver else {
        return;
    };
    loop {
        let packet = receiver
            .lock()
            .ok()
            .and_then(|mut receiver| receiver.try_recv().ok());
        let Some(packet) = packet else {
            break;
        };
        if packet.data_type == UNFLOADER_TEXT {
            debug_output(String::from_utf8_lossy(&packet.data).into_owned(), device);
        }
    }
}
