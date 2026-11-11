use odyssey_protocol::ota::{Code, Crc32, Image, Request, Status};

const WRITE_ALIGNMENT: usize = 4;
const VERIFY_CHUNK: usize = 64;

#[repr(align(4))]
struct AlignedWrite([u8; 96]);

/// Flash slot operations used by the receiver.
///
/// Since: Unreleased.
pub trait Slots {
    /// Usable image capacity after backend reservations.
    fn capacity(&self, image: Image) -> u32;
    /// Writes bytes at aligned image offset.
    fn write(&mut self, image: Image, offset: u32, data: &[u8]) -> impl core::future::Future<Output = Result<(), Code>>;
    /// Reads actual image bytes for checksum verification.
    fn read(&mut self, image: Image, offset: u32, data: &mut [u8]) -> impl core::future::Future<Output = Result<(), Code>>;
    /// Checks image vectors and target-specific constraints.
    fn valid(&mut self, image: Image, size: u32) -> impl core::future::Future<Output = Result<(), Code>>;
    /// Writes durable activation metadata.
    fn commit(&mut self, app_size: Option<u32>, netcore_size: Option<u32>) -> impl core::future::Future<Output = Result<(), Code>>;
}

/// Result of processing one OTA request.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action { Continue, Reset }

#[derive(Clone, Copy, Debug)]
struct Transfer { image: Image, size: u32, checksum: u32, offset: u32 }

/// Stages images and activates them only after explicit commit.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug)]
pub struct Receiver {
    error: Code,
    transfer: Option<Transfer>,
    app_size: Option<u32>,
    netcore_size: Option<u32>,
}

impl Default for Receiver {
    fn default() -> Self { Self::new() }
}

impl Receiver {
    /// Creates an empty transfer session.
    pub const fn new() -> Self {
        Self { error: Code::Ok, transfer: None, app_size: None, netcore_size: None }
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
                let Some(write_size) = size.checked_add(3).map(|value| value & !3) else {
                    return self.fail(Code::Size);
                };
                if self.transfer.is_some() { Err(Code::Busy) }
                else if size < 8 || write_size > slots.capacity(image) { Err(Code::Size) }
                else if self.staged_size(image).is_some() { Err(Code::Busy) }
                else {
                    self.transfer = Some(Transfer { image, size, checksum: crc32, offset: 0 });
                    Ok(Action::Continue)
                }
            }
            Request::Data { offset, data } => self.data(slots, offset, data).await,
            Request::Commit => self.commit(slots).await,
            Request::Abort => { self.abort(); Ok(Action::Continue) }
        };
        match result {
            Ok(action) => { self.error = Code::Ok; Ok(action) }
            Err(error) => self.fail(error),
        }
    }

    /// Reports receiver and device state.
    pub fn status(&self, app_slot: u8, supported: u8, firmware_version: [u32; 3], netcore_release: u16) -> Status {
        Status {
            error: self.error,
            app_slot,
            supported,
            staged: u8::from(self.app_size.is_some()) | (u8::from(self.netcore_size.is_some()) << 1),
            receiving: self.transfer.map(|transfer| transfer.image),
            offset: self.transfer.map_or(0, |transfer| transfer.offset),
            firmware_version,
            netcore_release,
        }
    }

    /// Drops in-memory partial and staged state without changing boot metadata.
    pub fn abort(&mut self) {
        self.transfer = None;
        self.app_size = None;
        self.netcore_size = None;
    }

    async fn data(&mut self, slots: &mut impl Slots, offset: u32, data: &[u8]) -> Result<Action, Code> {
        let Some(mut transfer) = self.transfer else { return Err(Code::State); };
        if offset != transfer.offset { return Err(Code::Offset); }
        if offset as usize % WRITE_ALIGNMENT != 0 { return Err(Code::Alignment); }
        let length = u32::try_from(data.len()).map_err(|_| Code::Size)?;
        let end = offset.checked_add(length).ok_or(Code::Size)?;
        if end > transfer.size { return Err(Code::Size); }
        if end < transfer.size && data.len() % WRITE_ALIGNMENT != 0 { return Err(Code::Alignment); }

        let mut padded = AlignedWrite([0xff; 96]);
        let write_len = (data.len() + 3) & !3;
        if write_len > padded.0.len() { return Err(Code::Size); }
        padded.0[..data.len()].copy_from_slice(data);
        if slots.write(transfer.image, offset, &padded.0[..write_len]).await.is_err() {
            self.abort();
            return Err(Code::Flash);
        }
        transfer.offset = end;
        self.transfer = Some(transfer);
        if end != transfer.size { return Ok(Action::Continue); }

        let mut crc = Crc32::new();
        let mut read_offset = 0;
        let mut bytes = [0; VERIFY_CHUNK];
        while read_offset < transfer.size {
            let count = (transfer.size - read_offset).min(VERIFY_CHUNK as u32) as usize;
            if slots.read(transfer.image, read_offset, &mut bytes[..count]).await.is_err() {
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

    async fn commit(&mut self, slots: &mut impl Slots) -> Result<Action, Code> {
        if self.transfer.is_some() { return Err(Code::Busy); }
        if self.app_size.is_none() && self.netcore_size.is_none() { return Err(Code::State); }
        if slots.commit(self.app_size, self.netcore_size).await.is_err() {
            self.abort();
            return Err(Code::Flash);
        }
        self.app_size = None;
        self.netcore_size = None;
        Ok(Action::Reset)
    }

    fn staged_size(&self, image: Image) -> Option<u32> {
        match image { Image::Application => self.app_size, Image::NetCore => self.netcore_size }
    }

    fn fail<T>(&mut self, error: Code) -> Result<T, Code> {
        self.error = error;
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Receiver, Slots};
    use odyssey_protocol::ota::{self, Code, Image, Request};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

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
            match image { Image::Application => &mut self.app, Image::NetCore => &mut self.netcore }
        }
    }
    impl Slots for Flash {
        fn capacity(&self, _: Image) -> u32 { self.capacity }
        async fn write(&mut self, image: Image, offset: u32, data: &[u8]) -> Result<(), Code> {
            assert_eq!(data.as_ptr() as usize % 4, 0);
            if self.fail_write { return Err(Code::Flash); }
            self.writes.push((image, offset, data.to_vec()));
            let start = offset as usize;
            self.region(image)[start..start + data.len()].copy_from_slice(data);
            Ok(())
        }
        async fn read(&mut self, image: Image, offset: u32, data: &mut [u8]) -> Result<(), Code> {
            if self.fail_read { return Err(Code::Flash); }
            let start = offset as usize;
            data.copy_from_slice(&self.region(image)[start..start + data.len()]);
            Ok(())
        }
        async fn valid(&mut self, _: Image, _: u32) -> Result<(), Code> {
            if self.fail_valid { Err(Code::Image) } else { Ok(()) }
        }
        async fn commit(&mut self, app: Option<u32>, netcore: Option<u32>) -> Result<(), Code> {
            if self.fail_commit { return Err(Code::Flash); }
            self.committed = Some((app, netcore));
            Ok(())
        }
    }
    struct Noop;
    impl Wake for Noop { fn wake(self: Arc<Self>) {} }
    fn run<F: core::future::Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) { Poll::Ready(value) => value, Poll::Pending => panic!("test future must be ready") }
    }
    fn request(receiver: &mut Receiver, flash: &mut Flash, request: Request<'_>) -> Result<Action, Code> {
        let mut frame = [0; ota::MAX_FRAME];
        let used = request.encode(&mut frame).expect("request fits frame");
        run(receiver.handle(flash, &frame[..used]))
    }
    fn transfer(receiver: &mut Receiver, flash: &mut Flash, image: Image, bytes: &[u8]) -> Result<Action, Code> {
        request(receiver, flash, Request::Begin { image, size: bytes.len() as u32, crc32: ota::crc32(bytes) })?;
        request(receiver, flash, Request::Data { offset: 0, data: bytes })
    }


    #[test]
    fn begin_checks_aligned_capacity_and_size_overflow() {
        let mut receiver = Receiver::new(); let mut flash = Flash::new();
        let begin = |size| Request::Begin { image: Image::Application, size, crc32: 0 };
        flash.capacity = 11;
        assert_eq!(request(&mut receiver, &mut flash, begin(9)), Err(Code::Size));
        flash.capacity = 12;
        assert_eq!(request(&mut receiver, &mut flash, begin(9)), Ok(Action::Continue));
        receiver.abort();
        assert_eq!(request(&mut receiver, &mut flash, begin(u32::MAX)), Err(Code::Size));
        assert_eq!(flash.writes.len(), 0);
    }
    #[test]
    fn stages_both_images_then_commits_once() {
        let mut receiver = Receiver::new(); let mut flash = Flash::new();
        let netcore = [1, 2, 3, 4, 5, 6, 7, 8]; let app = [9, 8, 7, 6, 5, 4, 3, 2];
        assert_eq!(transfer(&mut receiver, &mut flash, Image::NetCore, &netcore), Ok(Action::Continue));
        assert_eq!(transfer(&mut receiver, &mut flash, Image::Application, &app), Ok(Action::Continue));
        assert_eq!(receiver.status(1, 3, [0; 3], 0).staged, 3);
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Ok(Action::Reset));
        assert_eq!(flash.committed, Some((Some(8), Some(8))));
    }

    #[test]
    fn checksum_flash_and_vector_faults_discard_every_staged_image() {
        let mut receiver = Receiver::new(); let mut flash = Flash::new(); let bytes = [1, 2, 3, 4, 5, 6, 7, 8];
        transfer(&mut receiver, &mut flash, Image::NetCore, &bytes).unwrap();
        request(&mut receiver, &mut flash, Request::Begin { image: Image::Application, size: 8, crc32: 0 }).unwrap();
        assert_eq!(request(&mut receiver, &mut flash, Request::Data { offset: 0, data: &bytes }), Err(Code::Checksum));
        assert_eq!(receiver.status(1, 3, [0; 3], 0).staged, 0);
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Err(Code::State));
        flash.fail_write = true;
        assert_eq!(transfer(&mut receiver, &mut flash, Image::Application, &bytes), Err(Code::Flash));
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Err(Code::State));
        flash.fail_write = false; flash.fail_valid = true;
        assert_eq!(transfer(&mut receiver, &mut flash, Image::Application, &bytes), Err(Code::Image));
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Err(Code::State));
        assert_eq!(flash.committed, None);
    }

    #[test]
    fn truncation_order_and_overflow_never_activate() {
        let mut receiver = Receiver::new(); let mut flash = Flash::new();
        assert_eq!(run(receiver.handle(&mut flash, &[1, 0])), Err(Code::InvalidFrame));
        assert_eq!(request(&mut receiver, &mut flash, Request::Data { offset: 0, data: &[0; 8] }), Err(Code::State));
        request(&mut receiver, &mut flash, Request::Begin { image: Image::Application, size: 8, crc32: ota::crc32(&[0; 8]) }).unwrap();
        assert_eq!(request(&mut receiver, &mut flash, Request::Data { offset: 0, data: &[0; 9] }), Err(Code::Size));
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Err(Code::Busy));
        assert_eq!(flash.committed, None);
    }

    #[test]
    fn abort_and_disconnect_clear_partial_and_staged_state() {
        let mut receiver = Receiver::new(); let mut flash = Flash::new();
        request(&mut receiver, &mut flash, Request::Begin { image: Image::Application, size: 8, crc32: 0 }).unwrap();
        receiver.abort();
        assert_eq!(receiver.status(0, 3, [0; 3], 0).receiving, None);
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Err(Code::State));
        transfer(&mut receiver, &mut flash, Image::Application, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        request(&mut receiver, &mut flash, Request::Abort).unwrap();
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Err(Code::State));
        assert_eq!(flash.committed, None);
    }

    #[test]
    fn nonword_tail_gets_aligned_padding_but_checksum_uses_image_bytes() {
        let mut receiver = Receiver::new(); let mut flash = Flash::new(); let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        transfer(&mut receiver, &mut flash, Image::Application, &bytes).unwrap();
        assert_eq!(flash.writes[0].2, [1, 2, 3, 4, 5, 6, 7, 8, 9, 0xff, 0xff, 0xff]);
        assert_eq!(ota::crc32(&flash.app[..9]), ota::crc32(&bytes));
        assert_eq!(request(&mut receiver, &mut flash, Request::Commit), Ok(Action::Reset));
    }
}
