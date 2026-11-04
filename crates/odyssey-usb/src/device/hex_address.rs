use std::fmt;

/// A device address (the 6-byte UUID) as hex for logs and display names.
///
/// `{}` prints `AA:BB:CC:DD:EE:FF`; the alternate form `{:#}` prints
/// `AABBCCDDEEFF`.
#[derive(Clone, Copy, Debug)]
pub(super) struct HexAddress(pub(super) [u8; 6]);

impl fmt::Display for HexAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if index > 0 && !f.alternate() {
                f.write_str(":")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_form_is_colon_separated_and_alternate_is_packed() {
        let address = HexAddress([0x00, 0xAB, 0x01, 0xFF, 0x10, 0x7E]);
        assert_eq!(address.to_string(), "00:AB:01:FF:10:7E");
        assert_eq!(format!("{address:#}"), "00AB01FF107E");
    }
}
