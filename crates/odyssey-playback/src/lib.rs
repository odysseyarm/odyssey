//! Reads recorded device sessions: postcard-encoded [`GeneralSettings`]
//! followed by `(timestamp, packet)` records until the end of the file.

use std::path::Path;
use std::{fs, io};

use odyssey_usb::device::GeneralSettings;
use odyssey_usb::packets::vm::Packet;
use postcard::take_from_bytes;

/// Every packet is preceded by its timestamp as a little-endian `u128`.
const TIMESTAMP_LEN: usize = size_of::<u128>();

fn invalid_data(what: &str, error: &postcard::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what}: {error:?}"))
}

/// Reads a whole recording: the session's general configuration and every
/// timestamped packet after it.
///
/// # Errors
/// Fails when the file cannot be read, when the configuration or a packet does
/// not decode (`InvalidData`), or when the file ends inside a timestamp or a
/// packet (`UnexpectedEof`).
///
/// # Since
/// Odyssey Desktop 0.6.0.
pub fn read_file(path: &Path) -> io::Result<(GeneralSettings, Vec<(u128, Packet)>)> {
    let data = fs::read(path)?;

    let (general_config, mut bytes) = take_from_bytes::<GeneralSettings>(&data)
        .map_err(|error| invalid_data("GeneralSettings decode", &error))?;

    let mut packets = Vec::new();
    while !bytes.is_empty() {
        let Some((timestamp_bytes, after_timestamp)) = bytes.split_first_chunk::<TIMESTAMP_LEN>()
        else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF in timestamp",
            ));
        };
        let timestamp = u128::from_le_bytes(*timestamp_bytes);

        match take_from_bytes::<Packet>(after_timestamp) {
            Ok((packet, rest)) => {
                bytes = rest;
                packets.push((timestamp, packet));
            }
            Err(postcard::Error::DeserializeUnexpectedEnd) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF in Packet",
                ));
            }
            Err(error) => return Err(invalid_data("Packet decode", &error)),
        }
    }

    Ok((general_config, packets))
}

#[cfg(test)]
mod tests {
    use odyssey_usb::packets::vm::PacketData;

    use super::*;

    fn read_bytes(bytes: &[u8]) -> io::Result<(GeneralSettings, Vec<(u128, Packet)>)> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "odyssey-playback-{}-{}.bin",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, bytes)?;
        let result = read_file(&path);
        fs::remove_file(path)?;
        result
    }

    #[test]
    fn recording_preserves_settings_and_record_boundaries() {
        let config = GeneralSettings {
            impact_threshold: 7,
            suppress_ms: 11,
            ..GeneralSettings::default()
        };
        let mut bytes = postcard::to_stdvec(&config).unwrap();
        for timestamp in [u128::MAX, 0] {
            bytes.extend_from_slice(&timestamp.to_le_bytes());
            bytes.extend(
                postcard::to_stdvec(&Packet {
                    id: 42,
                    data: PacketData::Ack(),
                })
                .unwrap(),
            );
        }
        let (read_config, records) = read_bytes(&bytes).unwrap();
        assert_eq!(
            postcard::to_stdvec(&read_config).unwrap(),
            postcard::to_stdvec(&config).unwrap()
        );
        assert!(matches!(
            records.as_slice(),
            [
                (
                    u128::MAX,
                    Packet {
                        id: 42,
                        data: PacketData::Ack()
                    }
                ),
                (
                    0,
                    Packet {
                        id: 42,
                        data: PacketData::Ack()
                    }
                )
            ]
        ));
    }

    #[test]
    fn incomplete_record_is_unexpected_eof_and_invalid_packet_is_invalid_data() {
        let header = postcard::to_stdvec(&GeneralSettings::default()).unwrap();
        let packet = postcard::to_stdvec(&Packet {
            id: 0,
            data: PacketData::Ack(),
        })
        .unwrap();
        let mut record = Vec::from(1_u128.to_le_bytes());
        record.extend(packet);
        for length in 1..record.len() {
            let mut bytes = header.clone();
            bytes.extend_from_slice(&record[..length]);
            assert_eq!(
                read_bytes(&bytes).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        let mut bytes = header;
        bytes.extend_from_slice(&0_u128.to_le_bytes());
        // PacketData has no variant numbered 127.
        bytes.push(127);
        assert_eq!(
            read_bytes(&bytes).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
