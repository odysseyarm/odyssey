use serde::de::{self, Deserializer, MapAccess, Unexpected, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;

pub struct HexKeyMap<T>(pub HashMap<u64, T>);

impl<'de, T> Deserialize<'de> for HexKeyMap<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HexKeyMapVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T> Visitor<'de> for HexKeyMapVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = HexKeyMap<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map with hexadecimal string keys and T values")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut values = HashMap::new();
                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    let parsed_key = u64::from_str_radix(key.trim_start_matches("0x"), 16)
                        .map_err(|_| de::Error::custom(format!("Invalid hex key: {}", key)))?;
                    values.insert(parsed_key, value);
                }
                Ok(HexKeyMap(values))
            }
        }

        deserializer.deserialize_map(HexKeyMapVisitor(std::marker::PhantomData))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HexValue(pub u64);

impl Serialize for HexValue {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ser.serialize_str(&format!("0x{:x}", self.0))
    }
}

impl<'de> Deserialize<'de> for HexValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HVVisitor;
        impl<'de> Visitor<'de> for HVVisitor {
            type Value = HexValue;
            fn expecting(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
                fmt.write_str("a hex string like 0xc0decafe")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let s = v
                    .strip_prefix("0x")
                    .ok_or_else(|| de::Error::invalid_value(Unexpected::Str(v), &self))?;
                u64::from_str_radix(s, 16)
                    .map(HexValue)
                    .map_err(|_| de::Error::invalid_value(Unexpected::Str(v), &self))
            }
        }
        deserializer.deserialize_str(HVVisitor)
    }
}

pub struct HexKeyMapN<T, const N: usize>(pub HashMap<[u8; N], T>);

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
            // produce e.g. "0x001122aabbcc"
            let key_str = format!(
                "0x{}",
                bytes
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>()
            );
            map_ser.serialize_entry(&key_str, value)?;
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
        struct HKMV<T, const N: usize>(PhantomData<T>);

        impl<'de, T, const N: usize> Visitor<'de> for HKMV<T, N>
        where
            T: Deserialize<'de>,
        {
            type Value = HexKeyMapN<T, N>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(
                    f,
                    "a map with {}-byte hex string keys (\"0x…\") and T values",
                    N
                )
            }

            fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut values = HashMap::with_capacity(access.size_hint().unwrap_or(0));
                while let Some((key_str, value)) = access.next_entry::<String, T>()? {
                    let s = key_str.strip_prefix("0x").ok_or_else(|| {
                        de::Error::custom(format!("Missing 0x prefix: {}", key_str))
                    })?;
                    if s.len() != 2 * N {
                        return Err(de::Error::custom(format!(
                            "Expected {} hex digits ({} bytes), got {}: {}",
                            2 * N,
                            N,
                            s.len(),
                            key_str
                        )));
                    }
                    let mut arr = [0u8; N];
                    for i in 0..N {
                        let byte_str = &s[2 * i..2 * i + 2];
                        arr[i] = u8::from_str_radix(byte_str, 16).map_err(|_| {
                            de::Error::custom(format!("Invalid hex byte: {}", byte_str))
                        })?;
                    }
                    values.insert(arr, value);
                }
                Ok(HexKeyMapN(values))
            }
        }

        deserializer.deserialize_map(HKMV(PhantomData))
    }
}
