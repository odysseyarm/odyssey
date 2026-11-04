//! Intel HEX to UF2 (<https://github.com/microsoft/uf2>), the hex path of
//! uf2conv.py: 256-byte payloads at 256-byte-aligned addresses, bytes the hex
//! does not set left zero. Blocks are written in address order.

use std::collections::BTreeMap;

use eros::Context as _;

const MAGIC_START0: u32 = 0x0A32_4655;
const MAGIC_START1: u32 = 0x9E5D_5157;
const MAGIC_END: u32 = 0x0AB1_6F30;
/// The file-size word carries a family id instead.
const FLAG_FAMILY_ID_PRESENT: u32 = 0x0000_2000;
const PAYLOAD_LEN: usize = 256;
const BLOCK_LEN: usize = 512;
/// Eight little-endian header words.
const HEADER_LEN: usize = 32;
const PADDING_LEN: usize = BLOCK_LEN - HEADER_LEN - PAYLOAD_LEN - 4;

const RECORD_DATA: u8 = 0x00;
const RECORD_EOF: u8 = 0x01;
const RECORD_EXTENDED_SEGMENT: u8 = 0x02;
const RECORD_START_SEGMENT: u8 = 0x03;
const RECORD_EXTENDED_LINEAR: u8 = 0x04;
const RECORD_START_LINEAR: u8 = 0x05;
/// Byte count, offset (2), type, up to 255 data bytes, checksum.
const MAX_RECORD_LEN: usize = 1 + 2 + 1 + 255 + 1;

pub(crate) fn from_ihex(hex: &str, family: u32) -> eros::Result<Vec<u8>> {
    let mut pages: BTreeMap<u32, [u8; PAYLOAD_LEN]> = BTreeMap::new();
    let mut base = 0u32;
    for (index, line) in hex.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record = record(line).with_context(|| format!("hex line {}", index + 1))?;
        match record.kind() {
            RECORD_DATA => {
                for (offset, byte) in (0u32..).zip(record.data()) {
                    let address = base
                        .checked_add(u32::from(record.offset()) + offset)
                        .with_context(|| format!("hex line {}: address overflows", index + 1))?;
                    let page = pages.entry(address & !0xFF).or_insert([0; PAYLOAD_LEN]);
                    page[(address & 0xFF) as usize] = *byte;
                }
            }
            RECORD_EOF => break,
            RECORD_EXTENDED_SEGMENT => base = u32::from(word(record.data())?) << 4,
            RECORD_EXTENDED_LINEAR => base = u32::from(word(record.data())?) << 16,
            RECORD_START_SEGMENT | RECORD_START_LINEAR => {}
            kind => eros::bail!("hex line {}: unknown record type {kind:#04x}", index + 1),
        }
    }
    let total = u32::try_from(pages.len()).context("too many UF2 blocks")?;
    let mut uf2 = Vec::with_capacity(pages.len() * BLOCK_LEN);
    for (number, (address, payload)) in (0u32..).zip(&pages) {
        let header = [
            MAGIC_START0,
            MAGIC_START1,
            FLAG_FAMILY_ID_PRESENT,
            *address,
            PAYLOAD_LEN as u32,
            number,
            total,
            family,
        ];
        for word in header {
            uf2.extend_from_slice(&word.to_le_bytes());
        }
        uf2.extend_from_slice(payload);
        uf2.extend_from_slice(&[0; PADDING_LEN]);
        uf2.extend_from_slice(&MAGIC_END.to_le_bytes());
    }
    Ok(uf2)
}

/// A decoded record whose length and checksum have been checked.
struct Record([u8; MAX_RECORD_LEN]);

impl Record {
    fn kind(&self) -> u8 {
        self.0[3]
    }

    fn offset(&self) -> u16 {
        u16::from_be_bytes([self.0[1], self.0[2]])
    }

    fn data(&self) -> &[u8] {
        &self.0[4..4 + usize::from(self.0[0])]
    }
}

fn record(line: &str) -> eros::Result<Record> {
    let Some(digits) = line.strip_prefix(':') else {
        eros::bail!("record does not start with ':'");
    };
    let len = digits.len() / 2;
    if digits.len() % 2 != 0 || !(5..=MAX_RECORD_LEN).contains(&len) {
        eros::bail!("record has {} hex digits", digits.len());
    }
    let mut bytes = [0; MAX_RECORD_LEN];
    hex::decode_to_slice(digits, &mut bytes[..len]).context("record is not hex")?;
    if len != usize::from(bytes[0]) + 5 {
        eros::bail!(
            "record says {} data bytes but carries {}",
            bytes[0],
            len - 5
        );
    }
    if bytes[..len]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
        != 0
    {
        eros::bail!("record checksum does not add up");
    }
    Ok(Record(bytes))
}

fn word(data: &[u8]) -> eros::Result<u16> {
    let [hi, lo] = data else {
        eros::bail!("address record carries {} bytes, not 2", data.len());
    };
    Ok(u16::from_be_bytes([*hi, *lo]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY: u32 = 0xADA5_2840;

    fn word_at(uf2: &[u8], block: usize, index: usize) -> u32 {
        let at = block * BLOCK_LEN + index * 4;
        u32::from_le_bytes(uf2[at..at + 4].try_into().expect("four bytes"))
    }

    #[test]
    fn data_crossing_a_page_boundary_fills_two_numbered_blocks() {
        // Extended segment 0x1000 << 4 = 0x10000; 4 bytes at 0x100FE..=0x10101.
        let hex = ":020000021000EC\n:0400FE00AABBCCDDF0\n:00000001FF\n";
        let uf2 = from_ihex(hex, FAMILY).expect("valid hex");
        assert_eq!(uf2.len(), 2 * BLOCK_LEN);
        for block in 0..2 {
            assert_eq!(word_at(&uf2, block, 0), MAGIC_START0);
            assert_eq!(word_at(&uf2, block, 2), FLAG_FAMILY_ID_PRESENT);
            assert_eq!(word_at(&uf2, block, 5), block as u32);
            assert_eq!(word_at(&uf2, block, 6), 2);
            assert_eq!(word_at(&uf2, block, 7), FAMILY);
            assert_eq!(word_at(&uf2, block, 127), MAGIC_END);
        }
        assert_eq!(word_at(&uf2, 0, 3), 0x1_0000);
        assert_eq!(word_at(&uf2, 1, 3), 0x1_0100);
        assert_eq!(
            &uf2[HEADER_LEN + 0xFC..HEADER_LEN + PAYLOAD_LEN],
            &[0, 0, 0xAA, 0xBB]
        );
        assert_eq!(
            &uf2[BLOCK_LEN + HEADER_LEN..BLOCK_LEN + HEADER_LEN + 3],
            &[0xCC, 0xDD, 0]
        );
    }

    #[test]
    fn extended_linear_address_sets_the_upper_half() {
        let hex = ":020000040001F9\n:0100000042BD\n:00000001FF\n";
        let uf2 = from_ihex(hex, FAMILY).expect("valid hex");
        assert_eq!(word_at(&uf2, 0, 3), 0x1_0000);
        assert_eq!(uf2[HEADER_LEN], 0x42);
    }

    #[test]
    fn corrupt_records_are_refused() {
        assert!(
            from_ihex(":0100000642B7\n", FAMILY).is_err(),
            "unknown type"
        );
        assert!(from_ihex(":0200000042BC\n", FAMILY).is_err(), "short data");
        assert!(from_ihex("0100000042BD\n", FAMILY).is_err(), "no colon");
        assert!(
            from_ihex(":0100000042BC\n", FAMILY).is_err(),
            "bad checksum"
        );
    }
}
