//! The packet recording file format read by `odyssey-playback`.
//!
//! The postcard-encoded aggregate [`GeneralSettings`], followed by one
//! `(u128 little-endian millisecond timestamp, postcard-encoded Packet)` pair
//! per recorded packet.

use odyssey_usb::device::GeneralSettings;
use odyssey_usb::packets::vm::{Packet, PacketData};

/// Serializes a recording: the device config, then every timestamped packet.
///
/// # Errors
///
/// Returns a `postcard::Error` if config or packet serialization fails.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn encode_recording(
    general_config: &GeneralSettings,
    packets: &[(u128, PacketData)],
) -> Result<Vec<u8>, postcard::Error> {
    let mut bytes = postcard::to_stdvec(general_config)?;
    for (timestamp, packet_data) in packets {
        let packet = Packet {
            data: packet_data.clone(),
            id: 0,
        };
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes = postcard::to_extend(&packet, bytes)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use nalgebra::Vector3;
    use odyssey_protocol::{AccelReport, ImpactReport};

    use super::*;

    /// The playback crate must read back exactly what `encode_recording` writes:
    /// config first, then each packet with its own timestamp, in order.
    #[test]
    fn recording_round_trips_through_playback() {
        let mut config = GeneralSettings {
            impact_threshold: 173,
            suppress_ms: 29,
            ..GeneralSettings::default()
        };
        config.accel_config.b_x = 0.25;
        config.accel_config.s_y = 1.5;
        config.gyro_config.b_z = -0.75;
        config.stereo_iso.translation.vector = Vector3::new(0.1, 0.2, 0.3);
        let packets = vec![
            (
                1_700_000_000_123,
                PacketData::AccelReport(AccelReport {
                    timestamp: 5,
                    accel: Vector3::new(1.0, 2.0, 3.0),
                    gyro: Vector3::new(4.0, 5.0, 6.0),
                }),
            ),
            (
                1_700_000_000_456,
                PacketData::ImpactReport(ImpactReport { timestamp: 9 }),
            ),
        ];
        let bytes = encode_recording(&config, &packets).expect("encodes");

        let path = std::env::temp_dir().join(format!(
            "odyssey-vision-tool-recording-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("writes the recording");
        let read = odyssey_playback::read_file(&path);
        std::fs::remove_file(&path).expect("removes the recording");
        let (read_config, read_packets) = read.expect("playback reads the recording");
        assert_eq!(read_config.impact_threshold, 173);
        assert_eq!(read_config.suppress_ms, 29);
        assert_eq!(read_config.accel_config, config.accel_config);
        assert_eq!(read_config.gyro_config, config.gyro_config);
        assert_eq!(read_config.stereo_iso, config.stereo_iso);
        match read_packets.as_slice() {
            [(1_700_000_000_123, first), (1_700_000_000_456, second)] => {
                let PacketData::AccelReport(accel) = &first.data else {
                    panic!("expected an accelerometer report, got {:?}", first.data);
                };
                assert_eq!(accel.timestamp, 5);
                assert_eq!(accel.accel, Vector3::new(1.0, 2.0, 3.0));
                assert_eq!(accel.gyro, Vector3::new(4.0, 5.0, 6.0));
                assert!(matches!(
                    second.data,
                    PacketData::ImpactReport(ImpactReport { timestamp: 9 })
                ));
            }
            other => panic!("unexpected recording packets: {other:?}"),
        }
    }
}
