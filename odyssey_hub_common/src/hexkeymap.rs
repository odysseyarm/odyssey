use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;

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
