//! The packet recording file format read by `odyssey-playback`.
//!
//! The postcard-encoded general config, followed by one
//! `(u128 little-endian millisecond timestamp, postcard-encoded Packet)` pair
//! per recorded packet.

use odyssey_usb::packets::vm::{Packet, PacketData};
use serde::Serialize;

/// Serializes a recording: the device config, then every timestamped packet.
pub fn encode_recording(
    general_config: &impl Serialize,
    packets: &[(u128, PacketData)],
) -> Result<Vec<u8>, postcard::Error> {
    let mut bytes = postcard::to_stdvec(general_config)?;
    for (timestamp, packet_data) in packets {
        let packet = Packet {
            data: packet_data.clone(),
            id: 0,
        };
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.extend(postcard::to_stdvec(&packet)?);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use odyssey_protocol::ImpactReport;
    use odyssey_usb::packets::vm::GeneralConfig;

    use super::*;

    /// The playback crate must read back exactly what `encode_recording` writes:
    /// config first, then each packet with its own timestamp, in order.
    #[test]
    fn recording_round_trips_through_playback() {
        let packets = vec![
            (
                1_700_000_000_123,
                PacketData::ImpactReport(ImpactReport { timestamp: 5 }),
            ),
            (
                1_700_000_000_456,
                PacketData::ImpactReport(ImpactReport { timestamp: 9 }),
            ),
        ];
        let bytes =
            encode_recording(&GeneralConfig::ImpactThreshold(7), &packets).expect("encodes");

        let path = std::env::temp_dir().join(format!(
            "odyssey-vision-tool-recording-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("writes the recording");
        let read = odyssey_playback::read_file(&path);
        std::fs::remove_file(&path).expect("removes the recording");
        let (config, read_packets) = read.expect("playback reads the recording");

        assert!(matches!(config, GeneralConfig::ImpactThreshold(7)));
        let summary: Vec<_> = read_packets
            .iter()
            .map(|(timestamp, packet)| match &packet.data {
                PacketData::ImpactReport(impact) => (*timestamp, impact.timestamp),
                other => panic!("unexpected packet {other:?}"),
            })
            .collect();
        assert_eq!(
            summary,
            vec![(1_700_000_000_123, 5), (1_700_000_000_456, 9)]
        );
    }
}
