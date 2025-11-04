/// L2CAP MTU (Maximum Transmission Unit)
pub const PAYLOAD_LEN: usize = 1024;

/// L2CAP packet wrapper that manages a buffer
pub struct L2capPacket {
    buf: heapless::Vec<u8, PAYLOAD_LEN>,
}

impl L2capPacket {
    /// Create a new L2CAP packet from a slice
    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() > PAYLOAD_LEN {
            return None;
        }

        let mut buf = heapless::Vec::new();
        buf.extend_from_slice(slice).ok()?;

        Some(Self { buf })
    }

    /// Get the packet data as a slice
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    /// Get the length of the packet
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Check if the packet is empty
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Consume the packet and return the buffer
    pub fn into_inner(self) -> heapless::Vec<u8, PAYLOAD_LEN> {
        self.buf
    }
}
