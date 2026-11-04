use std::time::Duration;

use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::PacketData;
use tokio::time::sleep;
use tracing::{debug, info, warn};

const UUID_READ_ATTEMPTS: u8 = 3;
const UUID_READ_RETRY_DELAY: Duration = Duration::from_millis(500);

const EVENTS_VERSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const EVENTS_VERSION_REQUEST_ATTEMPTS: usize = 3;

/// Parse a six-byte UUID from a USB serial, allowing colon or dash separators.
/// Missing, malformed, or non-twelve-hex-digit serials have no usable UUID.
pub(super) fn uuid_from_usb_serial(serial: Option<&str>) -> Option<[u8; 6]> {
    let serial = serial?;
    let mut digits = serial.bytes().filter(|byte| !matches!(byte, b':' | b'-'));
    let mut uuid = [0; 6];
    for byte in &mut uuid {
        let high = char::from(digits.next()?).to_digit(16)? as u8;
        let low = char::from(digits.next()?).to_digit(16)? as u8;
        *byte = high << 4 | low;
    }
    digits.next().is_none().then_some(uuid)
}

/// Read a device UUID, retrying a few times.
/// Uses the control plane (read_uuid) for AtsLite/AtsPro (have a control endpoint),
/// or the packet protocol (read_prop) for AtsVm.
pub(super) async fn read_uuid_with_retry(
    vm_device: &VmDevice,
    product_id: odyssey_protocol::ProductId,
) -> Option<[u8; 6]> {
    if matches!(
        product_id,
        odyssey_protocol::ProductId::AtsLite | odyssey_protocol::ProductId::AtsPro
    ) {
        read_uuid_over_control(vm_device).await
    } else {
        read_uuid_over_packets(vm_device).await
    }
}

async fn read_uuid_over_control(vm_device: &VmDevice) -> Option<[u8; 6]> {
    for attempt in 1..=UUID_READ_ATTEMPTS {
        match vm_device.read_uuid().await {
            Ok(uuid) => return Some(uuid),
            Err(e) if attempt < UUID_READ_ATTEMPTS => {
                debug!(
                    "ATS Lite: Failed to read UUID via control (attempt {attempt}): {e}, retrying..."
                );
                sleep(UUID_READ_RETRY_DELAY).await;
            }
            Err(e) => warn!(
                "ATS Lite: Failed to read UUID via control after {UUID_READ_ATTEMPTS} attempts: {e}, skipping"
            ),
        }
    }
    None
}

async fn read_uuid_over_packets(vm_device: &VmDevice) -> Option<[u8; 6]> {
    for attempt in 1..=UUID_READ_ATTEMPTS {
        match vm_device.read_prop(odyssey_protocol::PropKind::Uuid).await {
            Ok(odyssey_protocol::Props::Uuid(bytes)) => return Some(bytes),
            Ok(other) => {
                warn!("Expected Uuid prop, got {other:?}, skipping device");
                return None;
            }
            Err(e) if attempt < UUID_READ_ATTEMPTS => {
                debug!("Failed to read UUID (attempt {attempt}): {e}, retrying...");
                sleep(UUID_READ_RETRY_DELAY).await;
            }
            Err(e) => {
                warn!(
                    "Failed to read UUID after {UUID_READ_ATTEMPTS} attempts: {e}, skipping device"
                );
            }
        }
    }
    None
}

/// Firmware (and control protocol) version over the control plane, which works in any
/// transport mode. `None` when the unit does not answer; the caller carries on without it.
pub(super) async fn read_firmware_version(
    vm_device: &VmDevice,
    uuid: [u8; 6],
) -> Option<odyssey_protocol::Version> {
    match vm_device.read_version().await {
        Ok(version) => {
            info!(
                "Device {uuid:02x?} firmware version: {}.{}.{}",
                version.firmware_semver[0], version.firmware_semver[1], version.firmware_semver[2]
            );
            Some(version)
        }
        Err(e) => {
            warn!("Failed to read version from device {uuid:02x?}: {e}");
            None
        }
    }
}

/// Which transport the unit streams its events over.
pub(super) async fn read_transport_mode(vm_device: &VmDevice, uuid: [u8; 6]) -> TransportMode {
    match vm_device.get_transport_mode().await {
        Ok(mode) => {
            info!("Device {uuid:02x?} is in {mode:?} mode");
            mode
        }
        Err(e) => {
            warn!("Failed to read transport mode from device {uuid:02x?}: {e}");
            // Assume USB mode if we can't read it
            TransportMode::Usb
        }
    }
}

/// Events protocol version via the packet interface, which is only readable while the unit
/// is in USB transport mode (direct events). `[0, 0, 0]` when unknown.
pub(super) async fn read_events_protocol_version(vm_device: &VmDevice, uuid: [u8; 6]) -> [u16; 3] {
    match vm_device
        .request_with_retry(
            PacketData::ReadVersion(),
            EVENTS_VERSION_REQUEST_TIMEOUT,
            EVENTS_VERSION_REQUEST_ATTEMPTS,
        )
        .await
    {
        Ok(PacketData::ReadVersionResponse(version)) => {
            info!(
                "Device {uuid:02x?} events protocol version: {}.{}.{}",
                version.protocol_semver[0], version.protocol_semver[1], version.protocol_semver[2]
            );
            version.protocol_semver
        }
        Ok(other) => {
            warn!("Unexpected response reading events version from {uuid:02x?}: {other:?}");
            [0, 0, 0]
        }
        Err(e) => {
            warn!("Failed to read events protocol version from {uuid:02x?}: {e}");
            [0, 0, 0]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_with_and_without_separators_gives_the_same_uuid() {
        let expected = Some([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(uuid_from_usb_serial(Some("AABBCCDDEEFF")), expected);
        assert_eq!(uuid_from_usb_serial(Some("aa:bb:cc:dd:ee:ff")), expected);
        assert_eq!(uuid_from_usb_serial(Some("aa-bb-cc-dd-ee-ff")), expected);
    }

    #[test]
    fn serial_that_is_not_twelve_hex_digits_is_rejected() {
        assert_eq!(uuid_from_usb_serial(None), None);
        assert_eq!(uuid_from_usb_serial(Some("AABBCCDDEE")), None);
        assert_eq!(uuid_from_usb_serial(Some("AABBCCDDEEFF00")), None);
        assert_eq!(uuid_from_usb_serial(Some("not a serial")), None);
        assert_eq!(uuid_from_usb_serial(Some("zzAABBCCDDEEFF")), None);
        assert_eq!(uuid_from_usb_serial(Some("AABB CCDDEEFF")), None);
    }
}
