use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dfu_core::DfuProtocol;
use dfu_core::asynchronous::{DfuASync, DfuAsyncIo};
use dfu_core::functional_descriptor::FunctionalDescriptor;
use futures::io::AsyncRead;
use nusb::transfer::TransferError;

const CLASS_INTERFACE_OUT: u8 = 0x21;
const DFU_DNLOAD: u8 = 1;

struct ManifestationIo<'a, IO> {
    inner: &'a IO,
    end_accepted: &'a AtomicBool,
}

impl<IO> DfuAsyncIo for ManifestationIo<'_, IO>
where
    IO: DfuAsyncIo<Read = usize, Write = usize, Reset = (), Error = dfu_nusb::Error> + Sync,
{
    type Read = usize;
    type Write = usize;
    type Reset = ();
    type Error = dfu_nusb::Error;
    type MemoryLayout = IO::MemoryLayout;

    async fn read_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        buffer: &mut [u8],
    ) -> Result<usize, Self::Error> {
        self.inner
            .read_control(request_type, request, value, buffer)
            .await
    }

    async fn write_control(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        buffer: &[u8],
    ) -> Result<usize, Self::Error> {
        let written = self
            .inner
            .write_control(request_type, request, value, buffer)
            .await?;
        if request_type == CLASS_INTERFACE_OUT && request == DFU_DNLOAD && buffer.is_empty() {
            self.end_accepted.store(true, Ordering::Relaxed);
        }
        Ok(written)
    }

    async fn usb_reset(&self) -> Result<(), Self::Error> {
        self.inner.usb_reset().await
    }

    async fn sleep(&self, duration: Duration) {
        self.inner.sleep(duration).await;
    }

    fn protocol(&self) -> &DfuProtocol<Self::MemoryLayout> {
        self.inner.protocol()
    }

    fn functional_descriptor(&self) -> &FunctionalDescriptor {
        self.inner.functional_descriptor()
    }
}

// Reader progress reaches 100% before the final data block is acknowledged.
// Only an acknowledged zero-length DNLOAD puts subsequent errors in manifestation.
pub(super) async fn download<IO, R>(io: &IO, reader: R, length: u32) -> Result<(), dfu_nusb::Error>
where
    IO: DfuAsyncIo<Read = usize, Write = usize, Reset = (), Error = dfu_nusb::Error> + Sync,
    R: AsyncRead + Unpin,
{
    let end_accepted = AtomicBool::new(false);
    let mut dfu = DfuASync::new(ManifestationIo {
        inner: io,
        end_accepted: &end_accepted,
    });
    let result = dfu.download(reader, length).await;
    match result {
        Ok(()) => {}
        Err(dfu_nusb::Error::Transfer(
            TransferError::Cancelled
            | TransferError::Stall
            | TransferError::Fault
            | TransferError::Disconnected,
        )) if end_accepted.load(Ordering::Relaxed)
            && io.functional_descriptor().will_detach
            && !io.functional_descriptor().manifestation_tolerant =>
        {
            tracing::info!(
                "Device reset during DFU manifestation; image still requires verification"
            );
        }
        Err(error) => return Err(error),
    }
    tracing::info!("DFU transfer finished; awaiting device verification");
    // Reboot into the new firmware. The device may already have reset itself after the
    // download, so errors here are expected.
    if let Err(e) = dfu.detach().await {
        tracing::debug!(
            "Post-flash detach error (expected if device auto-reset): {}",
            e
        );
    }
    if let Err(e) = dfu.usb_reset().await {
        tracing::debug!(
            "Post-flash USB reset error (expected if device auto-reset): {}",
            e
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use dfu_core::State;
    use dfu_core::memory_layout::MemoryLayout;
    use futures::io::Cursor;

    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FailurePoint {
        LastDataWrite,
        LastDataStatus,
        EndWrite,
        EndStatus,
        DfuStatus,
        None,
    }

    struct Device {
        descriptor: FunctionalDescriptor,
        protocol: DfuProtocol<MemoryLayout>,
        failure: FailurePoint,
        error: TransferError,
        last_block: Mutex<Option<u16>>,
    }

    impl Device {
        fn new(failure: FailurePoint, error: TransferError) -> Self {
            Self {
                descriptor: FunctionalDescriptor {
                    can_download: true,
                    can_upload: false,
                    manifestation_tolerant: false,
                    will_detach: true,
                    detach_timeout: 0,
                    transfer_size: 6,
                    dfu_version: (1, 1),
                },
                protocol: DfuProtocol::Dfu,
                failure,
                error,
                last_block: Mutex::new(None),
            }
        }
    }

    impl DfuAsyncIo for Device {
        type Read = usize;
        type Write = usize;
        type Reset = ();
        type Error = dfu_nusb::Error;
        type MemoryLayout = MemoryLayout;

        async fn read_control(
            &self,
            request_type: u8,
            request: u8,
            _: u16,
            buffer: &mut [u8],
        ) -> Result<usize, Self::Error> {
            assert_eq!((request_type & 0x7f, request), (CLASS_INTERFACE_OUT, 3));
            let block = *self.last_block.lock().expect("test device lock");
            if (block == Some(1) && self.failure == FailurePoint::LastDataStatus)
                || (block == Some(2) && self.failure == FailurePoint::EndStatus)
            {
                return Err(dfu_nusb::Error::Transfer(self.error));
            }
            let state = match block {
                None => State::DfuIdle,
                Some(0 | 1) => State::DfuDnloadIdle,
                Some(2) if self.failure == FailurePoint::DfuStatus => State::DfuError,
                Some(2) => State::DfuManifest,
                _ => panic!("unexpected block {block:?}"),
            };
            buffer[..6].copy_from_slice(&[0, 0, 0, 0, u8::from(state), 0]);
            Ok(6)
        }

        async fn write_control(
            &self,
            request_type: u8,
            request: u8,
            value: u16,
            buffer: &[u8],
        ) -> Result<usize, Self::Error> {
            if request == 0 {
                return Err(dfu_nusb::Error::Transfer(TransferError::Disconnected));
            }
            assert_eq!((request_type, request), (CLASS_INTERFACE_OUT, DFU_DNLOAD));
            let expected = match value {
                0 => &[1, 2, 3, 4, 5, 6][..],
                1 => &[7][..],
                2 => &[],
                _ => panic!("unexpected block {value}"),
            };
            assert_eq!(buffer, expected);
            if (value == 1 && self.failure == FailurePoint::LastDataWrite)
                || (value == 2 && self.failure == FailurePoint::EndWrite)
            {
                return Err(dfu_nusb::Error::Transfer(self.error));
            }
            *self.last_block.lock().expect("test device lock") = Some(value);
            Ok(buffer.len())
        }

        async fn usb_reset(&self) -> Result<(), Self::Error> {
            Err(dfu_nusb::Error::Transfer(TransferError::Disconnected))
        }

        async fn sleep(&self, _: Duration) {}

        fn protocol(&self) -> &DfuProtocol<Self::MemoryLayout> {
            &self.protocol
        }

        fn functional_descriptor(&self) -> &FunctionalDescriptor {
            &self.descriptor
        }
    }

    #[tokio::test]
    async fn reset_errors_are_accepted_only_after_acknowledged_download_end() {
        for error in [
            TransferError::Cancelled,
            TransferError::Stall,
            TransferError::Fault,
            TransferError::Disconnected,
        ] {
            for failure in [
                FailurePoint::LastDataWrite,
                FailurePoint::LastDataStatus,
                FailurePoint::EndWrite,
                FailurePoint::EndStatus,
            ] {
                let result = download(
                    &Device::new(failure, error),
                    Cursor::new([1, 2, 3, 4, 5, 6, 7]),
                    7,
                )
                .await;
                if failure == FailurePoint::EndStatus {
                    assert!(result.is_ok(), "{failure:?}: {error:?}");
                } else {
                    assert!(
                        matches!(result, Err(dfu_nusb::Error::Transfer(got)) if got == error),
                        "{failure:?}: {error:?}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn manifestation_preserves_protocol_errors_and_unexpected_disconnects() {
        for (will_detach, tolerant) in [(false, false), (true, true)] {
            let mut device = Device::new(FailurePoint::EndStatus, TransferError::Fault);
            device.descriptor.will_detach = will_detach;
            device.descriptor.manifestation_tolerant = tolerant;
            assert!(matches!(
                download(&device, Cursor::new([1, 2, 3, 4, 5, 6, 7]), 7).await,
                Err(dfu_nusb::Error::Transfer(TransferError::Fault))
            ));
        }
        let device = Device::new(FailurePoint::DfuStatus, TransferError::Fault);
        assert!(matches!(
            download(&device, Cursor::new([1, 2, 3, 4, 5, 6, 7]), 7).await,
            Err(dfu_nusb::Error::Dfu(_))
        ));
        let device = Device::new(FailurePoint::EndStatus, TransferError::Unknown(42));
        assert!(matches!(
            download(&device, Cursor::new([1, 2, 3, 4, 5, 6, 7]), 7).await,
            Err(dfu_nusb::Error::Transfer(TransferError::Unknown(42)))
        ));
    }

    #[tokio::test]
    async fn normal_manifestation_completes_without_transfer_error() {
        let device = Device::new(FailurePoint::None, TransferError::Fault);
        download(&device, Cursor::new([1, 2, 3, 4, 5, 6, 7]), 7)
            .await
            .expect("normal manifestation");
        assert_eq!(
            *device.last_block.lock().expect("test device lock"),
            Some(2)
        );
    }
}
