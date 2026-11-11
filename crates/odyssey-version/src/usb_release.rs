/// A version as published in the USB `bcdDevice` field.
///
/// Firmware and bootloaders put their version here so a host learns it from
/// enumeration alone, without opening the device or speaking the control
/// protocol.
///
/// The USB spec defines this field as binary-coded decimal with a single
/// sub-minor digit, which cannot represent a patch above 9 — ours are already
/// past that. The fields below are a private layout instead, so `lsusb` will
/// print something that is not the version. The widths are types, so a
/// component that cannot fit fails to compile at the call site.
#[bitbybit::bitfield(u16)]
#[derive(Debug, PartialEq, Eq)]
pub struct UsbDeviceRelease {
    #[bits(12..=15, rw)]
    pub major: arbitrary_int::u4,
    #[bits(6..=11, rw)]
    pub minor: arbitrary_int::u6,
    #[bits(0..=5, rw)]
    pub patch: arbitrary_int::u6,
}

impl UsbDeviceRelease {
    /// `major` 15 is reserved: it does not mean version 15, it means the
    /// remaining twelve bits use a layout this build does not know.
    ///
    /// Nothing is free to add later once devices ship, because a host cannot
    /// ask a device how to read a field it already published. Holding one major
    /// value back leaves room to switch to, say, a monotonic build counter
    /// while keeping the two encodings distinguishable. A host that sees it
    /// must treat the version as unknown rather than guessing.
    pub const RESERVED_MAJOR: u16 = 15;

    /// Pack a version. Out-of-range components fail to build, at compile time
    /// when called from a `const`.
    pub const fn from_version(version: [u16; 3]) -> Self {
        let [major, minor, patch] = version;
        assert!(
            major < Self::RESERVED_MAJOR,
            "major 15 is reserved for a future layout"
        );
        assert!(minor <= 63, "minor must fit the six-bit version field");
        assert!(patch <= 63, "patch must fit the six-bit version field");
        Self::builder()
            .with_major(arbitrary_int::u4::new(major as u8))
            .with_minor(arbitrary_int::u6::new(minor as u8))
            .with_patch(arbitrary_int::u6::new(patch as u8))
            .build()
    }

    /// What the field says about the version.
    pub const fn report(self) -> UsbVersionReport {
        if self.raw_value() == 0 {
            return UsbVersionReport::Unset;
        }
        if self.major().value() as u16 == Self::RESERVED_MAJOR {
            return UsbVersionReport::Unreadable;
        }
        UsbVersionReport::Known([
            self.major().value() as u16,
            self.minor().value() as u16,
            self.patch().value() as u16,
        ])
    }
}

/// The three things `bcdDevice` can tell a host, which must not be conflated.
///
/// `Unset` and `Unreadable` both mean "no version", but they call for opposite
/// behaviour: the first is a device older than version reporting, which an
/// update should replace; the second is a device newer than this build, which
/// an update must leave alone or it would install something older.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbVersionReport {
    /// The field was never set. The device predates version reporting.
    Unset,
    /// The field uses a layout this build does not know, so it was written by
    /// something newer.
    Unreadable,
    Known([u16; 3]),
}

/// A bootloader's release, published in its own `bcdDevice`.
///
/// Bootloaders are not versioned like applications. A host only asks "is the
/// published image newer than the one running", which a monotonic counter
/// answers without the semver ordering rules. The counter lives in the
/// application layout's reserved major (`0xF000 | n`), so a value without that
/// marker is unambiguously one written before counters existed.
///
/// That matters because shipped bootloaders never set the field and publish the
/// USB stack's default, `0x0010`. Read as a bare number that is release 16 and
/// would suppress every update below it; as `Legacy` it always updates.
///
/// Only meaningful for a device enumerating as a bootloader. The same bits in
/// an application mean a layout this build does not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootloaderRelease {
    /// Published before counters existed.
    Legacy,
    /// Monotonic, from 1.
    Counter(u16),
}

impl BootloaderRelease {
    /// Largest counter the twelve payload bits hold.
    pub const MAX_COUNTER: u16 = 0x0FFF;

    /// The `bcdDevice` value a bootloader publishes for `counter`. Out-of-range
    /// counters fail to build, at compile time when called from a `const`.
    pub const fn bcd_device(counter: u16) -> u16 {
        assert!(
            counter >= 1 && counter <= Self::MAX_COUNTER,
            "bootloader release counter must be 1..=4095"
        );
        (UsbDeviceRelease::RESERVED_MAJOR << 12) | counter
    }

    pub const fn from_bcd_device(raw: u16) -> Self {
        let counter = raw & Self::MAX_COUNTER;
        if raw >> 12 == UsbDeviceRelease::RESERVED_MAJOR && counter != 0 {
            Self::Counter(counter)
        } else {
            Self::Legacy
        }
    }

    /// Whether a bootloader running `self` should be replaced by `published`.
    /// A legacy bootloader is replaced by any counter.
    pub const fn superseded_by(self, published: u16) -> bool {
        match self {
            Self::Legacy => true,
            Self::Counter(running) => running < published,
        }
    }
}

#[cfg(test)]
mod bootloader_release_tests {
    use super::*;
    #[test]
    fn application_version_fields_round_trip_at_their_limits() {
        for version in [[0, 0, 1], [14, 63, 63]] {
            assert_eq!(
                UsbDeviceRelease::from_version(version).report(),
                UsbVersionReport::Known(version)
            );
        }
    }

    #[test]
    #[should_panic(expected = "minor must fit")]
    fn application_minor_cannot_wrap_before_validation() {
        let _ = UsbDeviceRelease::from_version([1, 256, 0]);
    }

    #[test]
    #[should_panic(expected = "patch must fit")]
    fn application_patch_cannot_wrap_before_validation() {
        let _ = UsbDeviceRelease::from_version([1, 0, 256]);
    }

    #[test]
    fn shipped_default_is_legacy_not_release_16() {
        let shipped = 0x0010;
        assert_eq!(
            BootloaderRelease::from_bcd_device(shipped),
            BootloaderRelease::Legacy
        );
        assert!(BootloaderRelease::from_bcd_device(shipped).superseded_by(1));
    }

    #[test]
    fn counter_round_trips_and_orders() {
        for n in [1, 2, 16, 4095] {
            let raw = BootloaderRelease::bcd_device(n);
            assert_eq!(
                BootloaderRelease::from_bcd_device(raw),
                BootloaderRelease::Counter(n)
            );
        }
        assert!(!BootloaderRelease::Counter(2).superseded_by(2));
        assert!(!BootloaderRelease::Counter(3).superseded_by(2));
        assert!(BootloaderRelease::Counter(2).superseded_by(3));
    }

    #[test]
    fn marker_without_payload_and_unset_are_legacy() {
        assert_eq!(
            BootloaderRelease::from_bcd_device(0xF000),
            BootloaderRelease::Legacy
        );
        assert_eq!(
            BootloaderRelease::from_bcd_device(0),
            BootloaderRelease::Legacy
        );
    }
}
