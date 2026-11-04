use std::time::Duration;

use odyssey_usb::device::VmDevice;
use tokio::time::sleep;
use tracing::{debug, warn};

/// Parse a [u8; 6] UUID from a USB serial number string (e.g. "AABBCCDDEEFF" or "AA:BB:CC:DD:EE:FF").
/// Returns None if the serial is absent or not a valid 12-hex-digit MAC address.
pub(super) fn uuid_from_usb_serial(serial: Option<&str>) -> Option<[u8; 6]> {
    let serial = serial?;
    let clean: String = serial.chars().filter(char::is_ascii_hexdigit).collect();
    if clean.len() != 12 {
        return None;
    }
    let mut uuid = [0u8; 6];
    for i in 0..6 {
        uuid[i] = u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(uuid)
}

/// Read a device UUID with up to 3 retries.
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
        for attempt in 1u8..=3 {
            match vm_device.read_uuid().await {
                Ok(uuid) => return Some(uuid),
                Err(e) if attempt < 3 => {
                    debug!(
                        "ATS Lite: Failed to read UUID via control (attempt {}): {}, retrying...",
                        attempt, e
                    );
                    sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    warn!(
                        "ATS Lite: Failed to read UUID via control after 3 attempts: {}, skipping",
                        e
                    );
                }
            }
        }
    } else {
        for attempt in 1u8..=3 {
            match vm_device.read_prop(odyssey_protocol::PropKind::Uuid).await {
                Ok(odyssey_protocol::Props::Uuid(bytes)) => return Some(bytes),
                Ok(other) => {
                    warn!("Expected Uuid prop, got {:?}, skipping device", other);
                    return None;
                }
                Err(e) if attempt < 3 => {
                    debug!(
                        "Failed to read UUID (attempt {}): {}, retrying...",
                        attempt, e
                    );
                    sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    warn!(
                        "Failed to read UUID after 3 attempts: {}, skipping device",
                        e
                    );
                }
            }
        }
    }
    None
}
