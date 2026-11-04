use dioxus::logger::tracing;
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_server::firmware::{self, FirmwareManifest};

use super::update_notes::{HeldBack, NetCoreBlocked};
use super::update_state::FirmwareUpdateState;

/// The Odyssey Desktop version this build is, for matching against the oldest
/// desktop each firmware release declares it needs.
pub(super) fn desktop_version() -> [u16; 3] {
    [
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
    ]
}

/// "1.2.3" for a `[major, minor, patch]` version.
pub(super) fn format_version(version: [u16; 3]) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

/// Whether the device is sitting in the DFU bootloader: DFU without CONTROL.
/// Application-mode devices expose a DFU interface too.
pub(super) fn is_in_bootloader(device: &Device) -> bool {
    device.capabilities.contains(DeviceCapabilities::DFU)
        && !device.capabilities.contains(DeviceCapabilities::CONTROL)
}

/// What the manifest offers `device`, as the state its row should show.
///
/// `current_state` is only read to log a transition once instead of on every
/// render.
pub(super) fn check_for_update(
    manifest: &FirmwareManifest,
    device: &Device,
    current_state: &FirmwareUpdateState,
) -> FirmwareUpdateState {
    let device_version = device.firmware_version;
    let is_dfu_device = is_in_bootloader(device);

    // The control protocol always reads a version, so [0,0,0] only comes from a
    // device already in the DFU bootloader.
    if device.product_id == 0 || device_version == [0, 0, 0] && !is_dfu_device {
        return FirmwareUpdateState::Idle;
    }
    let Some((vid, pid, device_type)) = firmware::device_info_from_pid(device.product_id) else {
        return FirmwareUpdateState::Idle;
    };

    // A device in the DFU bootloader is offered the latest firmware. A connected
    // device is offered it only when it is newer, across major and minor
    // versions too (protocol upgrades). `run_firmware_update` selects from the
    // same net core reading, so the entry shown here is the one installed.
    let offer = firmware::select_firmware(
        manifest,
        device_type,
        desktop_version(),
        device.netcore_release,
    );
    // A zero the hub has not settled on yet can mean the net core has
    // not answered (e.g. right after a transport swap), so it is not
    // judged as "behind".
    let readable_netcore = firmware::known_netcore_release(
        device
            .capabilities
            .contains(DeviceCapabilities::NETCORE_KNOWN),
        device.netcore_release,
    );
    let is_newer = |fw: &firmware::FirmwareEntry| {
        is_dfu_device || firmware::is_worth_installing(fw, device_version, readable_netcore)
    };
    let held_back = offer
        .held_back
        .filter(|fw| is_newer(fw))
        .map(|fw| HeldBack {
            version: fw.version.clone(),
            min_desktop: fw.min_desktop.clone(),
        });
    let net_core_blocked =
        offer
            .net_core_blocked
            .filter(|fw| is_newer(fw))
            .map(|fw| NetCoreBlocked {
                version: fw.version.clone(),
            });

    let Some(available_fw) = offer.offered.filter(|fw| is_newer(fw)) else {
        if let Some(held_back) = held_back {
            return FirmwareUpdateState::NeedsNewerDesktop(held_back);
        }
        if let Some(blocked) = net_core_blocked {
            return FirmwareUpdateState::NetCoreBlockedNoPath(blocked);
        }
        return FirmwareUpdateState::Idle;
    };
    let available_version = available_fw.version.clone();
    let device_uuid = device.uuid;

    // A device in the DFU bootloader needs flashing to recover, so it always
    // gets the button. Anything else needs CONTROL to be flashed over USB.
    if is_dfu_device || device.capabilities.contains(DeviceCapabilities::CONTROL) {
        if !matches!(current_state, FirmwareUpdateState::Available { .. }) {
            if is_dfu_device {
                tracing::info!(
                    "DFU device {:02x?} (PID 0x{:04x}) ready to flash -> {}",
                    device_uuid,
                    pid,
                    available_version
                );
            } else {
                tracing::info!(
                    "Update available for {:02x?}: {} -> {}",
                    device_uuid,
                    format_version(device_version),
                    available_version
                );
            }
        }
        return FirmwareUpdateState::Available {
            current: device_version,
            available: available_version,
            device_type: device_type.to_string(),
            vid,
            pid,
            held_back,
            net_core_blocked,
        };
    }

    if !matches!(
        current_state,
        FirmwareUpdateState::NeedsUsbConnection { .. }
    ) {
        tracing::info!(
            "Update available for {:02x?}: {} -> {} (needs USB connection)",
            device_uuid,
            format_version(device_version),
            available_version
        );
    }
    FirmwareUpdateState::NeedsUsbConnection {
        current: device_version,
        available: available_version,
    }
}
