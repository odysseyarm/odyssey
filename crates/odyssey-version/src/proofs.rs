use crate::{firmware_bcd_device, legacy_bootloader_counter, legacy_bootloader_version};

#[kani::proof]
fn shipped_bootloader_counters_have_exact_reversible_identities() {
    let counter: u16 = kani::any();
    if let Some(version) = legacy_bootloader_version(counter) {
        assert!(counter >= 1 && counter <= 5);
        assert_eq!(version, [0, 0, u32::from(counter)]);
        assert_eq!(legacy_bootloader_counter(version), Some(counter));
    } else {
        assert!(counter == 0 || counter > 5);
    }
    let version: [u32; 3] = kani::any();
    if let Some(counter) = legacy_bootloader_counter(version) {
        assert_eq!(legacy_bootloader_version(counter), Some(version));
    } else {
        assert!(version[0] != 0 || version[1] != 0 || version[2] == 0 || version[2] > 5);
    }
}

#[kani::proof]
fn firmware_revision_is_decimal_major_without_reserved_15() {
    let major: u32 = kani::any();
    kani::assume(major <= 99);
    let raw = firmware_bcd_device(major);
    assert_eq!(raw & 0x00ff, 0);
    assert_eq!(u32::from(raw >> 12), major / 10);
    assert_eq!(u32::from((raw >> 8) & 0x0f), major % 10);
}
