#[cfg(feature = "minicbor")]
use minicbor::{CborLen, Decode, Encode};
use odyssey_protocol_macros::protocol_type;

/// A device name, at most 32 bytes.
///
/// A newtype because pyo3 cannot convert `heapless::String`. Serializes
/// transparently.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "serde", serde(transparent))]
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceName(pub heapless::String<32>);

impl DeviceName {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Truncates on overflow, matching how names arrive off the wire.
    pub fn from_str_lossy(s: &str) -> Self {
        let mut out = heapless::String::new();
        for c in s.chars() {
            if out.push(c).is_err() {
                break;
            }
        }
        Self(out)
    }
}

impl core::ops::Deref for DeviceName {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl From<heapless::String<32>> for DeviceName {
    fn from(value: heapless::String<32>) -> Self {
        Self(value)
    }
}

impl From<DeviceName> for heapless::String<32> {
    fn from(value: DeviceName) -> Self {
        value.0
    }
}

/// Why a rename was refused.
///
/// `TooLong` is reachable only if the caller stops truncating: `DeviceName`
/// holds 32 bytes, so a longer name has to be rejected before it reaches the
/// device. `Failed` covers the device-side causes — a flash write that did not
/// stick, or a state that forbids renaming.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(eq, eq_int))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SetDeviceNameError {
    #[cfg_attr(feature = "minicbor", n(0))]
    TooLong,
    #[cfg_attr(feature = "minicbor", n(1))]
    Failed,
}

/// The result of a rename request.
///
/// A dedicated enum rather than `Result` because pyo3 converts `Result` only
/// as a function return, where it raises; a status reported by the device is
/// data.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(
        from = "Result<(), SetDeviceNameError>",
        into = "Result<(), SetDeviceNameError>"
    )
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetDeviceNameResult {
    Ok(),
    Err(SetDeviceNameError),
}

impl From<Result<(), SetDeviceNameError>> for SetDeviceNameResult {
    fn from(value: Result<(), SetDeviceNameError>) -> Self {
        match value {
            Ok(()) => Self::Ok(),
            Err(e) => Self::Err(e),
        }
    }
}

impl From<SetDeviceNameResult> for Result<(), SetDeviceNameError> {
    fn from(value: SetDeviceNameResult) -> Self {
        match value {
            SetDeviceNameResult::Ok() => Ok(()),
            SetDeviceNameResult::Err(e) => Err(e),
        }
    }
}

#[cfg(feature = "minicbor")]
impl<C> Encode<C> for SetDeviceNameResult {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        Result::<(), SetDeviceNameError>::from(*self).encode(e, ctx)
    }
}

#[cfg(feature = "minicbor")]
impl<'b, C> Decode<'b, C> for SetDeviceNameResult {
    fn decode(d: &mut minicbor::Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        Result::<(), SetDeviceNameError>::decode(d, ctx).map(Self::from)
    }
}

#[cfg(feature = "minicbor")]
impl<C> CborLen<C> for SetDeviceNameResult {
    fn cbor_len(&self, ctx: &mut C) -> usize {
        Result::<(), SetDeviceNameError>::from(*self).cbor_len(ctx)
    }
}
