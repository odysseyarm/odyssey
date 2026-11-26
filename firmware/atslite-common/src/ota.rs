//! nRF5340 OTA flash backend for lite1 and legacy-atslite.
//!
//! [`ota_channel`] owns the transport-independent receiver task and request/reply channels.
//!
//! [`FlashSlots`] implements `better_ota::Slots`. The application goes into the bank that is not
//! running, the net core into a staging region. `commit` writes the boot-state page through
//! `better_boot::write_ota_state`; `firmware/lite1-bootloader` and
//! `firmware/legacy-atslite-bootloader` read it on the next reset. `firmware/better-ota/src/lib.rs`
//! describes the whole flow.

use better_ota::Slots;
use odyssey_protocol::ota::{Code, Image};
use release_signing::Component;

/// nRF5340 flash page, the erase unit.
const PAGE_SIZE: u32 = 4096;
/// Board the firmware runs on. Selects the signature component id, so a signed image for one
/// product does not install on the other.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Product {
    Lite1,
    LegacyAtslite,
}
const fn image_prefix(product: Product) -> u32 {
    if matches!(product, Product::Lite1) {
        512
    } else {
        0
    }
}

/// Flash and memory map the update works on. Filled from linker symbols by the board's `main.rs`.
#[expect(
    missing_debug_implementations,
    reason = "Carries peripheral ownership and linker ranges"
)]
pub struct Layout {
    /// Shared with boot confirmation and DFU, which use the same NVMC.
    pub flash: &'static embassy_sync::blocking_mutex::Mutex<
        embassy_sync::blocking_mutex::raw::NoopRawMutex,
        core::cell::RefCell<embassy_nrf::nvmc::Nvmc<'static>>,
    >,
    /// Flash ranges of application bank A and bank B.
    pub app_banks: [core::ops::Range<u32>; 2],
    /// Index into `app_banks` of the bank this firmware executes from.
    pub running_app: u8,
    /// RAM a valid application initial stack pointer points into.
    pub app_ram: core::ops::Range<u32>,
    /// Flash region the net core image is written to before the bootloader installs it.
    pub netcore_stage: core::ops::Range<u32>,
    /// Flash range the net core executes from, after installation. Bounds its reset vector.
    pub netcore_active: core::ops::Range<u32>,
    /// RAM a valid net core initial stack pointer points into.
    pub netcore_ram: core::ops::Range<u32>,
    /// Address of the boot-state page that `better_boot::write_ota_state` rewrites.
    pub boot_state: u32,
    /// Start of the separate two-page Embassy direct-XIP journal.
    pub boot_journal: u32,
    /// Product this firmware is built for.
    pub product: Product,
}

#[expect(
    missing_debug_implementations,
    reason = "Owns the OTA flash slot writer"
)]
pub struct FlashSlots {
    layout: Layout,
    target_app: u8,
    erased_until: [u32; 2],
    operational_verifier: Option<
        fn(Component, u32, [u32; 3], &[u8; 32], &[u8; release_signing::SIGNATURE_LEN]) -> bool,
    >,
}

impl FlashSlots {
    pub const fn new(layout: Layout) -> Self {
        Self::new_with_verifier(layout, None)
    }

    pub const fn new_with_verifier(
        layout: Layout,
        operational_verifier: Option<
            fn(Component, u32, [u32; 3], &[u8; 32], &[u8; release_signing::SIGNATURE_LEN]) -> bool,
        >,
    ) -> Self {
        Self {
            target_app: 1 - layout.running_app,
            layout,
            erased_until: [0; 2],
            operational_verifier,
        }
    }

    /// Slot number reported in the OTA status: 1 for bank A, 2 for bank B.
    pub const fn app_slot(&self) -> u8 {
        self.target_app + 1
    }

    fn region(&self, image: Image) -> (u32, u32) {
        match image {
            Image::Application => {
                let region = &self.layout.app_banks[self.target_app as usize];
                (region.start, region.end)
            }
            Image::NetCore => {
                // One page of each region is held back; the image must fit both the staging
                // region it is written to and the active region it is installed into.
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
    fn prefix(&self) -> u32 {
        image_prefix(self.layout.product)
    }
}

impl Slots for FlashSlots {
    fn capacity(&self, image: Image) -> u32 {
        let (start, end) = self.region(image);
        end.saturating_sub(start).saturating_sub(self.prefix())
    }

    async fn write(&mut self, image: Image, offset: u32, data: &[u8]) -> Result<(), Code> {
        use embedded_storage::nor_flash::NorFlash;

        let index = image as usize;
        let (base, end) = self.region(image);
        let prefix = self.prefix();
        let len = u32::try_from(data.len()).map_err(|_| Code::Size)?;
        let limit = end.checked_sub(base).ok_or(Code::Size)?;
        let write_end = offset.checked_add(len).ok_or(Code::Size)?;
        if write_end > limit.saturating_sub(prefix) {
            return Err(Code::Size);
        }
        if offset == 0 {
            self.erased_until[index] = 0;
        }
        let physical_end = prefix.checked_add(write_end).ok_or(Code::Size)?;
        let erase_end =
            physical_end.checked_add(PAGE_SIZE - 1).ok_or(Code::Size)? & !(PAGE_SIZE - 1);
        let erase_start = self.erased_until[index];
        self.layout.flash.lock(|cell| {
            let mut flash = cell.borrow_mut();
            if let Image::Application = image {
                let manager = better_boot::BootStateManager::new(
                    self.layout.boot_state,
                    self.layout.boot_journal,
                );
                if manager.update_slot(&mut *flash).map_err(|_| Code::Busy)? != self.target_app {
                    return Err(Code::Busy);
                }
            }
            if erase_start < erase_end {
                flash
                    .erase(base + erase_start, base + erase_end)
                    .map_err(|_| Code::Flash)?;
            }
            flash
                .write(base + prefix + offset, data)
                .map_err(|_| Code::Flash)
        })?;
        self.erased_until[index] = self.erased_until[index].max(erase_end);
        Ok(())
    }

    async fn read(&mut self, image: Image, offset: u32, data: &mut [u8]) -> Result<(), Code> {
        use embedded_storage::nor_flash::ReadNorFlash;

        let (base, end) = self.region(image);
        let prefix = self.prefix();
        let len = u32::try_from(data.len()).map_err(|_| Code::Size)?;
        let limit = end.checked_sub(base).ok_or(Code::Size)?;
        if offset
            .checked_add(len)
            .is_none_or(|read_end| read_end > limit.saturating_sub(prefix))
        {
            return Err(Code::Size);
        }
        self.layout.flash.lock(|cell| {
            cell.borrow_mut()
                .read(base + prefix + offset, data)
                .map_err(|_| Code::Flash)
        })
    }

    fn component(&self, image: Image) -> Component {
        match (self.layout.product, image) {
            (Product::Lite1, Image::Application) => Component::Lite1SecureApp,
            (Product::Lite1, Image::NetCore) => Component::Lite1SecureNetCore,
            (Product::LegacyAtslite, Image::Application) => Component::LegacyAtsliteApp,
            (Product::LegacyAtslite, Image::NetCore) => Component::LegacyAtsliteNetCore,
        }
    }
    #[cfg(feature = "signed-ota")]
    fn verify_signature(
        &self,
        component: Component,
        size: u32,
        version: [u32; 3],
        hash: &[u8; 32],
        signature: &[u8; release_signing::SIGNATURE_LEN],
    ) -> bool {
        if self.layout.product == Product::Lite1 {
            self.operational_verifier
                .is_some_and(|verify| verify(component, size, version, hash, signature))
        } else {
            let message = release_signing::message(component, size, version, hash);
            release_signing::verify(&message, signature)
        }
    }

    #[cfg(feature = "signed-ota")]
    async fn write_header(
        &mut self,
        image: Image,
        header: &[u8; release_signing::DFU_HEADER_LEN],
    ) -> Result<(), Code> {
        use embedded_storage::nor_flash::NorFlash;

        if self.layout.product == Product::LegacyAtslite {
            return Ok(());
        }
        let base = self.region(image).0;
        self.layout.flash.lock(|cell| {
            cell.borrow_mut()
                .write(base, header)
                .map_err(|_| Code::Flash)
        })
    }

    async fn valid(&mut self, image: Image, size: u32) -> Result<(), Code> {
        let mut vectors = [0; 8];
        self.read(image, 0, &mut vectors).await?;
        let sp = u32::from_le_bytes(vectors[..4].try_into().map_err(|_| Code::Image)?);
        let reset = u32::from_le_bytes(vectors[4..].try_into().map_err(|_| Code::Image)?);
        let (ram, execution) = match image {
            Image::Application => (
                &self.layout.app_ram,
                &self.layout.app_banks[self.target_app as usize],
            ),
            Image::NetCore => (&self.layout.netcore_ram, &self.layout.netcore_active),
        };
        let payload_start = execution
            .start
            .checked_add(self.prefix())
            .ok_or(Code::Image)?;
        let execution_end = payload_start.checked_add(size).ok_or(Code::Image)?;
        let reset_address = reset & !1;
        if sp < ram.start
            || sp > ram.end
            || reset & 1 == 0
            || reset_address < payload_start
            || reset_address >= execution_end
        {
            return Err(Code::Image);
        }
        Ok(())
    }

    async fn commit(
        &mut self,
        app_size: Option<u32>,
        netcore_size: Option<u32>,
    ) -> Result<(), Code> {
        let netcore = netcore_size
            .map(|size| {
                size.checked_next_multiple_of(4)
                    .and_then(|len| len.checked_add(self.prefix()))
                    .map(|len| (self.layout.netcore_stage.start, len))
                    .ok_or(Code::Image)
            })
            .transpose()?;
        self.layout.flash.lock(|cell| {
            better_boot::write_ota_state(
                self.layout.boot_state,
                self.layout.boot_journal,
                &mut *cell.borrow_mut(),
                app_size.map(|_| self.target_app),
                netcore,
                PAGE_SIZE,
            )
            .map_err(|_| Code::Flash)
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
    ota_channel::run(slots, app_slot, 3, firmware_version, netcore_release).await
}
#[cfg(test)]
mod tests {
    use super::{Product, image_prefix};

    #[test]
    fn signed_slot_prefix_applies_only_to_lite1() {
        assert_eq!(image_prefix(Product::Lite1), 512);
        assert_eq!(image_prefix(Product::LegacyAtslite), 0);
    }
}
