//! USB CDC-ACM utilities

use embassy_usb::class::cdc_acm;
use embassy_usb::driver::EndpointError;

/// Write data to a CDC-ACM sender, handling packet size limits.
///
/// This function splits the data into chunks that fit within the max packet size
/// and sends a zero-length packet if necessary to signal end of transfer.
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
    // Send ZLP if data was exact multiple of max packet size
    if data.len() % max_packet_size == 0 {
        let _ = snd.write_packet(&[]).await;
    }
}

/// Wait for a single byte from a CDC-ACM receiver.
///
/// This function handles endpoint disabled errors by waiting for reconnection.
pub async fn wait_for_serial<'d, D: embassy_usb::driver::Driver<'d>>(
    rcv: &mut cdc_acm::Receiver<'d, D>,
) -> u8 {
    loop {
        let mut buf = [0; 64];
        let r = rcv.read_packet(&mut buf).await;
        match r {
            Ok(n) if n > 0 => return buf[0],
            Ok(_) => continue, // Empty packet, try again
            Err(EndpointError::Disabled) => {
                rcv.wait_connection().await;
            }
            Err(EndpointError::BufferOverflow) => unreachable!(),
        }
    }
}
