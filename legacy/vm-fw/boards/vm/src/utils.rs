pub fn device_id() -> [u8; 8] {
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.deviceid(0).read();
    let high = ficr.deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    [a, b, c, d, e, f, g, h]
}
