//! Accommodations for devices built before the current bootloader.
//!
//! Everything here exists only to get the devices already in the field onto a
//! bootloader that identifies itself. Once every device has taken an update —
//! and the fleet is small enough to know that for certain — delete this module
//! and the call sites will fail to compile, which is the point.
//!
//! Deliberately not here: comparing the installed bootloader against a known
//! hash. The bootloaders advertise `CAN_DOWNLOAD` without `CAN_UPLOAD`, so the
//! host cannot read flash back over DFU and has nothing to hash.

use nusb::{DeviceInfo, MaybeFuture};

/// Serial numbers that identify nothing.
///
/// The legacy ATS Lite bootloader reports a hardcoded string for every unit
/// (`firmware/legacy/vm-fw/boards/atslite/bootloader/src/main.rs`), so while a
/// device of that generation sits in DFU, its serial cannot be matched against
/// a uuid. The current bootloader derives the serial from FICR instead.
const PLACEHOLDER_SERIALS: &[&str] = &["1235678"];

pub fn is_placeholder_serial(serial: Option<&str>) -> bool {
    match serial {
        Some(serial) => PLACEHOLDER_SERIALS.contains(&serial),
        // A bootloader that publishes no serial at all is equally unidentifiable.
        None => true,
    }
}

/// Pick a device whose serial cannot identify it.
///
/// Only resolves when exactly one device of that vendor and product is attached,
/// so the result is unambiguous: with a second device present there is no way to
/// tell which one was asked for, and guessing would flash the wrong unit.
pub fn single_unidentifiable_device(candidates: Vec<DeviceInfo>) -> Option<DeviceInfo> {
    let mut unidentifiable = candidates
        .into_iter()
        .filter(|dev| is_placeholder_serial(dev.serial_number()));
    let only = unidentifiable.next()?;
    if unidentifiable.next().is_some() {
        tracing::warn!(
            "Several devices report an unusable serial; refusing to guess which one to use"
        );
        return None;
    }
    tracing::warn!(
        "Device {:04x}:{:04x} reports the placeholder serial {:?}; it is the only one attached, so treating it as the requested device. This accommodation goes away once its bootloader is updated.",
        only.vendor_id(),
        only.product_id(),
        only.serial_number().unwrap_or("<none>"),
    );
    Some(only)
}

/// A stand-in uuid for a bootloader that cannot identify itself and was never
/// seen running an application.
///
/// A device stuck in the bootloader — because an update failed before this
/// fleet was migrated — has no other way into the device list, and without an
/// entry there is no button to flash it. The value is derived from the product
/// id so it is stable across scans, and the leading `ff` bytes keep it clear of
/// a real uuid, which comes from FICR.
///
/// Only resolves when exactly one such device is attached; flashing then
/// resolves the same way through `single_unidentifiable_device`.
pub fn recovery_uuid(vid: u16, pid: u16) -> Option<[u8; 6]> {
    let candidates: Vec<_> = nusb::list_devices()
        .wait()
        .ok()?
        .filter(|dev| dev.vendor_id() == vid && dev.product_id() == pid)
        .collect();
    single_unidentifiable_device(candidates)?;
    Some([0xff, 0xff, 0xff, 0xff, (pid >> 8) as u8, pid as u8])
}

/// Whether a uuid is one this module invented rather than one a device reported.
pub fn is_recovery_uuid(uuid: &[u8; 6]) -> bool {
    uuid[..4] == [0xff, 0xff, 0xff, 0xff]
}

/// Find a bootloader that enumerates under the application's product id.
///
/// Bootloaders now take an id of their own, which is what makes the two modes
/// distinguishable from the descriptor alone. Anything flashed before that
/// still answers on the application's id, so a device being recovered has to be
/// looked for there too.
pub fn find_bootloader_sharing_app_pid(
    uuid: &[u8; 6],
    vid: u16,
    app_pid: u16,
) -> Result<nusb::DeviceInfo, crate::firmware::DfuError> {
    tracing::debug!(
        "No bootloader at {:04x}:{:04x}; looking for one that still shares the application id",
        vid,
        crate::firmware::bootloader_pid(app_pid)
    );
    crate::firmware::find_device_by_uuid(uuid, vid, app_pid)
}
