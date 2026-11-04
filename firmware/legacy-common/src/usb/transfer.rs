use core::cmp::max;

use embassy_usb::driver::{EndpointError, EndpointOut};

pub(super) const BULK_MAX_PACKET_SIZE: u16 = 64;

/// Retains a partial bulk transfer when another transport cancels a pending read.
///
/// Use with the endpoints created by `usb_device`. A short packet (including a
/// ZLP) terminates the transfer. Oversized transfers are drained to that boundary
/// before reporting overflow, so their tail cannot become the next request.
pub struct TransferReader<const N: usize> {
    data: [u8; N],
    len: usize,
    overflow: bool,
}

impl<const N: usize> TransferReader<N> {
    pub const fn new() -> Self {
        Self {
            data: [0; N],
            len: 0,
            overflow: false,
        }
    }

    /// The endpoint's single-packet read must be cancellation-safe, as nRF USB is.
    pub async fn read<'a>(
        &'a mut self,
        ep: &mut impl EndpointOut,
    ) -> Result<&'a [u8], EndpointError> {
        let max_packet = max(ep.info().max_packet_size, 1) as usize;
        let mut tail = [0; BULK_MAX_PACKET_SIZE as usize];

        loop {
            // Read directly into the transfer buffer whenever a full packet fits.
            // The scratch buffer also lets an exactly-full transfer consume its
            // terminating ZLP without incorrectly reporting overflow.
            let direct = !self.overflow && self.data.len() - self.len >= max_packet;
            let result = if direct {
                ep.read(&mut self.data[self.len..]).await
            } else {
                ep.read(&mut tail).await
            };
            let n = match result {
                Ok(n) => n,
                Err(error) => {
                    self.len = 0;
                    self.overflow = false;
                    return Err(error);
                }
            };

            if !self.overflow {
                if n > self.data.len() - self.len {
                    self.overflow = true;
                } else {
                    if !direct {
                        self.data[self.len..self.len + n].copy_from_slice(&tail[..n]);
                    }
                    self.len += n;
                }
            }

            if n < max_packet {
                let len = self.len;
                self.len = 0;
                if core::mem::replace(&mut self.overflow, false) {
                    return Err(EndpointError::BufferOverflow);
                }
                return Ok(&self.data[..len]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::future::{Future, poll_fn};
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};
    use std::collections::VecDeque;
    use std::vec;
    use std::vec::Vec;

    use embassy_usb::driver::{Endpoint, EndpointInfo, EndpointType};

    use super::*;

    enum Read {
        Packet(Vec<u8>),
        Pending,
        Disabled,
    }

    struct EndpointStub {
        info: EndpointInfo,
        reads: VecDeque<Read>,
    }

    impl EndpointStub {
        fn new(reads: impl IntoIterator<Item = Read>) -> Self {
            Self {
                info: EndpointInfo {
                    addr: 1.into(),
                    ep_type: EndpointType::Bulk,
                    max_packet_size: BULK_MAX_PACKET_SIZE,
                    interval_ms: 0,
                },
                reads: reads.into_iter().collect(),
            }
        }
    }

    impl Endpoint for EndpointStub {
        fn info(&self) -> &EndpointInfo {
            &self.info
        }

        async fn wait_enabled(&mut self) {}
    }

    impl EndpointOut for EndpointStub {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
            poll_fn(
                |_| match self.reads.pop_front().expect("unexpected endpoint read") {
                    Read::Packet(packet) => {
                        assert!(packet.len() <= buf.len());
                        buf[..packet.len()].copy_from_slice(&packet);
                        Poll::Ready(Ok(packet.len()))
                    }
                    Read::Pending => Poll::Pending,
                    Read::Disabled => Poll::Ready(Err(EndpointError::Disabled)),
                },
            )
            .await
        }
    }

    fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
        pin!(future).poll(&mut Context::from_waker(Waker::noop()))
    }

    fn ready<F: Future>(future: F) -> F::Output {
        match poll_once(future) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("unexpected pending transfer"),
        }
    }

    #[test]
    fn cancellation_retains_every_consumed_packet() {
        let mut reader = TransferReader::<256>::new();
        let mut endpoint = EndpointStub::new([
            Read::Packet(vec![1; 64]),
            Read::Pending,
            Read::Packet(vec![2; 64]),
            Read::Pending,
            Read::Packet(vec![3; 7]),
        ]);
        assert!(poll_once(reader.read(&mut endpoint)).is_pending());
        assert!(poll_once(reader.read(&mut endpoint)).is_pending());
        let mut expected = vec![1; 64];
        expected.extend([2; 64]);
        expected.extend([3; 7]);
        assert_eq!(ready(reader.read(&mut endpoint)).unwrap(), expected);
    }

    #[test]
    fn full_buffer_waits_for_zlp_without_losing_next_transfer() {
        let mut reader = TransferReader::<64>::new();
        let mut endpoint = EndpointStub::new([
            Read::Packet(vec![1; 64]),
            Read::Pending,
            Read::Packet(vec![]),
            Read::Packet(vec![2; 3]),
        ]);
        assert!(poll_once(reader.read(&mut endpoint)).is_pending());
        assert_eq!(ready(reader.read(&mut endpoint)).unwrap(), &[1; 64]);
        assert_eq!(ready(reader.read(&mut endpoint)).unwrap(), &[2; 3]);
    }

    #[test]
    fn short_final_packet_fits_remaining_capacity() {
        let mut reader = TransferReader::<65>::new();
        let mut endpoint = EndpointStub::new([Read::Packet(vec![1; 64]), Read::Packet(vec![2])]);
        let mut expected = vec![1; 64];
        expected.push(2);
        assert_eq!(ready(reader.read(&mut endpoint)).unwrap(), expected);
    }

    #[test]
    fn cancelled_overflow_drains_tail_before_next_transfer() {
        let mut reader = TransferReader::<65>::new();
        let mut endpoint = EndpointStub::new([
            Read::Packet(vec![1; 64]),
            Read::Packet(vec![2; 64]),
            Read::Pending,
            Read::Packet(vec![3; 7]),
            Read::Packet(vec![4; 3]),
        ]);
        assert!(poll_once(reader.read(&mut endpoint)).is_pending());
        assert_eq!(
            ready(reader.read(&mut endpoint)),
            Err(EndpointError::BufferOverflow)
        );
        assert_eq!(ready(reader.read(&mut endpoint)).unwrap(), &[4; 3]);
    }

    #[test]
    fn disabled_endpoint_discards_partial_and_overflow_state() {
        let mut reader = TransferReader::<64>::new();
        let mut endpoint = EndpointStub::new([
            Read::Packet(vec![1; 64]),
            Read::Packet(vec![2; 64]),
            Read::Pending,
            Read::Disabled,
            Read::Packet(vec![3; 7]),
        ]);
        assert!(poll_once(reader.read(&mut endpoint)).is_pending());
        assert_eq!(
            ready(reader.read(&mut endpoint)),
            Err(EndpointError::Disabled)
        );
        assert_eq!(ready(reader.read(&mut endpoint)).unwrap(), &[3; 7]);
    }
}
