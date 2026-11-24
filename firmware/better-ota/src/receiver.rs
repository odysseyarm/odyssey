//! The OTA state machine. `lib.rs` describes how it fits into the update flow.

use odyssey_protocol::ota::{Code, Crc32, Image, MAX_FRAME, Request, SIGNATURE_RECORD_LEN, Status};
use release_signing::Component;
#[cfg(feature = "signed")]
use {odyssey_protocol::ota::SIGNATURE_LEN, release_signing::Sha256};

/// Flash word size. Every write starts on, and (except for an image's tail) covers whole words.
const WRITE_ALIGNMENT: usize = 4;
/// Stack buffer for reading an image back to verify its CRC-32.
const VERIFY_CHUNK: usize = 64;
/// Smallest image the receiver accepts: the stack pointer and reset vector the validity check
/// reads.
const MIN_IMAGE_SIZE: u32 = 8;
/// Whether `Commit` requires a trusted signature on every staged image.
const SIGNATURE_REQUIRED: bool = cfg!(feature = "signed");
/// Largest data payload (`MAX_FRAME` minus opcode and offset) rounded up to a whole flash word.
const MAX_WRITE: usize = (MAX_FRAME - 5).next_multiple_of(WRITE_ALIGNMENT);

/// Data payload copied here so the backend sees a word-aligned buffer, since a frame sits at an
/// arbitrary offset inside the packet it arrived in.
#[repr(align(4))]
struct AlignedWrite([u8; MAX_WRITE]);

/// Flash slot operations used by the receiver. One implementation per board owns the flash
/// layout and is the only code that touches boot metadata.
///
/// Since: Unreleased.
pub trait Slots {
    /// Usable image capacity after backend reservations.
    fn capacity(&self, image: Image) -> u32;
    /// Writes bytes at aligned image offset. `offset` and `data.len()` are multiples of 4 and
    /// `data` is word aligned; an image's last write is padded with `0xff`. A write at offset 0
    /// starts a new image, so the backend erases what it needs there.
    fn write(
        &mut self,
        image: Image,
        offset: u32,
        data: &[u8],
    ) -> impl core::future::Future<Output = Result<(), Code>>;
    /// Reads actual image bytes for checksum verification.
    fn read(
        &mut self,
        image: Image,
        offset: u32,
        data: &mut [u8],
    ) -> impl core::future::Future<Output = Result<(), Code>>;
    /// Component id the image is signed as. Binds a signature to this board and image, so a
    /// signed image for another product does not verify here.
    fn component(&self, image: Image) -> Component;
    /// Checks image vectors and target-specific constraints. Runs after the CRC-32 matched.
    fn valid(
        &mut self,
        image: Image,
        size: u32,
    ) -> impl core::future::Future<Output = Result<(), Code>>;
    /// Writes durable activation metadata so the bootloader runs the staged images on the next
    /// reset. `None` means that image was not staged. An `Ok` return commits the update.
    fn commit(
        &mut self,
        app_size: Option<u32>,
        netcore_size: Option<u32>,
    ) -> impl core::future::Future<Output = Result<(), Code>>;
}

/// What the transport does after sending the status reply.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Continue,
    /// An update was committed. Reset the device after the reply is out, so the host sees the
    /// acknowledgement.
    Reset,
}

#[derive(Clone, Copy, Debug)]
struct Transfer {
    image: Image,
    size: u32,
    checksum: u32,
    offset: u32,
}

/// A signature record assembled from in-order chunks.
#[derive(Clone, Copy, Debug)]
struct SignatureBuffer {
    received: u8,
    bytes: [u8; SIGNATURE_RECORD_LEN],
}

impl SignatureBuffer {
    const EMPTY: Self = Self {
        received: 0,
        bytes: [0; SIGNATURE_RECORD_LEN],
    };
}

/// Stages images and activates them only after explicit commit.
///
/// One receiver serves one session. The transport must feed it one frame at a time and call
/// [`Receiver::abort`] when the link drops.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug)]
pub struct Receiver {
    error: Code,
    transfer: Option<Transfer>,
    app_size: Option<u32>,
    netcore_size: Option<u32>,
    /// Signature record received so far for each image, indexed by `Image as usize`.
    signatures: [SignatureBuffer; 2],
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    /// Creates an empty transfer session.
    pub const fn new() -> Self {
        Self {
            error: Code::Ok,
            transfer: None,
            app_size: None,
            netcore_size: None,
            signatures: [SignatureBuffer::EMPTY; 2],
        }
    }

    /// Processes one exact OTA frame.
    pub async fn handle(&mut self, slots: &mut impl Slots, frame: &[u8]) -> Result<Action, Code> {
        let request = match Request::decode(frame) {
            Ok(request) => request,
            Err(error) => return self.fail(error),
        };
        let result = match request {
            Request::Query => {
                self.error = Code::Ok;
                return Ok(Action::Continue);
            }
            Request::Begin { image, size, crc32 } => {
                // The slot must hold the size rounded up to a flash word, which is what gets written.
                let Some(write_size) = size.checked_add(3).map(|value| value & !3) else {
                    return self.fail(Code::Size);
                };
                if self.transfer.is_some() {
                    Err(Code::Busy)
                } else if size < MIN_IMAGE_SIZE || write_size > slots.capacity(image) {
                    Err(Code::Size)
                } else if self.staged_size(image).is_some() {
                    Err(Code::Busy)
                } else {
                    self.signatures[image as usize] = SignatureBuffer::EMPTY;
                    self.transfer = Some(Transfer {
                        image,
                        size,
                        checksum: crc32,
                        offset: 0,
                    });
                    Ok(Action::Continue)
                }
            }
            Request::Data { offset, data } => self.data(slots, offset, data).await,
            Request::Signature {
                image,
                offset,
                bytes,
            } => self.signature(image, offset, bytes),
            Request::Commit => self.commit(slots).await,
            Request::Abort => {
                self.abort();
                Ok(Action::Continue)
            }
        };
        match result {
            Ok(action) => {
                self.error = Code::Ok;
                Ok(action)
            }
            Err(error) => self.fail(error),
        }
    }

    /// Reports receiver and device state.
    pub fn status(
        &self,
        app_slot: u8,
        supported: u8,
        firmware_version: [u32; 3],
        netcore_release: u16,
    ) -> Status {
        Status {
            error: self.error,
            app_slot,
            supported,
            staged: u8::from(self.app_size.is_some())
                | (u8::from(self.netcore_size.is_some()) << 1),
            receiving: self.transfer.map(|transfer| transfer.image),
            offset: self.transfer.map_or(0, |transfer| transfer.offset),
            firmware_version,
            netcore_release,
            signature_required: SIGNATURE_REQUIRED,
        }
    }

    /// Drops in-memory partial and staged state without changing boot metadata.
    pub fn abort(&mut self) {
        self.transfer = None;
        self.app_size = None;
        self.netcore_size = None;
        self.signatures = [SignatureBuffer::EMPTY; 2];
    }

    async fn data(
        &mut self,
        slots: &mut impl Slots,
        offset: u32,
        data: &[u8],
    ) -> Result<Action, Code> {
        let Some(mut transfer) = self.transfer else {
            return Err(Code::State);
        };
        if offset != transfer.offset {
            return Err(Code::Offset);
        }
        if !(offset as usize).is_multiple_of(WRITE_ALIGNMENT) {
            return Err(Code::Alignment);
        }
        let length = u32::try_from(data.len()).map_err(|_| Code::Size)?;
        let end = offset.checked_add(length).ok_or(Code::Size)?;
        if end > transfer.size {
            return Err(Code::Size);
        }
        if end < transfer.size && !data.len().is_multiple_of(WRITE_ALIGNMENT) {
            return Err(Code::Alignment);
        }

        let mut padded = AlignedWrite([0xff; MAX_WRITE]);
        let write_len = (data.len() + 3) & !3;
        if write_len > padded.0.len() {
            return Err(Code::Size);
        }
        padded.0[..data.len()].copy_from_slice(data);
        if slots
            .write(transfer.image, offset, &padded.0[..write_len])
            .await
            .is_err()
        {
            self.abort();
            return Err(Code::Flash);
        }
        transfer.offset = end;
        self.transfer = Some(transfer);
        if end != transfer.size {
            return Ok(Action::Continue);
        }

        let mut crc = Crc32::new();
        let mut read_offset = 0;
        let mut bytes = [0; VERIFY_CHUNK];
        while read_offset < transfer.size {
            let count = (transfer.size - read_offset).min(VERIFY_CHUNK as u32) as usize;
            if slots
                .read(transfer.image, read_offset, &mut bytes[..count])
                .await
                .is_err()
            {
                self.abort();
                return Err(Code::Flash);
            }
            crc.update(&bytes[..count]);
            read_offset += count as u32;
        }
        if crc.finish() != transfer.checksum {
            self.abort();
            return Err(Code::Checksum);
        }
        if slots.valid(transfer.image, transfer.size).await.is_err() {
            self.abort();
            return Err(Code::Image);
        }
        self.transfer = None;
        match transfer.image {
            Image::Application => self.app_size = Some(transfer.size),
            Image::NetCore => self.netcore_size = Some(transfer.size),
        }
        Ok(Action::Continue)
    }

    fn signature(&mut self, image: Image, offset: u8, bytes: &[u8]) -> Result<Action, Code> {
        // Only an image in flight or staged can be signed; anything else is out of order.
        let present = self
            .transfer
            .is_some_and(|transfer| transfer.image == image)
            || self.staged_size(image).is_some();
        if !present {
            return Err(Code::State);
        }
        let buffer = &mut self.signatures[image as usize];
        if offset != buffer.received {
            return Err(Code::Offset);
        }
        let start = usize::from(offset);
        let end = start + bytes.len();
        let target = buffer.bytes.get_mut(start..end).ok_or(Code::Size)?;
        target.copy_from_slice(bytes);
        buffer.received = u8::try_from(end).map_err(|_| Code::Size)?;
        Ok(Action::Continue)
    }

    async fn commit(&mut self, slots: &mut impl Slots) -> Result<Action, Code> {
        if self.transfer.is_some() {
            return Err(Code::Busy);
        }
        if self.app_size.is_none() && self.netcore_size.is_none() {
            return Err(Code::State);
        }
        // A failed verification or commit drops every staged image; the host restages.
        if let Err(error) = self.verify_signatures(slots).await {
            self.abort();
            return Err(error);
        }
        if slots
            .commit(self.app_size, self.netcore_size)
            .await
            .is_err()
        {
            self.abort();
            return Err(Code::Flash);
        }
        self.app_size = None;
        self.netcore_size = None;
        self.signatures = [SignatureBuffer::EMPTY; 2];
        Ok(Action::Reset)
    }

    /// Without the `signed` feature neither the hash nor the verifier is compiled in: the ATS
    /// application's flash bank has little room to spare.
    #[cfg(not(feature = "signed"))]
    async fn verify_signatures(&self, _slots: &mut impl Slots) -> Result<(), Code> {
        Ok(())
    }

    /// Hashes each staged image as it sits in flash and checks its signature, so what is
    /// verified is exactly what the bootloader will install.
    #[cfg(feature = "signed")]
    async fn verify_signatures(&self, slots: &mut impl Slots) -> Result<(), Code> {
        for image in [Image::Application, Image::NetCore] {
            let Some(size) = self.staged_size(image) else {
                continue;
            };
            let buffer = &self.signatures[image as usize];
            if usize::from(buffer.received) != SIGNATURE_RECORD_LEN {
                return Err(Code::Signature);
            }
            let (version_bytes, signature) = buffer.bytes.split_at(12);
            let signature: &[u8; SIGNATURE_LEN] =
                signature.try_into().map_err(|_| Code::Signature)?;
            let mut version = [0; 3];
            for (word, chunk) in version.iter_mut().zip(version_bytes.as_chunks::<4>().0) {
                *word = u32::from_le_bytes(*chunk);
            }
            let mut hash = Sha256::new();
            let mut offset = 0;
            let mut bytes = [0; VERIFY_CHUNK];
            while offset < size {
                let count = (size - offset).min(VERIFY_CHUNK as u32) as usize;
                slots.read(image, offset, &mut bytes[..count]).await?;
                hash.update(&bytes[..count]);
                offset += count as u32;
            }
            let message =
                release_signing::message(slots.component(image), size, version, &hash.finalize());
            if !release_signing::verify(&message, signature) {
                return Err(Code::Signature);
            }
        }
        Ok(())
    }

    fn staged_size(&self, image: Image) -> Option<u32> {
        match image {
            Image::Application => self.app_size,
            Image::NetCore => self.netcore_size,
        }
    }

    fn fail<T>(&mut self, error: Code) -> Result<T, Code> {
        self.error = error;
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Poll, Waker};

    use odyssey_protocol::ota::{self, Code, Image, Request};
    use release_signing::Component;

    use super::{Action, Receiver, Slots};

    struct Flash {
        app: [u8; 256],
        netcore: [u8; 256],
        capacity: u32,
        fail_write: bool,
        fail_read: bool,
        fail_valid: bool,
        fail_commit: bool,
        writes: std::vec::Vec<(Image, u32, std::vec::Vec<u8>)>,
        committed: Option<(Option<u32>, Option<u32>)>,
    }
    impl Flash {
        fn new() -> Self {
            Self {
                app: [0xff; 256],
                netcore: [0xff; 256],
                capacity: 256,
                fail_write: false,
                fail_read: false,
                fail_valid: false,
                fail_commit: false,
                writes: std::vec::Vec::new(),
                committed: None,
            }
        }
        fn region(&mut self, image: Image) -> &mut [u8; 256] {
            match image {
                Image::Application => &mut self.app,
                Image::NetCore => &mut self.netcore,
            }
        }
    }
    impl Slots for Flash {
        fn capacity(&self, _: Image) -> u32 {
            self.capacity
        }
        async fn write(&mut self, image: Image, offset: u32, data: &[u8]) -> Result<(), Code> {
            assert_eq!(data.as_ptr() as usize % 4, 0);
            if self.fail_write {
                return Err(Code::Flash);
            }
            self.writes.push((image, offset, data.to_vec()));
            let start = offset as usize;
            self.region(image)[start..start + data.len()].copy_from_slice(data);
            Ok(())
        }
        async fn read(&mut self, image: Image, offset: u32, data: &mut [u8]) -> Result<(), Code> {
            if self.fail_read {
                return Err(Code::Flash);
            }
            let start = offset as usize;
            data.copy_from_slice(&self.region(image)[start..start + data.len()]);
            Ok(())
        }
        async fn valid(&mut self, _: Image, _: u32) -> Result<(), Code> {
            if self.fail_valid {
                Err(Code::Image)
            } else {
                Ok(())
            }
        }
        fn component(&self, image: Image) -> Component {
            match image {
                Image::Application => Component::Lite1App,
                Image::NetCore => Component::Lite1NetCore,
            }
        }
        async fn commit(&mut self, app: Option<u32>, netcore: Option<u32>) -> Result<(), Code> {
            if self.fail_commit {
                return Err(Code::Flash);
            }
            self.committed = Some((app, netcore));
            Ok(())
        }
    }
    fn run<F: core::future::Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test future must be ready"),
        }
    }
    fn request(
        receiver: &mut Receiver,
        flash: &mut Flash,
        request: Request<'_>,
    ) -> Result<Action, Code> {
        let mut frame = [0; ota::MAX_FRAME];
        let used = request.encode(&mut frame).expect("request fits frame");
        run(receiver.handle(flash, &frame[..used]))
    }
    fn transfer(
        receiver: &mut Receiver,
        flash: &mut Flash,
        image: Image,
        bytes: &[u8],
    ) -> Result<Action, Code> {
        request(
            receiver,
            flash,
            Request::Begin {
                image,
                size: bytes.len() as u32,
                crc32: ota::crc32(bytes),
            },
        )?;
        request(
            receiver,
            flash,
            Request::Data {
                offset: 0,
                data: bytes,
            },
        )
    }

    #[test]
    fn begin_checks_aligned_capacity_and_size_overflow() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        let begin = |size| Request::Begin {
            image: Image::Application,
            size,
            crc32: 0,
        };
        flash.capacity = 11;
        assert_eq!(
            request(&mut receiver, &mut flash, begin(9)),
            Err(Code::Size)
        );
        flash.capacity = 12;
        assert_eq!(
            request(&mut receiver, &mut flash, begin(9)),
            Ok(Action::Continue)
        );
        receiver.abort();
        assert_eq!(
            request(&mut receiver, &mut flash, begin(u32::MAX)),
            Err(Code::Size)
        );
        assert_eq!(flash.writes.len(), 0);
    }
    #[cfg(not(feature = "signed"))]
    #[test]
    fn stages_both_images_then_commits_once() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        let netcore = [1, 2, 3, 4, 5, 6, 7, 8];
        let app = [9, 8, 7, 6, 5, 4, 3, 2];
        assert_eq!(
            transfer(&mut receiver, &mut flash, Image::NetCore, &netcore),
            Ok(Action::Continue)
        );
        assert_eq!(
            transfer(&mut receiver, &mut flash, Image::Application, &app),
            Ok(Action::Continue)
        );
        assert_eq!(receiver.status(1, 3, [0; 3], 0).staged, 3);
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Ok(Action::Reset)
        );
        assert_eq!(flash.committed, Some((Some(8), Some(8))));
    }

    #[test]
    fn checksum_flash_and_vector_faults_discard_every_staged_image() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8];
        transfer(&mut receiver, &mut flash, Image::NetCore, &bytes).unwrap();
        request(
            &mut receiver,
            &mut flash,
            Request::Begin {
                image: Image::Application,
                size: 8,
                crc32: 0,
            },
        )
        .unwrap();
        assert_eq!(
            request(
                &mut receiver,
                &mut flash,
                Request::Data {
                    offset: 0,
                    data: &bytes
                }
            ),
            Err(Code::Checksum)
        );
        assert_eq!(receiver.status(1, 3, [0; 3], 0).staged, 0);
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::State)
        );
        flash.fail_write = true;
        assert_eq!(
            transfer(&mut receiver, &mut flash, Image::Application, &bytes),
            Err(Code::Flash)
        );
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::State)
        );
        flash.fail_write = false;
        flash.fail_valid = true;
        assert_eq!(
            transfer(&mut receiver, &mut flash, Image::Application, &bytes),
            Err(Code::Image)
        );
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::State)
        );
        assert_eq!(flash.committed, None);
    }

    #[test]
    fn truncation_order_and_overflow_never_activate() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        assert_eq!(
            run(receiver.handle(&mut flash, &[1, 0])),
            Err(Code::InvalidFrame)
        );
        assert_eq!(
            request(
                &mut receiver,
                &mut flash,
                Request::Data {
                    offset: 0,
                    data: &[0; 8]
                }
            ),
            Err(Code::State)
        );
        request(
            &mut receiver,
            &mut flash,
            Request::Begin {
                image: Image::Application,
                size: 8,
                crc32: ota::crc32(&[0; 8]),
            },
        )
        .unwrap();
        assert_eq!(
            request(
                &mut receiver,
                &mut flash,
                Request::Data {
                    offset: 0,
                    data: &[0; 9]
                }
            ),
            Err(Code::Size)
        );
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::Busy)
        );
        assert_eq!(flash.committed, None);
    }

    #[test]
    fn abort_and_disconnect_clear_partial_and_staged_state() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        request(
            &mut receiver,
            &mut flash,
            Request::Begin {
                image: Image::Application,
                size: 8,
                crc32: 0,
            },
        )
        .unwrap();
        receiver.abort();
        assert_eq!(receiver.status(0, 3, [0; 3], 0).receiving, None);
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::State)
        );
        transfer(
            &mut receiver,
            &mut flash,
            Image::Application,
            &[1, 2, 3, 4, 5, 6, 7, 8],
        )
        .unwrap();
        request(&mut receiver, &mut flash, Request::Abort).unwrap();
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::State)
        );
        assert_eq!(flash.committed, None);
    }

    #[cfg(not(feature = "signed"))]
    #[test]
    fn nonword_tail_gets_aligned_padding_but_checksum_uses_image_bytes() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        transfer(&mut receiver, &mut flash, Image::Application, &bytes).unwrap();
        assert_eq!(
            flash.writes[0].2,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 0xff, 0xff, 0xff]
        );
        assert_eq!(ota::crc32(&flash.app[..9]), ota::crc32(&bytes));
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Ok(Action::Reset)
        );
    }

    /// Data chunk of the marker's GATT link, which also sizes its signature chunks.
    const MARKER_CHUNK: usize = 12;

    fn send_record(
        receiver: &mut Receiver,
        flash: &mut Flash,
        image: Image,
        record: &[u8; ota::SIGNATURE_RECORD_LEN],
        chunk: usize,
    ) -> Result<(), Code> {
        for (index, bytes) in record.chunks(chunk).enumerate() {
            request(
                receiver,
                flash,
                Request::Signature {
                    image,
                    offset: (index * chunk) as u8,
                    bytes,
                },
            )?;
        }
        Ok(())
    }

    #[test]
    fn signature_needs_an_image_in_flight_or_staged() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        let record = [0; ota::SIGNATURE_RECORD_LEN];
        let send = |receiver: &mut Receiver, flash: &mut Flash| {
            send_record(receiver, flash, Image::Application, &record, MARKER_CHUNK)
        };
        assert_eq!(send(&mut receiver, &mut flash), Err(Code::State));
        request(
            &mut receiver,
            &mut flash,
            Request::Begin {
                image: Image::NetCore,
                size: 8,
                crc32: 0,
            },
        )
        .unwrap();
        assert_eq!(send(&mut receiver, &mut flash), Err(Code::State));
        request(&mut receiver, &mut flash, Request::Abort).unwrap();
        transfer(&mut receiver, &mut flash, Image::Application, &[0; 8]).unwrap();
        assert_eq!(send(&mut receiver, &mut flash), Ok(()));
    }

    #[test]
    fn signature_chunks_must_be_contiguous_and_stay_inside_the_record() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        transfer(&mut receiver, &mut flash, Image::Application, &[0; 8]).unwrap();
        let chunk = |offset, bytes| Request::Signature {
            image: Image::Application,
            offset,
            bytes,
        };
        assert_eq!(
            request(&mut receiver, &mut flash, chunk(12, &[0; 4])),
            Err(Code::Offset)
        );
        assert_eq!(
            request(&mut receiver, &mut flash, chunk(0, &[0; 12])),
            Ok(Action::Continue)
        );
        assert_eq!(
            request(&mut receiver, &mut flash, chunk(0, &[0; 12])),
            Err(Code::Offset)
        );
        assert_eq!(
            request(&mut receiver, &mut flash, chunk(12, &[0; 64])),
            Ok(Action::Continue)
        );
        assert_eq!(
            run(receiver.handle(&mut flash, &[5, 0, 76, 1])),
            Err(Code::Size)
        );
    }

    #[test]
    fn signature_record_fits_one_frame_or_marker_sized_chunks_and_abort_restarts_it() {
        let record = [7; ota::SIGNATURE_RECORD_LEN];
        for chunk in [ota::SIGNATURE_RECORD_LEN, MARKER_CHUNK, 1] {
            let mut receiver = Receiver::new();
            let mut flash = Flash::new();
            transfer(&mut receiver, &mut flash, Image::Application, &[0; 8]).unwrap();
            assert_eq!(
                send_record(
                    &mut receiver,
                    &mut flash,
                    Image::Application,
                    &record,
                    chunk
                ),
                Ok(()),
                "chunk {chunk}"
            );
        }
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        transfer(&mut receiver, &mut flash, Image::Application, &[0; 8]).unwrap();
        request(
            &mut receiver,
            &mut flash,
            Request::Signature {
                image: Image::Application,
                offset: 0,
                bytes: &record[..40],
            },
        )
        .unwrap();
        request(&mut receiver, &mut flash, Request::Abort).unwrap();
        transfer(&mut receiver, &mut flash, Image::Application, &[0; 8]).unwrap();
        assert_eq!(
            send_record(
                &mut receiver,
                &mut flash,
                Image::Application,
                &record,
                MARKER_CHUNK
            ),
            Ok(())
        );
    }

    #[cfg(feature = "signed")]
    fn stage_with_signature(
        receiver: &mut Receiver,
        flash: &mut Flash,
        image: Image,
        bytes: &[u8],
        version: [u32; 3],
        signature: [u8; ota::SIGNATURE_LEN],
    ) {
        transfer(receiver, flash, image, bytes).unwrap();
        let mut record = [0; ota::SIGNATURE_RECORD_LEN];
        for (chunk, word) in record.as_chunks_mut::<4>().0.iter_mut().zip(version) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        record[12..].copy_from_slice(&signature);
        send_record(receiver, flash, image, &record, MARKER_CHUNK).unwrap();
    }

    #[cfg(feature = "signed")]
    #[test]
    fn commit_with_partial_signature_record_is_refused() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        transfer(&mut receiver, &mut flash, Image::Application, &[1; 8]).unwrap();
        request(
            &mut receiver,
            &mut flash,
            Request::Signature {
                image: Image::Application,
                offset: 0,
                bytes: &[0; MARKER_CHUNK],
            },
        )
        .unwrap();
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::Signature)
        );
        assert_eq!(flash.committed, None);
    }

    #[cfg(feature = "signed")]
    #[test]
    fn commit_without_signature_is_refused_and_drops_staged_images() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        transfer(&mut receiver, &mut flash, Image::Application, &[1; 8]).unwrap();
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::Signature)
        );
        assert_eq!(receiver.status(0, 3, [0; 3], 0).staged, 0);
        assert_eq!(flash.committed, None);
    }

    #[cfg(feature = "signed")]
    #[test]
    fn commit_with_untrusted_signature_is_refused() {
        let mut receiver = Receiver::new();
        let mut flash = Flash::new();
        stage_with_signature(
            &mut receiver,
            &mut flash,
            Image::Application,
            &[1; 8],
            [1, 2, 3],
            [0x5a; ota::SIGNATURE_LEN],
        );
        assert_eq!(
            request(&mut receiver, &mut flash, Request::Commit),
            Err(Code::Signature)
        );
        assert_eq!(flash.committed, None);
    }

    #[cfg(all(feature = "signed", feature = "dev-signing-key"))]
    mod dev_signed {
        use release_signing::{Component, Sha256};

        use super::{
            Action, Code, Flash, Image, Receiver, Request, ota, request, stage_with_signature,
        };

        /// The public development seed, the same value as in `firmware/release-signing`.
        const DEV_SEED: [u8; 32] = [
            0xda, 0x87, 0xce, 0x81, 0xe4, 0xa7, 0x02, 0xa2, 0x7b, 0x21, 0xed, 0xf4, 0xe4, 0x59,
            0x4f, 0x6f, 0xd2, 0x77, 0x18, 0x9e, 0xe3, 0x95, 0x8c, 0x64, 0x8e, 0x50, 0xec, 0x73,
            0x35, 0xaa, 0x4b, 0x04,
        ];
        const APP: [u8; 8] = [9, 8, 7, 6, 5, 4, 3, 2];
        const NETCORE: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

        fn sign(component: Component, bytes: &[u8], version: [u32; 3]) -> [u8; ota::SIGNATURE_LEN] {
            let mut hash = Sha256::new();
            hash.update(bytes);
            let digest = hash.finalize();
            let message = release_signing::message(component, bytes.len() as u32, version, &digest);
            salty::Keypair::from(&DEV_SEED).sign(&message).to_bytes()
        }

        #[test]
        fn valid_signatures_commit_both_images() {
            let mut receiver = Receiver::new();
            let mut flash = Flash::new();
            let signed = sign;
            stage_with_signature(
                &mut receiver,
                &mut flash,
                Image::NetCore,
                &NETCORE,
                [2, 0, 0],
                signed(Component::Lite1NetCore, &NETCORE, [2, 0, 0]),
            );
            stage_with_signature(
                &mut receiver,
                &mut flash,
                Image::Application,
                &APP,
                [1, 2, 3],
                signed(Component::Lite1App, &APP, [1, 2, 3]),
            );
            assert_eq!(
                request(&mut receiver, &mut flash, Request::Commit),
                Ok(Action::Reset)
            );
            assert_eq!(flash.committed, Some((Some(8), Some(8))));
        }

        #[test]
        fn image_changed_after_staging_is_refused() {
            let mut receiver = Receiver::new();
            let mut flash = Flash::new();
            stage_with_signature(
                &mut receiver,
                &mut flash,
                Image::Application,
                &APP,
                [1, 2, 3],
                sign(Component::Lite1App, &APP, [1, 2, 3]),
            );
            flash.app[3] ^= 1;
            assert_eq!(
                request(&mut receiver, &mut flash, Request::Commit),
                Err(Code::Signature)
            );
            assert_eq!(flash.committed, None);
        }

        #[test]
        fn signature_for_another_component_or_version_is_refused() {
            let wrong = [
                (Component::Lite1NetCore, [1, 2, 3], [1, 2, 3]),
                (Component::MarkerApp, [1, 2, 3], [1, 2, 3]),
                (Component::Lite1App, [1, 2, 3], [1, 2, 4]),
            ];
            for (component, signed_version, sent_version) in wrong {
                let mut receiver = Receiver::new();
                let mut flash = Flash::new();
                stage_with_signature(
                    &mut receiver,
                    &mut flash,
                    Image::Application,
                    &APP,
                    sent_version,
                    sign(component, &APP, signed_version),
                );
                assert_eq!(
                    request(&mut receiver, &mut flash, Request::Commit),
                    Err(Code::Signature),
                    "{component:?} {signed_version:?} {sent_version:?}"
                );
            }
        }
    }
}
