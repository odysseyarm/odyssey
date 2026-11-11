use std::io::{Read, Write};
use std::time::{Duration, Instant};

use thiserror::Error;

// Wire protocol: https://docs.nordicsemi.com/bundle/sdk_nrf5_v17.1.0/page/lib_dfu_transport_serial.html
const CREATE: u8 = 0x01;
const SET_PRN: u8 = 0x02;
const CRC: u8 = 0x03;
const EXECUTE: u8 = 0x04;
const SELECT: u8 = 0x06;
const MTU: u8 = 0x07;
const WRITE: u8 = 0x08;
const PING: u8 = 0x09;
const FIRMWARE_VERSION: u8 = 0x0b;
const RESPONSE: u8 = 0x60;
const COMMAND: u8 = 1;
const DATA: u8 = 2;
const END: u8 = 0xc0;
const ESC: u8 = 0xdb;
const ESC_END: u8 = 0xdc;
const ESC_ESC: u8 = 0xdd;
const FRAME_LIMIT: usize = 4096;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Package {
    pub(super) init: Vec<u8>,
    pub(super) image: Vec<u8>,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Debug, Error)]
pub enum Error {
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Cannot open dongle bootloader serial port: {0}")]
    Serial(#[from] serialport::Error),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Dongle DFU I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// Since: Odyssey Desktop 0.7.5.
    #[error(
        "Dongle rejected DFU operation 0x{operation:02x}: result 0x{code:02x}, extended {extended:?}"
    )]
    Rejected {
        /// Since: Odyssey Desktop 0.7.5.
        operation: u8,
        /// Since: Odyssey Desktop 0.7.5.
        code: u8,
        /// Since: Odyssey Desktop 0.7.5.
        extended: Option<u8>,
    },
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Invalid dongle DFU response: {0}")]
    Protocol(&'static str),
    /// Since: Odyssey Desktop 0.7.5.
    #[error(
        "Dongle DFU checksum or byte offset does not match; reconnect in bootloader mode and retry"
    )]
    Checksum,
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Neither signed package was accepted by this dongle's bootloader")]
    NoMatchingSignature,
    /// Since: Odyssey Desktop 0.7.5.
    #[error(
        "This package could overwrite saved pairings during bootloader staging. Update stopped before erase; use a newer dongle firmware release."
    )]
    UnsafeLayout,
}

pub(super) fn flash(
    port: &str,
    packages: &[Package],
    progress: impl FnMut(usize, usize),
) -> Result<(), Error> {
    let mut serial = serialport::new(port, 115_200)
        .flow_control(serialport::FlowControl::Hardware)
        .timeout(Duration::from_secs(1))
        .open()?;
    serial.write_data_terminal_ready(true)?;
    transfer(&mut serial, packages, progress)
}

pub(super) fn transfer(
    io: &mut (impl Read + Write),
    packages: &[Package],
    mut progress: impl FnMut(usize, usize),
) -> Result<(), Error> {
    let mut link = Link {
        io,
        frame: [0; FRAME_LIMIT],
        chunk_size: 0,
        frame_limit: FRAME_LIMIT,
    };
    let ping = link.command(PING, &[1], 1)?;
    if ping[0] != 1 {
        return Err(Error::Protocol("ping identity mismatch"));
    }
    link.command(SET_PRN, &[0, 0], 0)?;
    let mtu = link.command(MTU, &[], 2)?;
    let mtu = usize::from(u16::from_le_bytes([mtu[0], mtu[1]])).min(FRAME_LIMIT);
    link.frame_limit = mtu;
    link.chunk_size = mtu
        .checked_sub(1)
        .and_then(|n| (n / 2).checked_sub(1))
        .filter(|&n| n > 0)
        .ok_or(Error::Protocol("MTU too small"))?;
    let bootloader = link.command(FIRMWARE_VERSION, &[0], 13)?;
    let application = link.command(FIRMWARE_VERSION, &[1], 13)?;
    let bootloader_start =
        u32::from_le_bytes([bootloader[5], bootloader[6], bootloader[7], bootloader[8]]);
    let installed_size = u32::from_le_bytes([
        application[9],
        application[10],
        application[11],
        application[12],
    ]);
    if bootloader[0] != 2
        || !matches!(application[0], 1 | 0xff)
        || (application[0] == 1
            && u32::from_le_bytes([
                application[5],
                application[6],
                application[7],
                application[8],
            ]) != super::layout::APP_ORIGIN)
    {
        return Err(Error::Protocol("unsupported bootloader application layout"));
    }

    let mut selected = None;
    for package in packages {
        if package.init.is_empty() || package.image.is_empty() || package.image.len() > 1024 * 1024
        {
            return Err(Error::Protocol("empty or oversized firmware package"));
        }
        validate_placement(bootloader_start, installed_size, package.image.len())?;
        let maximum = link.select(COMMAND)?;
        if package.init.len() > maximum {
            return Err(Error::Protocol(
                "signed init packet exceeds bootloader limit",
            ));
        }
        // CREATE(command) resets pending upload, including after a rejected signature.
        link.create(COMMAND, package.init.len())?;
        let mut crc = crc32fast::Hasher::new();
        link.stream(&package.init, 0, &mut crc)?;
        match link.command(EXECUTE, &[], 0) {
            Ok(_) => {
                selected = Some(package);
                break;
            }
            // Nordic SDK nrf_dfu_validation_signature_check returns INVALID_OBJECT (0x05).
            Err(Error::Rejected {
                operation: EXECUTE,
                code: 0x05 | 0x06,
                ..
            }) => {}
            Err(error) => return Err(error),
        }
    }
    let package = selected.ok_or(Error::NoMatchingSignature)?;
    let maximum = link.select(DATA)?;
    let mut crc = crc32fast::Hasher::new();
    let total = package.image.len();
    progress(0, total);
    for (index, object) in package.image.chunks(maximum).enumerate() {
        let offset = index * maximum;
        link.create(DATA, object.len())?;
        link.stream(object, offset, &mut crc)?;
        // A final disconnect is not success: require the execute acknowledgement.
        link.command(EXECUTE, &[], 0)?;
        progress(offset + object.len(), total);
    }
    Ok(())
}

fn validate_placement(
    bootloader_start: u32,
    installed_size: u32,
    incoming_size: usize,
) -> Result<(), Error> {
    use super::layout::{APP_ORIGIN, APPLICATION_SIZE, FLASH_END, SETTINGS_ORIGIN};
    const PAGE: u32 = 4096;
    if installed_size > APPLICATION_SIZE
        || incoming_size == 0
        || incoming_size > APPLICATION_SIZE as usize
        || !(SETTINGS_ORIGIN..FLASH_END).contains(&bootloader_start)
    {
        return Err(Error::Protocol("invalid firmware flash bounds"));
    }
    let staging = (APP_ORIGIN + installed_size).next_multiple_of(PAGE);
    let incoming = (incoming_size as u32).next_multiple_of(PAGE);
    // SDK cache_prepare falls back to bank 0 only when bank 1 cannot fit the image.
    if staging <= bootloader_start
        && incoming <= bootloader_start - staging
        && staging + incoming > SETTINGS_ORIGIN
    {
        return Err(Error::UnsafeLayout);
    }
    Ok(())
}

struct Link<'a, I> {
    io: &'a mut I,
    frame: [u8; FRAME_LIMIT],
    chunk_size: usize,
    frame_limit: usize,
}

impl<I: Read + Write> Link<'_, I> {
    fn send(&mut self, operation: u8, payload: &[u8]) -> Result<(), Error> {
        let mut size = 0;
        for byte in std::iter::once(operation).chain(payload.iter().copied()) {
            let escaped = match byte {
                END => Some(ESC_END),
                ESC => Some(ESC_ESC),
                _ => None,
            };
            if size + 3 > self.frame_limit {
                return Err(Error::Protocol("outgoing frame exceeds MTU"));
            }
            if let Some(escaped) = escaped {
                self.frame[size] = ESC;
                self.frame[size + 1] = escaped;
                size += 2;
            } else {
                self.frame[size] = byte;
                size += 1;
            }
        }
        self.frame[size] = END;
        self.io.write_all(&self.frame[..=size])?;
        Ok(())
    }

    fn command(
        &mut self,
        operation: u8,
        payload: &[u8],
        expected: usize,
    ) -> Result<[u8; 13], Error> {
        self.send(operation, payload)?;
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        let mut response = [0_u8; 16];
        let mut size = 0;
        let mut escaped = false;
        loop {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "dongle DFU response timed out",
                )
                .into());
            }
            let mut byte = [0];
            match self.io.read_exact(&mut byte) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                result => result?,
            }
            let byte = byte[0];
            if byte == END && !escaped {
                if size == 0 {
                    continue;
                }
                break;
            }
            let decoded = if escaped {
                escaped = false;
                match byte {
                    ESC_END => END,
                    ESC_ESC => ESC,
                    _ => return Err(Error::Protocol("invalid SLIP escape")),
                }
            } else if byte == ESC {
                escaped = true;
                continue;
            } else {
                byte
            };
            if size == response.len() {
                return Err(Error::Protocol("response frame too long"));
            }
            response[size] = decoded;
            size += 1;
        }
        if size < 3 || response[0] != RESPONSE || response[1] != operation {
            return Err(Error::Protocol("unexpected response opcode"));
        }
        let code = response[2];
        if code != 1 {
            if (code == 0x0b && size != 4) || (code != 0x0b && size != 3) {
                return Err(Error::Protocol("invalid rejection length"));
            }
            if operation == FIRMWARE_VERSION && code == 2 {
                return Err(Error::Protocol(
                    "bootloader cannot report flash layout; update stopped before erase",
                ));
            }
            return Err(Error::Rejected {
                operation,
                code,
                extended: (code == 0x0b).then_some(response[3]),
            });
        }
        if size != expected + 3 || expected > 13 {
            return Err(Error::Protocol("unexpected response length"));
        }
        let mut payload = [0; 13];
        payload[..expected].copy_from_slice(&response[3..size]);
        Ok(payload)
    }

    fn select(&mut self, kind: u8) -> Result<usize, Error> {
        let response = self.command(SELECT, &[kind], 12)?;
        let maximum =
            u32::from_le_bytes([response[0], response[1], response[2], response[3]]) as usize;
        if maximum == 0 || maximum > 1024 * 1024 {
            return Err(Error::Protocol("invalid maximum object size"));
        }
        if kind == DATA && response[4..12] != [0; 8] {
            return Err(Error::Protocol(
                "new signed init did not reset firmware offset",
            ));
        }
        Ok(maximum)
    }

    fn create(&mut self, kind: u8, size: usize) -> Result<(), Error> {
        let mut request = [kind, 0, 0, 0, 0];
        let size =
            u32::try_from(size).map_err(|_overflow| Error::Protocol("object size overflow"))?;
        request[1..].copy_from_slice(&size.to_le_bytes());
        self.command(CREATE, &request, 0)?;
        Ok(())
    }

    fn stream(
        &mut self,
        bytes: &[u8],
        offset: usize,
        crc: &mut crc32fast::Hasher,
    ) -> Result<(), Error> {
        for chunk in bytes.chunks(self.chunk_size) {
            self.send(WRITE, chunk)?;
            crc.update(chunk);
        }
        let response = self.command(CRC, &[], 8)?;
        let actual_offset =
            u32::from_le_bytes([response[0], response[1], response[2], response[3]]) as usize;
        let actual_crc = u32::from_le_bytes([response[4], response[5], response[6], response[7]]);
        if actual_offset != offset + bytes.len() || actual_crc != crc.clone().finalize() {
            return Err(Error::Checksum);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
