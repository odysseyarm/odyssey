use odyssey_protocol_macros::protocol_type;

#[protocol_type]
#[repr(u8)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    #[cfg_attr(feature = "minicbor", n(0))]
    Object,
    #[cfg_attr(feature = "minicbor", n(1))]
    Image,
}
