const MAJOR: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MAJOR"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_MAJOR"),
};
const MINOR: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MINOR"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_MINOR"),
};
const PATCH: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_PATCH"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_PATCH"),
};

pub const FIRMWARE_VERSION: [u16; 3] = [MAJOR, MINOR, PATCH];

pub fn device_id() -> [u8; 8] {
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.deviceid(0).read();
    let high = ficr.deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    [a, b, c, d, e, f, g, h]
}
