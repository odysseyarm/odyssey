//! Reads recorded device sessions: a postcard-encoded [`GeneralConfig`]
//! followed by `(timestamp, packet)` records until the end of the file.

use std::path::PathBuf;
use std::{fs, io};

use odyssey_usb::packets::vm::{GeneralConfig, Packet};
use postcard::take_from_bytes;

/// Every packet is preceded by its timestamp as a little-endian `u128`.
const TIMESTAMP_LEN: usize = size_of::<u128>();

fn invalid_data(what: &str, error: postcard::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what}: {error:?}"))
}

/// Reads a whole recording: the session's general configuration and every
/// timestamped packet after it.
///
/// # Errors
/// Fails when the file cannot be read, when the configuration or a packet does
/// not decode (`InvalidData`), or when the file ends inside a timestamp or a
/// packet (`UnexpectedEof`).
pub fn read_file(path: &PathBuf) -> io::Result<(GeneralConfig, Vec<(u128, Packet)>)> {
    let data = fs::read(path)?;

    let (general_config, mut bytes) = take_from_bytes::<GeneralConfig>(&data)
        .map_err(|error| invalid_data("GeneralConfig decode", error))?;

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
            Err(error) => return Err(invalid_data("Packet decode", error)),
        }
    }

    Ok((general_config, packets))
}
