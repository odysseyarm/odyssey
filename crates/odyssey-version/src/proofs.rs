use crate::{BootloaderRelease, UsbDeviceRelease, UsbVersionReport};

#[kani::proof]
fn usb_application_reports_follow_frozen_bit_layout() {
    let raw: u16 = kani::any();
    let report = UsbDeviceRelease::new_with_raw_value(raw).report();
    if raw == 0 {
        assert_eq!(report, UsbVersionReport::Unset);
    } else if raw >= 0xf000 {
        assert_eq!(report, UsbVersionReport::Unreadable);
    } else {
        let version = [raw / 4096, (raw / 64) % 64, raw % 64];
        assert_eq!(report, UsbVersionReport::Known(version));
        assert_eq!(UsbDeviceRelease::from_version(version).raw_value(), raw);
    }
}

#[kani::proof]
fn usb_bootloader_reports_and_update_order_follow_frozen_layout() {
    let raw: u16 = kani::any();
    let published: u16 = kani::any();
    kani::assume(published > 0 && published <= 4095);
    let report = BootloaderRelease::from_bcd_device(raw);
    let counter = raw % 4096;
    if raw >= 0xf000 && counter != 0 {
        assert_eq!(report, BootloaderRelease::Counter(counter));
        assert_eq!(report.superseded_by(published), counter < published);
    } else {
        assert_eq!(report, BootloaderRelease::Legacy);
        assert!(report.superseded_by(published));
    }
    assert_eq!(BootloaderRelease::bcd_device(published), 61440 + published);
}
