use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use better_ota::{Action, Receiver, Slots};
use odyssey_protocol::ota::{MAX_FRAME, STATUS_LEN};

const QUEUE: usize = 1;

static REQUESTS: Channel<ThreadModeRawMutex, Frame, QUEUE> = Channel::new();
static REPLIES: Channel<ThreadModeRawMutex, Reply, QUEUE> = Channel::new();

#[derive(Clone, Copy)]
struct Frame {
    len: u8,
    bytes: [u8; MAX_FRAME],
}

#[derive(Clone, Copy, Debug)]
pub struct Reply {
    pub status: [u8; STATUS_LEN],
    pub reset: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtaHandle;

impl OtaHandle {
    pub async fn request(&self, bytes: &[u8]) -> Option<Reply> {
        let Ok(len) = u8::try_from(bytes.len()) else {
            return None;
        };
        if bytes.len() > MAX_FRAME {
            return None;
        }
        let mut frame = Frame { len, bytes: [0; MAX_FRAME] };
        frame.bytes[..bytes.len()].copy_from_slice(bytes);
        REQUESTS.send(frame).await;
        Some(REPLIES.receive().await)
    }
}

pub async fn run(
    mut slots: impl Slots,
    app_slot: u8,
    supported: u8,
    firmware_version: [u32; 3],
    netcore_release: fn() -> u16,
) -> ! {
    let mut receiver = Receiver::new();
    loop {
        let frame = REQUESTS.receive().await;
        let input = &frame.bytes[..usize::from(frame.len)];
        let action = receiver.handle(&mut slots, input).await;
        let status = receiver.status(app_slot, supported, firmware_version, netcore_release());
        REPLIES.send(Reply {
            status: status.encode(),
            reset: matches!(action, Ok(Action::Reset)),
        }).await;
    }
}

const PAGE_SIZE: u32 = 4096;

#[expect(missing_debug_implementations, reason = "Carries peripheral ownership and linker ranges")]
pub struct Layout {
    pub flash: &'static embassy_sync::blocking_mutex::Mutex<
        embassy_sync::blocking_mutex::raw::NoopRawMutex,
        core::cell::RefCell<embassy_nrf::nvmc::Nvmc<'static>>,
    >,
    pub app_banks: [core::ops::Range<u32>; 2],
    pub running_app: u8,
    pub app_ram: core::ops::Range<u32>,
    pub netcore_stage: core::ops::Range<u32>,
    pub netcore_active: core::ops::Range<u32>,
    pub netcore_ram: core::ops::Range<u32>,
    pub boot_state: u32,
}

#[expect(missing_debug_implementations, reason = "Owns the OTA flash slot writer")]
pub struct FlashSlots {
    layout: Layout,
    target_app: u8,
    erased_until: [u32; 2],
}

impl FlashSlots {
    pub const fn new(layout: Layout) -> Self {
        Self {
            target_app: 1 - layout.running_app,
            layout,
            erased_until: [0; 2],
        }
    }

    pub const fn app_slot(&self) -> u8 {
        self.target_app + 1
    }

    fn region(&self, image: odyssey_protocol::ota::Image) -> (u32, u32) {
        match image {
            odyssey_protocol::ota::Image::Application => {
                let region = &self.layout.app_banks[self.target_app as usize];
                (region.start, region.end)
            }
            odyssey_protocol::ota::Image::NetCore => {
                let staging_capacity = self
                    .layout
                    .netcore_stage
                    .end
                    .saturating_sub(self.layout.netcore_stage.start)
                    .saturating_sub(PAGE_SIZE);
                let active_capacity = self
                    .layout
                    .netcore_active
                    .end
                    .saturating_sub(self.layout.netcore_active.start)
                    .saturating_sub(PAGE_SIZE);
                (
                    self.layout.netcore_stage.start,
                    self.layout.netcore_stage.start + staging_capacity.min(active_capacity),
                )
            }
        }
    }
}

impl Slots for FlashSlots {
    fn capacity(&self, image: odyssey_protocol::ota::Image) -> u32 {
        let (start, end) = self.region(image);
        end.saturating_sub(start)
    }

    async fn write(
        &mut self,
        image: odyssey_protocol::ota::Image,
        offset: u32,
        data: &[u8],
    ) -> Result<(), odyssey_protocol::ota::Code> {
        use embedded_storage::nor_flash::NorFlash;

        let index = image as usize;
        let (base, end) = self.region(image);
        let len = u32::try_from(data.len()).map_err(|_| odyssey_protocol::ota::Code::Size)?;
        let limit = end.checked_sub(base).ok_or(odyssey_protocol::ota::Code::Size)?;
        let write_end = offset.checked_add(len).ok_or(odyssey_protocol::ota::Code::Size)?;
        if write_end > limit {
            return Err(odyssey_protocol::ota::Code::Size);
        }
        if offset == 0 {
            self.erased_until[index] = 0;
        }
        let erase_end = write_end
            .checked_add(PAGE_SIZE - 1)
            .ok_or(odyssey_protocol::ota::Code::Size)?
            & !(PAGE_SIZE - 1);
        let erase_start = self.erased_until[index];
        self.layout.flash.lock(|cell| {
            let mut flash = cell.borrow_mut();
            if erase_start < erase_end {
                flash
                    .erase(base + erase_start, base + erase_end)
                    .map_err(|_| odyssey_protocol::ota::Code::Flash)?;
            }
            flash
                .write(base + offset, data)
                .map_err(|_| odyssey_protocol::ota::Code::Flash)
        })?;
        self.erased_until[index] = self.erased_until[index].max(erase_end);
        Ok(())
    }

    async fn read(
        &mut self,
        image: odyssey_protocol::ota::Image,
        offset: u32,
        data: &mut [u8],
    ) -> Result<(), odyssey_protocol::ota::Code> {
        use embedded_storage::nor_flash::ReadNorFlash;

        let (base, end) = self.region(image);
        let len = u32::try_from(data.len()).map_err(|_| odyssey_protocol::ota::Code::Size)?;
        let address = base
            .checked_add(offset)
            .ok_or(odyssey_protocol::ota::Code::Size)?;
        if address.checked_add(len).is_none_or(|read_end| read_end > end) {
            return Err(odyssey_protocol::ota::Code::Size);
        }
        self.layout.flash.lock(|cell| {
            cell.borrow_mut()
                .read(address, data)
                .map_err(|_| odyssey_protocol::ota::Code::Flash)
        })
    }

    async fn valid(
        &mut self,
        image: odyssey_protocol::ota::Image,
        size: u32,
    ) -> Result<(), odyssey_protocol::ota::Code> {
        let (base, _) = self.region(image);
        let mut vectors = [0; 8];
        self.read(image, 0, &mut vectors).await?;
        let sp = u32::from_le_bytes(
            vectors[..4]
                .try_into()
                .map_err(|_| odyssey_protocol::ota::Code::Image)?,
        );
        let reset = u32::from_le_bytes(
            vectors[4..]
                .try_into()
                .map_err(|_| odyssey_protocol::ota::Code::Image)?,
        );
        let (ram, execution) = match image {
            odyssey_protocol::ota::Image::Application => (
                &self.layout.app_ram,
                &self.layout.app_banks[self.target_app as usize],
            ),
            odyssey_protocol::ota::Image::NetCore => {
                (&self.layout.netcore_ram, &self.layout.netcore_active)
            }
        };
        let execution_end = execution
            .start
            .checked_add(size)
            .ok_or(odyssey_protocol::ota::Code::Image)?;
        let reset_address = reset & !1;
        if sp < ram.start
            || sp > ram.end
            || reset & 1 == 0
            || reset_address < execution.start
            || reset_address >= execution_end
            || (image == odyssey_protocol::ota::Image::Application && base != execution.start)
        {
            return Err(odyssey_protocol::ota::Code::Image);
        }
        Ok(())
    }

    async fn commit(
        &mut self,
        app_size: Option<u32>,
        netcore_size: Option<u32>,
    ) -> Result<(), odyssey_protocol::ota::Code> {
        let netcore = netcore_size.map(|size| (self.layout.netcore_stage.start, size));
        self.layout.flash.lock(|cell| {
            better_boot::write_ota_state(
                self.layout.boot_state,
                &mut *cell.borrow_mut(),
                app_size.map(|_| self.target_app),
                netcore,
                PAGE_SIZE,
            )
            .map_err(|_| odyssey_protocol::ota::Code::Flash)
        })
    }
}

#[embassy_executor::task]
pub async fn task(
    slots: FlashSlots,
    firmware_version: [u32; 3],
    netcore_release: fn() -> u16,
) -> ! {
    let app_slot = slots.app_slot();
    run(slots, app_slot, 3, firmware_version, netcore_release).await
}
