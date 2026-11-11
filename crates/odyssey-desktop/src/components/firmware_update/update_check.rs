use odyssey_hub_common::device::{Device, DeviceCapabilities, Transport};
use odyssey_hub_common::firmware_version::FirmwareVersion;
use odyssey_hub_server::firmware::{self, FirmwareManifest};

use super::update_notes::{HeldBack, NetCoreBlocked};
use super::update_state::FirmwareUpdateState;

/// The Odyssey Desktop version this build is, for matching against the oldest
/// desktop each firmware release declares it needs.
pub(super) fn desktop_version() -> semver::Version {
    semver::Version::parse(env!("ODYSSEY_BUILD_VERSION"))
        .expect("the build version is validated during release preparation")
}

/// "1.2.3" for a `[major, minor, patch]` version.
pub(super) fn format_version(version: [u32; 3]) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

/// DFU recovery entry, not a control-capable or DFU-only running application.
pub(super) fn is_in_bootloader(device: &Device) -> bool {
    device.capabilities.contains(DeviceCapabilities::DFU)
        && !device.capabilities.contains(DeviceCapabilities::CONTROL)
        && !device
            .capabilities
            .contains(DeviceCapabilities::DFU_RUNTIME_ONLY)
}

pub(super) fn installed_bootloader_floor<'a>(
    manifest: &'a FirmwareManifest,
    device_type: &str,
    device: &Device,
) -> Option<&'a semver::Version> {
    let installed = device.firmware_identity.known()?;
    manifest
        .devices
        .get(device_type)?
        .firmwares
        .iter()
        .find(|entry| {
            entry
                .version
                .parse::<FirmwareVersion>()
                .is_ok_and(|candidate| candidate == installed)
        })
        .map(|entry| &entry.requires_bootloader)
}

fn ats_ble_update_needs_usb(
    device_type: &str,
    device: &Device,
    target: &firmware::FirmwareEntry,
    radio_ota_bootloader: Option<&semver::Version>,
) -> bool {
    if !matches!(device_type, "atslite1" | "legacy-atslite1")
        || device.transport != Transport::UsbMux
    {
        return false;
    }
    let Some(installed_bootloader) = radio_ota_bootloader else {
        return true;
    };
    let target_needs_new_bootloader = target.requires_bootloader > *installed_bootloader;
    let netcore_update = target
        .files
        .get("net-core")
        .and_then(|image| image.release)
        .is_none_or(|release| release > device.netcore_release);
    let safe_netcore_bootloader = semver::Version::new(0, 2, 0);
    target_needs_new_bootloader
        || netcore_update && installed_bootloader < &safe_netcore_bootloader
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
    check_for_update_with_proof(manifest, device, current_state, None)
}
pub(super) fn check_for_update_with_proof(
    manifest: &FirmwareManifest,
    device: &Device,
    current_state: &FirmwareUpdateState,
    radio_ota_bootloader: Option<semver::Version>,
) -> FirmwareUpdateState {
    let device_version = device.firmware_version;
    let installed = device.firmware_identity.known();
    let is_dfu_device = is_in_bootloader(device);

    if device.product_id == 0
        || device_version == [0, 0, 0] && installed.is_none() && !is_dfu_device
    {
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
        &desktop_version(),
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
        is_dfu_device
            || installed.map_or_else(
                || firmware::is_worth_installing(fw, device_version, readable_netcore),
                |installed| {
                    fw.version
                        .parse::<FirmwareVersion>()
                        .is_ok_and(|available| {
                            available > installed
                                || available == installed
                                    && fw
                                        .files
                                        .get("net-core")
                                        .and_then(|file| file.release)
                                        .zip(readable_netcore)
                                        .is_some_and(|(carried, running)| carried > running)
                        })
                },
            )
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
    if ats_ble_update_needs_usb(
        device_type,
        device,
        available_fw,
        radio_ota_bootloader.as_ref(),
    ) {
        return FirmwareUpdateState::NeedsUsbConnection {
            current: device_version,
            available: available_version,
        };
    }

    if device.capabilities.contains(DeviceCapabilities::DFU)
        || device.capabilities.contains(DeviceCapabilities::CONTROL)
    {
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

#[cfg(test)]
mod tests {
    use odyssey_hub_common::device::EventsTransport;

    use super::*;

    #[test]
    fn dfu_only_runtime_is_updatable_but_unknown_version_never_counts_as_recovery() {
        let manifest: FirmwareManifest = serde_json::from_value(serde_json::json!({
            "version": 2,
            "devices": { "marker-control-board": { "firmwares": [{
                "version": "0.1.1", "requires_bootloader": "0.0.1", "requires_netcore": 0,
                "min_desktop": "0.7.3",
                "files": { "app": { "sha256": "unused", "size": 1 } }
            }] } }
        }))
        .unwrap();
        let mut device = Device {
            uuid: [1; 6],
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::new(
                DeviceCapabilities::DFU | DeviceCapabilities::DFU_RUNTIME_ONLY,
            ),
            firmware_identity: FirmwareVersion::default(),
            firmware_version: [0, 1, 0],
            events_transport: EventsTransport::Wired,
            events_connected: false,
            product_id: 0x5213,
            name: [0; 33],
            netcore_release: 0,
        };
        assert!(!is_in_bootloader(&device));
        assert!(matches!(
            check_for_update(&manifest, &device, &FirmwareUpdateState::Idle),
            FirmwareUpdateState::Available { .. }
        ));
        device.firmware_version = [0, 0, 0];
        assert!(!is_in_bootloader(&device));
        assert!(matches!(
            check_for_update(&manifest, &device, &FirmwareUpdateState::Idle),
            FirmwareUpdateState::Idle
        ));
        device
            .capabilities
            .remove(DeviceCapabilities::DFU_RUNTIME_ONLY);
        assert!(is_in_bootloader(&device));
        assert!(matches!(
            check_for_update(&manifest, &device, &FirmwareUpdateState::Idle),
            FirmwareUpdateState::Available { .. }
        ));
    }

    #[test]
    fn full_versions_order_nightlies_and_stable_releases() {
        let make_manifest = |versions: &[&str]| {
            serde_json::from_value(serde_json::json!({
                "version": 2,
                "devices": { "marker-control-board": { "firmwares": versions.iter().map(|version| {
                    serde_json::json!({
                        "version": version,
                        "requires_bootloader": "0.0.1",
                        "requires_netcore": 0,
                        "min_desktop": "0.7.3",
                        "files": { "app": { "sha256": "unused", "size": 1 } }
                    })
                }).collect::<Vec<_>>() } }
            }))
            .unwrap()
        };
        let older = "0.1.1-nightly.20261111.t010000.g000000000001";
        let newer = "0.1.1-nightly.20261111.t020000.g000000000002";
        let identity = |text: &str| {
            text.parse::<FirmwareVersion>()
                .expect("fixture is a firmware version")
        };
        let mut device = Device {
            uuid: [1; 6],
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::new(DeviceCapabilities::CONTROL),
            firmware_identity: identity(older),
            firmware_version: [0, 1, 1],
            events_transport: EventsTransport::Wired,
            events_connected: false,
            product_id: 0x5213,
            name: [0; 33],
            netcore_release: 0,
        };
        let state = check_for_update(
            &make_manifest(&[newer, older]),
            &device,
            &FirmwareUpdateState::Idle,
        );
        assert!(
            matches!(state, FirmwareUpdateState::Available { available, .. } if available == newer)
        );

        device.firmware_identity = identity(newer);
        let state = check_for_update(
            &make_manifest(&["0.1.1", newer, older]),
            &device,
            &FirmwareUpdateState::Idle,
        );
        assert!(
            matches!(state, FirmwareUpdateState::Available { available, .. } if available == "0.1.1")
        );
    }
    #[test]
    fn ats_ble_netcore_update_requires_proven_bounded_retry_bootloader() {
        let manifest: FirmwareManifest = serde_json::from_value(serde_json::json!({
            "version": 2,
            "devices": { "atslite1": { "firmwares": [
                {
                    "version": "0.3.0",
                    "requires_bootloader": "0.2.0",
                    "requires_netcore": 0,
                    "min_desktop": "0.0.0",
                    "files": {
                        "app-a": { "sha256": "unused", "size": 8 },
                        "app-b": { "sha256": "unused", "size": 8 },
                        "net-core": { "sha256": "unused", "size": 8, "release": 2 }
                    }
                },
                {
                    "version": "0.2.0",
                    "requires_bootloader": "0.2.0",
                    "requires_netcore": 0,
                    "min_desktop": "0.0.0",
                    "files": {
                        "app-a": { "sha256": "unused", "size": 8 },
                        "app-b": { "sha256": "unused", "size": 8 }
                    }
                }
            ] } }
        }))
        .expect("valid fixture");
        let target = &manifest.devices["atslite1"].firmwares[0];
        let mut device = Device {
            uuid: [1; 6],
            transport: Transport::UsbMux,
            capabilities: DeviceCapabilities::new(DeviceCapabilities::CONTROL),
            firmware_identity: "0.1.0".parse().expect("valid version"),
            firmware_version: [0, 1, 0],
            events_transport: EventsTransport::Bluetooth,
            events_connected: true,
            product_id: 0x5211,
            name: [0; 33],
            netcore_release: 1,
        };

        assert!(ats_ble_update_needs_usb(
            "atslite1",
            &device,
            target,
            None,
        ));
        device.firmware_identity = FirmwareVersion::default();
        device.firmware_version = [0, 2, 0];
        let proven_bootloader = semver::Version::new(0, 2, 0);
        assert!(!ats_ble_update_needs_usb(
            "atslite1",
            &device,
            target,
            Some(&proven_bootloader),
        ));
    }
}
