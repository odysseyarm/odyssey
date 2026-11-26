//! HCI transport layers [📖](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface.html)

use core::mem::MaybeUninit;

use bt_hci_transport::{PacketKind, PacketToController, PacketToHost, ReadHciError, WithIndicator};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use embedded_io::{Error as _, ErrorKind, ErrorType, ReadExactError};

use super::icmsg_config;
use super::uninit_write_buf::UninitWriteBuf;

const STASH_MAX: usize = 1024;

/// Since: Odyssey Desktop 0.7.0.
pub struct MyTransport<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier> {
    reader: Mutex<M, icmsg::Receiver<W, { icmsg_config::ALIGN }>>,
    writer: Mutex<M, icmsg::Sender<N, { icmsg_config::ALIGN }>>,
}

impl<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier> MyTransport<M, W, N> {
    /// Since: Odyssey Desktop 0.7.0.
    pub fn new(
        reader: icmsg::Receiver<W, { icmsg_config::ALIGN }>,
        writer: icmsg::Sender<N, { icmsg_config::ALIGN }>,
    ) -> Self {
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
        }
    }
}

/// Transport failure. A local type because `ExternalController` needs
/// `From<ReadHciError<Infallible>>` and the orphan rule forbids adding that to
/// `ReadHciError<ErrorKind>`.
///
/// Since: Unreleased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub struct TransportError(pub ReadHciError<ErrorKind>);

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.0, f)
    }
}

impl core::error::Error for TransportError {}

impl embedded_io::Error for TransportError {
    fn kind(&self) -> ErrorKind {
        self.0.kind()
    }
}

impl From<ReadHciError<core::convert::Infallible>> for TransportError {
    fn from(error: ReadHciError<core::convert::Infallible>) -> Self {
        Self(map_read_error(error))
    }
}

impl<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier> ErrorType for MyTransport<M, W, N> {
    type Error = TransportError;
}

impl<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier> bt_hci_transport::Transport
    for MyTransport<M, W, N>
{
    async fn read<'a, P: PacketToHost<'a>>(&self, rx: &'a mut [u8]) -> Result<P, Self::Error> {
        let mut storage = [0; STASH_MAX];
        let mut r = self.reader.lock().await;
        let n = r
            .recv(&mut storage)
            .await
            .map_err(|e| TransportError(ReadHciError::Read(ReadExactError::Other(e.kind()))))?;
        decode_packet(&storage[..n], rx).map_err(TransportError)
    }

    async fn write<T: PacketToController>(&self, tx: &T) -> Result<(), Self::Error> {
        let needed = tx.size() + 1;
        assert!(needed <= STASH_MAX);

        let mut storage: [MaybeUninit<u8>; STASH_MAX] =
            [const { MaybeUninit::uninit() }; STASH_MAX];
        let mut sink = UninitWriteBuf::new(&mut storage[..needed]);

        defmt::unwrap!(WithIndicator::new(tx).write_hci_async(&mut sink).await);

        let buf = sink.as_init();

        let mut w = self.writer.lock().await;
        w.send(buf)
            .map_err(|e| TransportError(ReadHciError::Read(ReadExactError::Other(e.kind()))))
    }
}

fn map_read_error(error: ReadHciError<core::convert::Infallible>) -> ReadHciError<ErrorKind> {
    match error {
        ReadHciError::BufferTooSmall => ReadHciError::BufferTooSmall,
        ReadHciError::InvalidValue => ReadHciError::InvalidValue,
        ReadHciError::Read(ReadExactError::UnexpectedEof) => {
            ReadHciError::Read(ReadExactError::UnexpectedEof)
        }
        ReadHciError::Read(ReadExactError::Other(never)) => match never {},
    }
}

fn decode_packet<'a, P: PacketToHost<'a>>(
    bytes: &[u8],
    rx: &'a mut [u8],
) -> Result<P, ReadHciError<ErrorKind>> {
    let mut data = bytes;
    let kind = PacketKind::read(&mut data).map_err(map_read_error)?;
    let packet = P::read_hci(kind, &mut data, rx).map_err(map_read_error)?;
    if !data.is_empty() {
        return Err(ReadHciError::InvalidValue);
    }
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use bt_hci::ControllerToHostPacket;

    use super::*;

    const COMMAND_COMPLETE: &[u8] = &[4, 0x0e, 4, 1, 3, 0x0c, 0];

    #[test]
    fn hci_frame_requires_exact_length_and_valid_output_capacity() {
        let mut buffer = [0; 16];
        assert!(decode_packet::<ControllerToHostPacket>(COMMAND_COMPLETE, &mut buffer).is_ok());
        assert!(matches!(
            decode_packet::<ControllerToHostPacket>(&[4, 0x0e, 4, 1], &mut buffer),
            Err(ReadHciError::Read(ReadExactError::UnexpectedEof))
        ));
        let mut extra = [0; 8];
        extra[..7].copy_from_slice(COMMAND_COMPLETE);
        assert!(matches!(
            decode_packet::<ControllerToHostPacket>(&extra, &mut buffer),
            Err(ReadHciError::InvalidValue)
        ));
        assert!(matches!(
            decode_packet::<ControllerToHostPacket>(COMMAND_COMPLETE, &mut []),
            Err(ReadHciError::BufferTooSmall)
        ));
    }
}
