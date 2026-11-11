//! USB CDC-ACM utilities

use embassy_usb::class::cdc_acm;
use embassy_usb::driver::EndpointError;

/// Writes `data` to `snd` in max-packet-size chunks. Logs and stops at the
/// first write error.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn write_serial<'d, D: embassy_usb::driver::Driver<'d>>(
    snd: &mut cdc_acm::Sender<'d, D>,
    data: &[u8],
) {
    let max_packet_size = usize::from(snd.max_packet_size());
    for chunk in data.chunks(max_packet_size) {
        if let Err(e) = snd.write_packet(chunk).await {
            defmt::warn!("USB write error: {:?}", defmt::Debug2Format(&e));
            return;
        }
    }
    // The host only sees the transfer end on a short packet, so a full last
    // packet needs a zero-length packet after it.
    if data.len() % max_packet_size == 0 {
        let _ = snd.write_packet(&[]).await;
    }
}

/// Returns the first byte of the next non-empty packet; the rest of that packet
/// is dropped. While the endpoint is disabled, waits for the host to reconnect.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn wait_for_serial<'d, D: embassy_usb::driver::Driver<'d>>(
    rcv: &mut cdc_acm::Receiver<'d, D>,
) -> u8 {
    loop {
        let mut buf = [0; 64];
        let r = rcv.read_packet(&mut buf).await;
        match r {
            Ok(n) if n > 0 => return buf[0],
            Ok(_) => continue,
            Err(EndpointError::Disabled) => {
                rcv.wait_connection().await;
            }
            Err(EndpointError::BufferOverflow) => unreachable!(),
        }
    }
}
