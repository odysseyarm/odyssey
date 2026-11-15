use serde::de::{Deserializer, Error};

// FIXME: Restore skip_deserializing only when Serde preserves numeric tags: https://github.com/serde-rs/serde/pull/2952#issuecomment-6848033677
pub(crate) fn image_response<'de, D, T>(_deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    Err(D::Error::custom("image responses are device-to-host only"))
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Extended {
        Command,
        Status(#[serde(deserialize_with = "super::image_response")] u8),
        Chunk(#[serde(deserialize_with = "super::image_response")] u8),
        Object(#[serde(deserialize_with = "super::image_response")] u8),
        Appended(u8),
    }

    #[test]
    fn rejected_payloads_leave_later_variant_tags_reserved() {
        assert_eq!(
            postcard::from_bytes::<Extended>(&[4, 91]).expect("appended command decodes"),
            Extended::Appended(91)
        );
        assert_eq!(
            postcard::to_stdvec(&Extended::Appended(91)).expect("appended command encodes"),
            [4, 91]
        );
        for tag in 1..=3 {
            assert!(matches!(
                postcard::from_bytes::<Extended>(&[tag]),
                Err(postcard::Error::SerdeDeCustom)
            ));
        }
    }
}
