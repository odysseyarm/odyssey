/// Dongle metadata returned by the dongle list subscription.
#[derive(Clone, Debug, PartialEq)]
pub struct DongleInfo {
    pub id: String,
    pub protocol_version: [u16; 3],         // mux endpoint protocol
    pub control_protocol_version: [u16; 3], // control endpoint protocol
    pub firmware_version: [u16; 3],
    pub connected_devices: Vec<[u8; 6]>,
    pub bonded_devices: Vec<[u8; 6]>,
}
