use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::SerializeMap as _;
use serde::{Deserialize, Serialize, Serializer};

const HEX_PREFIX: &str = "0x";

/// A map keyed by fixed-size byte arrays that is written as a JSON object with
/// `"0x001122aabbcc"`-style keys (JSON keys must be strings).
pub(crate) struct HexKeyMapN<T, const N: usize>(pub HashMap<[u8; N], T>);

/// Renders `bytes` as `0x` followed by two lowercase hex digits per byte.
struct HexKey<'a, const N: usize>(&'a [u8; N]);

impl<const N: usize> fmt::Display for HexKey<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(HEX_PREFIX)?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl<const N: usize> Serialize for HexKey<'_, N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

/// Parses a key written by [`HexKey`].
fn parse_hex_key<const N: usize, E>(key: &str) -> Result<[u8; N], E>
where
    E: de::Error,
{
    let digits = key
        .strip_prefix(HEX_PREFIX)
        .ok_or_else(|| E::custom(format!("Missing 0x prefix: {key}")))?;
    if digits.len() != 2 * N {
        return Err(E::custom(format!(
            "Expected {} hex digits ({} bytes), got {}: {}",
            2 * N,
            N,
            digits.len(),
            key
        )));
    }
    let mut bytes = [0u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        // `get` rather than indexing: non-ASCII input can put a char boundary
        // inside a pair, and that must be an error, not a panic.
        let byte_str = digits
            .get(2 * index..2 * index + 2)
            .ok_or_else(|| E::custom(format!("Invalid hex byte at offset {index}: {key}")))?;
        *byte = u8::from_str_radix(byte_str, 16)
            .map_err(|source| E::custom(format!("Invalid hex byte: {byte_str} ({source})")))?;
    }
    Ok(bytes)
}

impl<T, const N: usize> Serialize for HexKeyMapN<T, N>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map_ser = serializer.serialize_map(Some(self.0.len()))?;
        for (bytes, value) in &self.0 {
            map_ser.serialize_entry(&HexKey(bytes), value)?;
        }
        map_ser.end()
    }
}

impl<'de, T, const N: usize> Deserialize<'de> for HexKeyMapN<T, N>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HexKeyMapVisitor<T, const N: usize>(PhantomData<T>);

        impl<'de, T, const N: usize> Visitor<'de> for HexKeyMapVisitor<T, N>
        where
            T: Deserialize<'de>,
        {
            type Value = HexKeyMapN<T, N>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    "a map with {N}-byte hex string keys (\"0x…\") and T values"
                )
            }

            fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut values = HashMap::with_capacity(access.size_hint().unwrap_or(0));
                while let Some((key_str, value)) = access.next_entry::<String, T>()? {
                    values.insert(parse_hex_key(&key_str)?, value);
                }
                Ok(HexKeyMapN(values))
            }
        }

        deserializer.deserialize_map(HexKeyMapVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(address: [u8; 3], value: u16) -> HexKeyMapN<u16, 3> {
        HexKeyMapN(HashMap::from([(address, value)]))
    }

    #[test]
    fn serializes_keys_as_zero_padded_lowercase_hex() {
        let json = serde_json::to_string(&map_of([0x00, 0xab, 0x0f], 7)).unwrap();
        assert_eq!(json, r#"{"0x00ab0f":7}"#);
    }

    #[test]
    fn round_trips() {
        let json = serde_json::to_string(&map_of([1, 2, 3], 9)).unwrap();
        let HexKeyMapN(parsed) = serde_json::from_str::<HexKeyMapN<u16, 3>>(&json).unwrap();
        assert_eq!(parsed, HashMap::from([([1, 2, 3], 9)]));
    }

    #[test]
    fn rejects_malformed_keys() {
        for key in [
            "001122",
            "0x0011",
            "0x00112233",
            "0x00zz22",
            "0x00é22",
            "0x0é222",
        ] {
            let json = format!(r#"{{"{key}": 1}}"#);
            assert!(
                serde_json::from_str::<HexKeyMapN<u16, 3>>(&json).is_err(),
                "{key} should be rejected"
            );
        }
    }
}
