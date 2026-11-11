/// Encode firmware major version as vendor-defined `bcdDevice` revision `major.00`.
///
/// Keeping patch and minor releases out of this field avoids changing USB device
/// revision identity for every software release. Full `SemVer` belongs in a string
/// descriptor. USB BCD permits two decimal digits per byte.
///
/// Since: Odyssey Desktop (unreleased).
/// # Panics
/// Panics when `major` exceeds 99.
///
pub const fn firmware_bcd_device(major: u32) -> u16 {
    assert!(major <= 99, "firmware major must fit two BCD digits");
    (((major / 10) as u16) << 12) | (((major % 10) as u16) << 8)
}

#[cfg(test)]
mod tests {
    use super::firmware_bcd_device;

    #[test]
    fn encodes_major_as_bcd_and_ignores_minor_patch() {
        assert_eq!(firmware_bcd_device(0), 0x0000);
        assert_eq!(firmware_bcd_device(1), 0x0100);
        assert_eq!(firmware_bcd_device(10), 0x1000);
        assert_eq!(firmware_bcd_device(15), 0x1500);
        assert_eq!(firmware_bcd_device(99), 0x9900);
    }

    #[test]
    #[should_panic(expected = "firmware major must fit two BCD digits")]
    fn refuses_unrepresentable_major() {
        firmware_bcd_device(100);
    }
}
