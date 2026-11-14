/// A bounded list. A newtype because pyo3 cannot convert `heapless::Vec`.
///
/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[cfg_attr(feature = "serde", serde(transparent))]
#[cfg_attr(
    feature = "serde",
    serde(bound(
        serialize = "T: serde::Serialize",
        deserialize = "T: serde::Deserialize<'de>"
    ))
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List<T, const N: usize>(pub heapless::Vec<T, N>);

impl<T, const N: usize> Default for List<T, N> {
    fn default() -> Self {
        Self(heapless::Vec::new())
    }
}

impl<T, const N: usize> core::ops::Deref for List<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const N: usize> From<heapless::Vec<T, N>> for List<T, N> {
    fn from(value: heapless::Vec<T, N>) -> Self {
        Self(value)
    }
}

impl<T, const N: usize> From<List<T, N>> for heapless::Vec<T, N> {
    fn from(value: List<T, N>) -> Self {
        value.0
    }
}
