pub use crate::ui_common::UsbData;

pub fn send_to_usb(usb_tx: &tokio::sync::mpsc::UnboundedSender<UsbData>, buffer: UsbData) {
    let _ = usb_tx.send(buffer);
}
