use atomic_pool::{Box, pool};
use core::ptr::NonNull;
use nrf_softdevice::ble::l2cap::Packet;

/// Maximum L2CAP packet size (aligned with MTU)
pub const L2CAP_MTU: usize = 512;

// Create a pool of L2CAP packet buffers (10 packets)
pool!(L2capPacketPool: [[u8; L2CAP_MTU]; 10]);

/// L2CAP packet using atomic_pool for allocation
pub struct L2capPacket {
    len: usize,
    buf: Box<L2capPacketPool>,
}

impl L2capPacket {
    #[allow(dead_code)]
    pub fn new() -> Option<Self> {
        Box::<L2capPacketPool>::new([0; L2CAP_MTU]).map(|buf| Self { len: 0, buf })
    }

    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() > L2CAP_MTU {
            return None;
        }

        let mut buf = Box::<L2capPacketPool>::new([0; L2CAP_MTU])?;
        buf[..slice.len()].copy_from_slice(slice);
        Some(Self {
            len: slice.len(),
            buf,
        })
    }

    #[allow(dead_code)]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Packet for L2capPacket {
    const MTU: usize = L2CAP_MTU;

    fn allocate() -> Option<NonNull<u8>> {
        Box::<L2capPacketPool>::new([0; L2CAP_MTU]).map(|buf| {
            let ptr = Box::into_raw(buf).cast::<u8>();
            ptr
        })
    }

    fn into_raw_parts(self) -> (NonNull<u8>, usize) {
        let ptr = Box::into_raw(self.buf).cast::<u8>();
        let len = self.len;
        (ptr, len)
    }

    unsafe fn from_raw_parts(ptr: NonNull<u8>, len: usize) -> Self {
        Self {
            len,
            buf: unsafe { Box::from_raw(ptr.cast::<[u8; L2CAP_MTU]>()) },
        }
    }
}
