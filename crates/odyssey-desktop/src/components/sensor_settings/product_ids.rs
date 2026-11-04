//! Which sensor each device model carries.

const PID_ATS_VM: u16 = 0x520F;
const PID_ATS_LITE: u16 = 0x5210;
const PID_ATS_PRO: u16 = 0x5211;

/// The sensor chip family on a device; it decides which tuning panels apply.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SensorFamily {
    /// Two sensors: near field and wide field.
    Paj,
    /// One sensor on the near-field port.
    Pag,
}

impl SensorFamily {
    pub(super) fn for_product(product_id: u16) -> Option<Self> {
        match product_id {
            PID_ATS_VM | PID_ATS_LITE => Some(Self::Paj),
            PID_ATS_PRO => Some(Self::Pag),
            _ => None,
        }
    }
}

/// Only ATS Lite (0x5210) and Lite1 (0x5211) can switch between BLE and USB.
pub(super) fn supports_transport_switching(product_id: u16) -> bool {
    product_id == PID_ATS_LITE || product_id == PID_ATS_PRO
}
