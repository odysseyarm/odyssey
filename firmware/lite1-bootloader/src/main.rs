#![no_std]
#![no_main]
#![expect(
    unsafe_code,
    reason = "Bootloader owns linker-defined flash, shared SRAM, and peripheral register access"
)]
mod dfu_timeout;

use core::cell::{Cell, RefCell};
use core::sync::atomic::Ordering;

use better_boot::{BootState, BootStateManager, STATE_MAGIC};
use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, DfuTarget, FirmwareWriter, ResetImmediate, usb_dfu};
use cortex_m_rt::exception;
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::peripherals;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Timer};
use embassy_usb::{Builder, msos};
use embedded_storage::nor_flash::NorFlash;
use panic_probe as _;

embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

use lite1_pcd::{PcdRegion, PcdStatusCode};

const DFU_ENTRY_TIMEOUT: Duration = Duration::from_secs(60);

// Set in high RAM by the application's DFU detach. Survives a soft reset.
#[repr(C)]
struct AppDfuFlag {
    magic: u32,
    dfu_requested: u32,
}

const DEVICE_INTERFACE_GUIDS: &[&str] = &["{9B7AD93A-0A2C-43BF-A0D9-B2C071C30F9F}"];

const VERSION: [u32; 3] = odyssey_version::parse_cargo_version(env!("CARGO_PKG_VERSION"));

// The linker script maps this section to the last bytes of the stage2 flash region,
// where the application reads the installed bootloader version.
#[used]
#[unsafe(link_section = ".bootloader_info")]
static BOOTLOADER_INFO: bootloader_info::BootloaderInfo =
    bootloader_info::BootloaderInfo::new(VERSION);
const BOOTLOADER_BCD_DEVICE: u16 = odyssey_version::firmware_bcd_device(VERSION[0]);
const SLOT_A: u8 = 0;
const SLOT_B: u8 = 1;
const APP_DFU_FLAG_MAGIC: u32 = 0xDF00B071;
const NETCORE_DFU_MAGIC: u32 = 0x4E45_5444; // "NETD"
const ERASED_WORD: u32 = 0xFFFF_FFFF;
include!(concat!(env!("OUT_DIR"), "/dfu_spu.rs"));

// PCD region in the app core's ICMSG RX buffer, shared with the netcore bootloader.
const PCD_REGION_ADDR: u32 = 0x20070000;

// Just below the PCD region, outside it.
const APP_DFU_FLAG_ADDR: u32 = 0x2006FF00;

fn get_pcd_region() -> &'static PcdRegion {
    // SAFETY: This fixed SRAM address is the linker-reserved PCD area shared with the netcore.
    unsafe { PcdRegion::at(PCD_REGION_ADDR as usize) }
}

fn get_app_dfu_flag() -> &'static mut AppDfuFlag {
    // SAFETY: This fixed SRAM address is reserved for the app DFU marker and only the bootloader accesses it.
    unsafe { &mut *(APP_DFU_FLAG_ADDR as *mut AppDfuFlag) }
}

/// Writes DFU images into flash: the inactive app bank, or the shared DFU region
/// for netcore and bootloader images.
struct DualBankFlashWriter<'d> {
    flash: &'d Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'d>>>,
    netcore_dfu_size: usize,
    signed_header: [u8; release_signing::DFU_HEADER_LEN],
    signed_size: usize,
}

impl<'d> DualBankFlashWriter<'d> {
    fn new(
        flash: &'d Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'d>>>,
    ) -> Self {
        Self {
            flash,
            netcore_dfu_size: 0,
            signed_header: [0xff; release_signing::DFU_HEADER_LEN],
            signed_size: 0,
        }
    }

    fn get_bank_range(&self, target: DfuTarget) -> (usize, usize) {
        // SAFETY: Linker script defines bank and DFU partition bounds in mapped flash.
        unsafe extern "C" {
            static __bootloader_app_a_start: u32;
            static __bootloader_app_a_end: u32;
            static __bootloader_app_b_start: u32;
            static __bootloader_app_b_end: u32;
            static __bootloader_dfu_region_start: u32;
            static __stage2_bootloader_start: u32;
            static __stage2_bootloader_end: u32;
            static __netcore_primary_start: u32;
            static __netcore_primary_end: u32;
        }

        // SAFETY: All addresses below are linker-defined flash partition boundaries.
        unsafe {
            match target {
                DfuTarget::AppBankA => (
                    &__bootloader_app_a_start as *const u32 as usize,
                    &__bootloader_app_a_end as *const u32 as usize,
                ),
                DfuTarget::AppBankB => (
                    &__bootloader_app_b_start as *const u32 as usize,
                    &__bootloader_app_b_end as *const u32 as usize,
                ),
                DfuTarget::NetCore => {
                    let start = &__bootloader_dfu_region_start as *const u32 as usize;
                    let end = start
                        + (&raw const __netcore_primary_end as usize
                            - &raw const __netcore_primary_start as usize);
                    (start, end)
                }
                DfuTarget::TrustList => {
                    let start = lite1_secure_boot::handoff::pending_record_address()
                        .expect("pending trust page missing")
                        as usize;
                    (start, start + PAGE_SIZE)
                }
                DfuTarget::Bootloader => {
                    // Stage1 swaps the bootloader in from here.
                    let start = &__bootloader_dfu_region_start as *const u32 as usize;
                    let end = start
                        + (&raw const __stage2_bootloader_end as usize
                            - &raw const __stage2_bootloader_start as usize);
                    (start, end)
                }
            }
        }
    }
    fn verify_completed(&self, target: DfuTarget) -> Result<(), ()> {
        if target == DfuTarget::TrustList || self.signed_size == 0 {
            return Err(());
        }
        let (start, end) = self.get_bank_range(target);
        let (logical_address, component) = match target {
            DfuTarget::AppBankA | DfuTarget::AppBankB => {
                (start as u32, release_signing::Component::Lite1SecureApp)
            }
            DfuTarget::NetCore => {
                unsafe extern "C" {
                    static __netcore_primary_start: u32;
                }
                (
                    &raw const __netcore_primary_start as u32,
                    release_signing::Component::Lite1SecureNetCore,
                )
            }
            DfuTarget::Bootloader => {
                unsafe extern "C" {
                    static __stage2_bootloader_start: u32;
                }
                (
                    &raw const __stage2_bootloader_start as u32,
                    release_signing::Component::Lite1SecureBootloader,
                )
            }
            DfuTarget::TrustList => return Err(()),
        };
        let slot_len = 512usize.checked_add(self.signed_size).ok_or(())?;
        if slot_len > end.saturating_sub(start) {
            return Err(());
        }
        let policy = lite1_secure_boot::handoff::current_policy().map_err(|_| ())?;
        // SAFETY: linker-defined slot contains the bounded completed image.
        let slot = unsafe { core::slice::from_raw_parts(start as *const u8, slot_len) };
        lite1_secure_boot::verify_slot(slot, logical_address, component, policy.keys())
            .map_err(|_| ())
    }
}

impl<'d> FirmwareWriter for DualBankFlashWriter<'d> {
    const WRITE_SIZE: usize = Nvmc::WRITE_SIZE;
    type Error = ();

    fn start_signed_download(
        &mut self,
        _target: DfuTarget,
        component: release_signing::Component,
        header: &[u8],
    ) -> Result<release_signing::DfuVerifier, better_dfu::consts::Status> {
        let policy = lite1_secure_boot::handoff::current_policy()
            .map_err(|_| better_dfu::consts::Status::ErrVerify)?;
        let keys = if _target == DfuTarget::TrustList {
            policy.root_keys()
        } else {
            policy.keys()
        };
        let verifier = release_signing::DfuVerifier::new_with_keys(component, header, keys)
            .map_err(|_| better_dfu::consts::Status::ErrVerify)?;
        self.signed_header.copy_from_slice(header);
        self.signed_size = verifier.image_size() as usize;
        if _target == DfuTarget::TrustList
            && (self.signed_size == 0 || self.signed_size > release_signing::POLICY_RECORD_MAX)
        {
            return Err(better_dfu::consts::Status::ErrAddress);
        }
        Ok(verifier)
    }

    fn commit_signed_header(
        &mut self,
        target: DfuTarget,
        header: &[u8; release_signing::DFU_HEADER_LEN],
    ) -> Result<(), ()> {
        let (start, _) = self.get_bank_range(target);
        if target == DfuTarget::TrustList {
            let record_len = self.signed_size;
            if record_len == 0 || record_len > release_signing::POLICY_RECORD_MAX {
                return Err(());
            }
            let (page_start, page_end) = self.get_bank_range(target);
            let page_len = release_signing::DFU_HEADER_LEN
                .checked_add(record_len)
                .ok_or(())?;
            if page_len > page_end.saturating_sub(page_start) {
                return Err(());
            }
            return self.flash.lock(|flash_cell| {
                let mut flash = flash_cell.borrow_mut();
                flash.write(page_start as u32, header).map_err(|_| ())
            });
        }
        self.flash.lock(|flash_cell| {
            flash_cell
                .borrow_mut()
                .write(start as u32, header)
                .map_err(|_| ())
        })
    }
    fn write_for_target(
        &mut self,
        target: DfuTarget,
        offset: usize,
        data: &[u8],
    ) -> Result<(), ()> {
        let (bank_start, bank_end) = self.get_bank_range(target);
        if bank_start == 0 {
            return Err(());
        }
        let bank_len = bank_end.saturating_sub(bank_start);
        let prefix = if target == DfuTarget::TrustList {
            release_signing::DFU_HEADER_LEN
        } else {
            512
        };
        let payload_capacity = bank_len.checked_sub(prefix).ok_or(())?;
        if offset > payload_capacity || data.len() > payload_capacity - offset {
            return Err(());
        }
        let write_addr = bank_start + prefix + offset;

        defmt::debug!(
            "Writing {} bytes to {:?} at offset {:#x}",
            data.len(),
            target,
            write_addr
        );

        let is_app = matches!(target, DfuTarget::AppBankA | DfuTarget::AppBankB);
        if offset == 0 && !is_app && target != DfuTarget::TrustList {
            clear_netcore_dfu_info(self.flash)?;
            unsafe extern "C" {
                static __stage1_state_start: u32;
            }
            let state = &raw const __stage1_state_start as u32;
            self.flash.lock(|flash_cell| {
                let mut flash = flash_cell.borrow_mut();
                flash
                    .erase(state, state + PAGE_SIZE as u32)
                    .map_err(|_| ())?;
                flash
                    .write(state, &better_boot::BOOT_MAGIC.to_le_bytes())
                    .map_err(|_| ())
            })?;
            let pcd = get_pcd_region();
            pcd.cmd.clear();
            pcd.status.clear();
        }
        self.flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();
            if matches!(target, DfuTarget::AppBankA | DfuTarget::AppBankB) {
                let requested_slot = if target == DfuTarget::AppBankA {
                    SLOT_A
                } else {
                    SLOT_B
                };
                if get_boot_state_manager()
                    .update_slot(&mut *flash)
                    .map_err(|_| ())?
                    != requested_slot
                {
                    return Err(());
                }
            }

            let page_start = if offset == 0 {
                bank_start
            } else {
                (write_addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
            };
            let page_end = (write_addr + data.len() + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

            for page_addr in (page_start..page_end).step_by(PAGE_SIZE) {
                if let Err(_) = flash.erase(page_addr as u32, (page_addr + PAGE_SIZE) as u32) {
                    defmt::error!("Failed to erase page at {:#x}", page_addr);
                    return Err(());
                }
            }

            if let Err(_) = flash.write(write_addr as u32, data) {
                defmt::error!("Failed to write at {:#x}", write_addr);
                return Err(());
            }

            Ok(())
        })?;

        if target == DfuTarget::NetCore {
            self.netcore_dfu_size = 512 + offset + data.len();
            let pcd = get_pcd_region();
            pcd.status.reserved[0].store(self.netcore_dfu_size as u32, Ordering::Release);
            pcd.status.reserved[1].store(0x4E_5450, Ordering::Release); // "NTP"
        }
        Ok(())
    }

    fn mark_updated_for_target(&mut self, target: DfuTarget) -> Result<(), ()> {
        defmt::debug!(
            "Marking {:?} as updated (direct XIP, no swap needed)",
            target
        );

        if target == DfuTarget::TrustList {
            return Ok(());
        }
        self.verify_completed(target)?;
        // The bank written last is preferred on the next boot.
        let target_slot = match target {
            DfuTarget::AppBankA => SLOT_A,
            DfuTarget::AppBankB => SLOT_B,
            DfuTarget::NetCore => {
                self.netcore_dfu_size = 512 + self.signed_size;
                defmt::debug!(
                    "Setting netcore DFU pending via PCD, size: {}",
                    self.netcore_dfu_size
                );

                // SAFETY: Linker script defines the shared DFU image partition.
                unsafe extern "C" {
                    static __bootloader_dfu_region_start: u32;
                }
                // SAFETY: The linker symbol points to the start of mapped DFU flash.
                let dfu_addr = unsafe { &__bootloader_dfu_region_start as *const u32 as u32 };

                // The RAM copy covers a soft reset.
                let pcd = get_pcd_region();
                pcd.status.clear();
                pcd.cmd.set_update(dfu_addr, self.netcore_dfu_size as u32);
                // The netcore copies whole pages, so the size is padded to a page.
                let padded = (self.netcore_dfu_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                pcd.status.reserved[0].store(padded as u32, Ordering::Release);
                pcd.status.reserved[1].store(0x4E_54_44, Ordering::Release); // "NTD"
                defmt::debug!(
                    "PCD command set in RAM: addr={:#x}, size={} (padded {})",
                    dfu_addr,
                    self.netcore_dfu_size,
                    padded
                );

                // The flash copy covers a hard reset, which USB DFU's WILL_DETACH causes.
                if let Err(_) =
                    persist_netcore_dfu_info(self.flash, dfu_addr, self.netcore_dfu_size as u32)
                {
                    defmt::warn!("Failed to persist netcore DFU info to flash");
                } else {
                    defmt::debug!("Netcore DFU info written to flash for persistence across reset");
                }

                return Ok(());
            }
            DfuTarget::Bootloader => {
                defmt::debug!("Bootloader DFU completed, marking for swap via stage1");
                // The new bootloader is already in the DFU region. SWAP_MAGIC in
                // stage1_state makes stage1 swap it in on the next boot.

                // SAFETY: Linker script defines the stage-one boot-state flash page.
                unsafe extern "C" {
                    static __stage1_state_start: u32;
                }
                // SAFETY: The linker symbol points to the stage-one state partition.
                let stage1_state_addr = unsafe { &__stage1_state_start as *const u32 as usize };

                self.flash.lock(|flash_cell| {
                    let mut flash = flash_cell.borrow_mut();

                    let page_start = stage1_state_addr & !(PAGE_SIZE - 1);
                    let page_end = page_start + PAGE_SIZE;

                    if let Err(_) = flash.erase(page_start as u32, page_end as u32) {
                        defmt::error!("Failed to erase stage1 state page");
                        return Err(());
                    }

                    if let Err(_) = flash.write(
                        stage1_state_addr as u32,
                        &better_boot::SWAP_MAGIC.to_le_bytes(),
                    ) {
                        defmt::error!("Failed to write SWAP_MAGIC to stage1 state");
                        return Err(());
                    }

                    defmt::debug!("Stage1 state marked for bootloader swap");
                    Ok(())
                })?;

                return Ok(());
            }
            DfuTarget::TrustList => return Err(()),
        };

        self.flash.lock(|flash_cell| {
            get_boot_state_manager()
                .mark_bank_updated(&mut *flash_cell.borrow_mut(), target_slot, PAGE_SIZE as u32)
                .map_err(|_| ())
        })
    }
}

fn stored_boot_state() -> BootState {
    // SAFETY: Linker script defines the boot-state flash partition.
    unsafe extern "C" {
        static __bootloader_state_start: u32;
    }
    let address = &raw const __bootloader_state_start as usize + size_of::<u32>();
    // SAFETY: The linker places the aligned BootState after the state partition magic.
    unsafe { core::ptr::read_volatile(address as *const BootState) }
}

fn valid_app_slot(start: usize, end: usize, keys: &[release_signing::PublicKey]) -> bool {
    if end <= start {
        return false;
    }
    // SAFETY: linker-defined app bank is readable mapped flash.
    let slot = unsafe { core::slice::from_raw_parts(start as *const u8, end - start) };
    lite1_secure_boot::verify_slot(
        slot,
        start as u32,
        release_signing::Component::Lite1SecureApp,
        keys,
    )
    .is_ok()
}

fn valid_resume_slot(slot: u8) -> bool {
    unsafe extern "C" {
        static __bootloader_app_a_start: u32;
        static __bootloader_app_a_end: u32;
        static __bootloader_app_b_start: u32;
        static __bootloader_app_b_end: u32;
    }
    let bank = match slot {
        SLOT_A => {
            (&raw const __bootloader_app_a_start as usize)
                ..(&raw const __bootloader_app_a_end as usize)
        }
        SLOT_B => {
            (&raw const __bootloader_app_b_start as usize)
                ..(&raw const __bootloader_app_b_end as usize)
        }
        _ => return false,
    };
    let Ok(policy) = lite1_secure_boot::handoff::current_policy() else {
        return false;
    };
    valid_app_slot(bank.start, bank.end, policy.keys())
}
fn valid_staged_netcore(address: u32, size: u32) -> bool {
    unsafe extern "C" {
        static __bootloader_dfu_region_start: u32;
        static __bootloader_dfu_region_end: u32;
        static __netcore_primary_start: u32;
    }
    let start = &raw const __bootloader_dfu_region_start as u32;
    let end = &raw const __bootloader_dfu_region_end as u32;
    if address != start || size < 512 || size > end.saturating_sub(start) {
        return false;
    }
    let Ok(policy) = lite1_secure_boot::handoff::current_policy() else {
        return false;
    };
    let Ok(len) = usize::try_from(size) else {
        return false;
    };
    // SAFETY: validated PCD address and size lie inside linker-defined DFU staging region.
    let slot = unsafe { core::slice::from_raw_parts(address as *const u8, len) };
    let Ok(verifier) = release_signing::DfuVerifier::new_with_keys(
        release_signing::Component::Lite1SecureNetCore,
        &slot[..release_signing::DFU_HEADER_LEN],
        policy.keys(),
    ) else {
        return false;
    };
    size == 512 + verifier.image_size()
        && lite1_secure_boot::verify_slot(
            slot,
            &raw const __netcore_primary_start as u32,
            release_signing::Component::Lite1SecureNetCore,
            policy.keys(),
        )
        .is_ok()
}

/// Selects a valid application bank and persists its boot decision.
///
/// Returns DFU recovery when no bank is bootable or journal persistence fails.
fn select_boot_bank(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> (Option<usize>, &'static str, BootState) {
    unsafe extern "C" {
        static __bootloader_app_a_start: u32;
        static __bootloader_app_a_end: u32;
        static __bootloader_app_b_start: u32;
        static __bootloader_app_b_end: u32;
    }

    let bank_a_start = &raw const __bootloader_app_a_start as usize;
    let bank_a_end = &raw const __bootloader_app_a_end as usize;
    let bank_b_start = &raw const __bootloader_app_b_start as usize;
    let bank_b_end = &raw const __bootloader_app_b_end as usize;
    let policy = lite1_secure_boot::handoff::current_policy().ok();
    let bank_a_valid = policy
        .as_ref()
        .is_some_and(|policy| valid_app_slot(bank_a_start, bank_a_end, policy.keys()));
    let bank_b_valid = policy
        .as_ref()
        .is_some_and(|policy| valid_app_slot(bank_b_start, bank_b_end, policy.keys()));

    defmt::debug!(
        "Bank A: addr={:#x}, signed-valid={}",
        bank_a_start,
        bank_a_valid
    );
    defmt::debug!(
        "Bank B: addr={:#x}, signed-valid={}",
        bank_b_start,
        bank_b_valid
    );

    let result = flash.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        get_boot_state_manager().prepare_boot(
            &mut *flash,
            [bank_a_valid, bank_b_valid],
            PAGE_SIZE as u32,
        )
    });
    let slot = match result {
        Ok(slot) => Some(slot),
        Err(_) => {
            defmt::error!("Failed to prepare boot; entering DFU recovery");
            None
        }
    };
    let state = read_boot_state(flash);
    let (offset, name) = match slot {
        Some(SLOT_A) => (Some(bank_a_start + 512), "Bank A"),
        Some(SLOT_B) => (Some(bank_b_start + 512), "Bank B"),
        Some(_) => unreachable!(),
        None => {
            defmt::warn!("No bootable bank; entering DFU recovery");
            if state.active_slot == SLOT_B {
                (None, "Bank B (DFU)")
            } else {
                (None, "Bank A (DFU)")
            }
        }
    };
    (offset, name, state)
}

/// Whether the state partition requests DFU. Its first word is 0xFFFFFFFF
/// (erased) or 0xD0D0D0D0 (BOOT_MAGIC) for a normal boot and 0xF0F0F0F0
/// (SWAP_MAGIC) for DFU.
fn check_dfu_state(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> bool {
    // SAFETY: Linker script defines the boot-state flash partition.
    unsafe extern "C" {
        static __bootloader_state_start: u32;
    }

    // SAFETY: Linker script defines the boot-state flash partition.
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };

    flash.lock(|flash_cell| {
        let _flash = flash_cell.borrow();
        // SAFETY: The state word is linker-defined, aligned, and readable flash.
        let state_word = unsafe { core::ptr::read_volatile(state_addr as *const u32) };

        state_word == better_boot::SWAP_MAGIC
    })
}

fn get_boot_state_manager() -> BootStateManager {
    unsafe extern "C" {
        static __bootloader_state_start: u32;
        static __bootloader_journal_start: u32;
    }
    // SAFETY: Linker symbols identify aligned state and journal flash regions.
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as u32 };
    let journal_addr = unsafe { &__bootloader_journal_start as *const u32 as u32 };
    BootStateManager::new(state_addr, journal_addr)
}

fn read_boot_state(
    _flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> BootState {
    get_boot_state_manager().read()
}

fn write_boot_state(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    state: &BootState,
) -> Result<(), ()> {
    let manager = get_boot_state_manager();
    flash.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        manager
            .write_boot_state(&mut *flash, state, PAGE_SIZE as u32)
            .map_err(|_| ())
    })
}

fn snapshot_state_words() -> (u32, BootState) {
    let manager = get_boot_state_manager();
    let flag_word = manager.read_magic();
    let state = manager.read();
    (flag_word, state)
}

fn rewrite_state_page_with_netcore(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    flag_word: u32,
    state: BootState,
    netcore_info: Option<(u32, u32)>,
    attempts: u32,
) -> Result<(), ()> {
    // SAFETY: Linker script defines the boot-state flash partition.
    unsafe extern "C" {
        static __bootloader_state_start: u32;
    }

    // SAFETY: The linker symbol points to the boot-state flash partition.
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };

    flash.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        let page_start = state_addr & !(PAGE_SIZE - 1);
        let page_end = page_start + PAGE_SIZE;

        flash
            .erase(page_start as u32, page_end as u32)
            .map_err(|_| ())?;

        let flag_bytes = flag_word.to_le_bytes();
        flash
            .write(state_addr as u32, &flag_bytes)
            .map_err(|_| ())?;

        // SAFETY: `state` is initialized and remains live for this exact byte slice length.
        let state_bytes = unsafe {
            core::slice::from_raw_parts(
                &state as *const BootState as *const u8,
                core::mem::size_of::<BootState>(),
            )
        };
        flash
            .write(
                (state_addr + core::mem::size_of::<u32>()) as u32,
                state_bytes,
            )
            .map_err(|_| ())?;

        let (magic, addr, size) = if let Some((addr, size)) = netcore_info {
            (NETCORE_DFU_MAGIC, addr, size)
        } else {
            (ERASED_WORD, ERASED_WORD, ERASED_WORD)
        };

        flash
            .write((state_addr + 12) as u32, &magic.to_le_bytes())
            .map_err(|_| ())?;
        flash
            .write((state_addr + 16) as u32, &addr.to_le_bytes())
            .map_err(|_| ())?;
        flash
            .write((state_addr + 20) as u32, &size.to_le_bytes())
            .map_err(|_| ())?;
        flash
            .write((state_addr + 24) as u32, &attempts.to_le_bytes())
            .map_err(|_| ())?;

        Ok(())
    })
}

fn clear_netcore_dfu_info(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> Result<(), ()> {
    let (flag_word, state) = snapshot_state_words();
    rewrite_state_page_with_netcore(flash, flag_word, state, None, ERASED_WORD)
}

fn persist_netcore_dfu_info(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    dfu_addr: u32,
    dfu_size: u32,
) -> Result<(), ()> {
    let (flag_word, state) = snapshot_state_words();
    rewrite_state_page_with_netcore(
        flash,
        flag_word,
        state,
        Some((dfu_addr, dfu_size)),
        ERASED_WORD,
    )
}

/// Records how many times the copy has been started for the persisted command.
fn persist_netcore_copy_attempt(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    dfu_addr: u32,
    dfu_size: u32,
    attempts: u32,
) -> Result<(), ()> {
    let (flag_word, state) = snapshot_state_words();
    rewrite_state_page_with_netcore(
        flash,
        flag_word,
        state,
        Some((dfu_addr, dfu_size)),
        attempts,
    )
}

/// Writes SWAP_MAGIC to the state word so the next boot enters DFU.
fn flag_dfu_requested(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    preserved_state: BootState,
) {
    // The RAM flag gets the next boot into DFU even if the flash write fails.
    let app_dfu_flag = get_app_dfu_flag();
    app_dfu_flag.magic = APP_DFU_FLAG_MAGIC;
    app_dfu_flag.dfu_requested = 1;

    // SAFETY: Linker script defines the boot-state flash partition.
    unsafe extern "C" {
        static __bootloader_state_start: u32;
    }
    // SAFETY: The linker symbol points to the boot-state flash partition.
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };

    flash.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        // The boot state after the flag word is kept: the one chosen for this
        // boot, or what flash holds.
        let state_ptr = state_addr + core::mem::size_of::<u32>();
        let mut current_state = preserved_state;
        // SAFETY: State address is linker-defined, aligned, and readable while the flash mutex is held.
        let flash_state = unsafe { core::ptr::read_volatile(state_ptr as *const BootState) };
        if flash_state.magic == STATE_MAGIC {
            current_state = flash_state;
        }
        if current_state.magic != STATE_MAGIC {
            defmt::warn!(
                "State magic invalid when flagging DFU; RAM flag set, skipping flash update"
            );
            return;
        }

        let page_start = state_addr & !(PAGE_SIZE - 1);
        let page_end = page_start + PAGE_SIZE;

        if let Err(_) = flash.erase(page_start as u32, page_end as u32) {
            defmt::warn!("Failed to erase state page at {:#x}", page_start);
            return;
        }

        if let Err(_) = flash.write(state_addr as u32, &better_boot::SWAP_MAGIC.to_le_bytes()) {
            defmt::warn!("Failed to write SWAP_MAGIC at {:#x}", state_addr);
            return;
        }

        // SAFETY: `current_state` is initialized and remains live for this exact byte slice length.
        let state_bytes = unsafe {
            core::slice::from_raw_parts(
                &current_state as *const BootState as *const u8,
                core::mem::size_of::<BootState>(),
            )
        };
        if let Err(_) = flash.write((state_addr + 4) as u32, state_bytes) {
            defmt::warn!("Failed to rewrite boot state at {:#x}", state_addr + 4);
            return;
        }

        // SAFETY: State address is linker-defined and aligned; flash write completed before read-back.
        let verify_flag = unsafe { core::ptr::read_volatile(state_addr as *const u32) };

        defmt::debug!(
            "Flagged DFU requested (SWAP_MAGIC) at {:#x} (read-back={:#010x}); preserved state active={}, pending={}, attempts={}",
            state_addr,
            verify_flag,
            current_state.active_slot,
            current_state.pending_slot,
            current_state.boot_counter
        );

        // Flash writes must complete before the reset.
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    });
}

#[embassy_executor::task]
async fn watchdog_feeder(mut wdt_handle: embassy_nrf::wdt::WatchdogHandle) {
    use embassy_time::{Duration, Timer};

    defmt::debug!("Watchdog feeder task started (feeding every 30s)");

    loop {
        wdt_handle.pet();
        Timer::after(Duration::from_secs(30)).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    let p = embassy_nrf::init(Default::default());

    defmt::info!("Starting bootloader");

    // 60 s covers DFU operations. The watchdog keeps running in the app, which
    // must feed it.
    let mut wdt_config = embassy_nrf::wdt::Config::default();
    wdt_config.timeout_ticks = 32768 * 60; // 32.768 kHz ticks
    wdt_config.action_during_sleep = embassy_nrf::wdt::SleepConfig::RUN;
    wdt_config.action_during_debug_halt = embassy_nrf::wdt::HaltConfig::PAUSE;
    let (_wdt, [wdt_handle]) = match embassy_nrf::wdt::Watchdog::try_new(p.WDT0, wdt_config) {
        Ok(wdt) => wdt,
        Err(_) => {
            defmt::error!("Failed to initialize watchdog");
            cortex_m::peripheral::SCB::sys_reset();
        }
    };

    spawner
        .spawn(watchdog_feeder(wdt_handle))
        .expect("Failed to spawn watchdog feeder");

    let flash = Mutex::new(RefCell::new(Nvmc::new(p.NVMC)));

    // Read any status the netcore bootloader left, then restore the RAM command from flash.
    let pcd = get_pcd_region();
    let netcore_recovery = pcd.cmd.is_valid() || pcd.status.is_valid();

    if pcd.status.is_valid() {
        if let Some(status) = pcd.status.get_status() {
            defmt::debug!("PCD status from previous net DFU: {}", status as u32);
            if matches!(status, PcdStatusCode::Success | PcdStatusCode::AppConfirmed) {
                if let Err(_) = clear_netcore_dfu_info(&flash) {
                    defmt::warn!("Failed to clear persisted netcore DFU info after status");
                }
                pcd.cmd.clear();
                pcd.status.clear();
            } else if matches!(
                status,
                PcdStatusCode::FailedCopy | PcdStatusCode::FailedVerify
            ) {
                pcd.status.clear();
            }
        }
    }

    // The RAM command is lost on a hard reset. Flash keeps a copy.
    if !pcd.cmd.is_valid() {
        // SAFETY: State page layout is linker-defined; these words were persisted by the same bootloader.
        unsafe {
            unsafe extern "C" {
                static __bootloader_state_start: u32;
            }
            let state_addr = &__bootloader_state_start as *const u32 as usize;
            let netcore_magic = core::ptr::read_volatile((state_addr + 12) as *const u32);

            if netcore_magic == NETCORE_DFU_MAGIC {
                let dfu_addr = core::ptr::read_volatile((state_addr + 16) as *const u32);
                let dfu_size = core::ptr::read_volatile((state_addr + 20) as *const u32);

                defmt::debug!(
                    "Found netcore DFU in flash: addr={:#x}, size={}",
                    dfu_addr,
                    dfu_size
                );
                pcd.status.clear();
                pcd.cmd.set_update(dfu_addr, dfu_size);
            }
        }
    }

    defmt::debug!(
        "PCD region: magic={:#x}, addr={:#x}, size={}",
        // SAFETY: `pcd` points to valid shared SRAM and the field is an aligned atomic word.
        unsafe { core::ptr::read_volatile(&pcd.cmd.magic as *const _ as *const u32) },
        pcd.cmd.get_image_addr(),
        pcd.cmd.get_image_size()
    );

    let netcore_was_pending = netcore_recovery || pcd.copy_pending();

    // A command nothing has answered yet is pending, whether or not the net core
    // bootloader has written a status. Terminal statuses were settled above.
    let mut netcore_dfu_pending = pcd.copy_pending();

    if netcore_dfu_pending {
        defmt::debug!("PCD command VALID - will start netcore bootloader");
    } else {
        defmt::debug!("PCD command INVALID - skipping netcore DFU");
    }

    // A copy that never reports back is retried a few times, then abandoned: the
    // pending command survives power cycles, so an unbounded retry would trap the
    // unit before DFU. Giving up enters DFU so the update can be run again.
    let mut netcore_gave_up = false;
    if netcore_dfu_pending
        && !valid_staged_netcore(pcd.cmd.get_image_addr(), pcd.cmd.get_image_size())
    {
        defmt::error!("Staged network slot failed authentication; entering USB recovery");
        let _ = clear_netcore_dfu_info(&flash);
        pcd.cmd.clear();
        pcd.status.clear();
        netcore_dfu_pending = false;
        netcore_gave_up = true;
    }
    if netcore_dfu_pending {
        // SAFETY: State page layout is linker-defined; attempt count is the persisted word at offset 24.
        let stored_attempts = unsafe {
            unsafe extern "C" {
                static __bootloader_state_start: u32;
            }
            let state_addr = &__bootloader_state_start as *const u32 as usize;
            core::ptr::read_volatile((state_addr + 24) as *const u32)
        };
        match lite1_pcd::next_copy_attempt(stored_attempts) {
            Some(attempt) => {
                defmt::debug!(
                    "Net core copy attempt {}/{}",
                    attempt,
                    lite1_pcd::MAX_COPY_ATTEMPTS
                );
                if persist_netcore_copy_attempt(
                    &flash,
                    pcd.cmd.get_image_addr(),
                    pcd.cmd.get_image_size(),
                    attempt,
                )
                .is_err()
                {
                    defmt::warn!("Failed to persist the net core copy attempt count");
                }
            }
            None => {
                defmt::error!(
                    "Net core copy abandoned after {} attempts",
                    lite1_pcd::MAX_COPY_ATTEMPTS
                );
                if clear_netcore_dfu_info(&flash).is_err() {
                    defmt::warn!("Failed to clear persisted netcore DFU info");
                }
                pcd.cmd.clear();
                pcd.status.clear();
                netcore_dfu_pending = false;
                netcore_gave_up = true;
            }
        }
    }

    // Run the netcore bootloader for a pending update.
    if netcore_dfu_pending {
        defmt::debug!("Network core DFU pending, starting network core bootloader");
        use embassy_nrf::pac::reset::vals::Forceoff;
        use embassy_nrf::pac::spu::vals;

        // SAFETY: This bootloader owns SPU configuration before releasing the network core.
        unsafe {
            let spu = embassy_nrf::pac::SPU_S;

            // The network core is a non-secure bus master. Only the 16 KiB SPU
            // regions covering DFU staging become non-secure; stage1, stage2 and
            // both app banks stay secure and unreachable from it.
            for i in DFU_SPU_REGIONS {
                spu.flashregion(i).perm().write(|w| {
                    w.set_read(true);
                    w.set_write(true);
                    w.set_execute(true);
                    w.set_secattr(false);
                    w.set_lock(false);
                });
            }

            spu.extdomain(0).perm().write(|w| {
                w.set_securemapping(vals::ExtdomainPermSecuremapping::NON_SECURE);
                w.set_lock(false);
            });

            // ICMSG SRAM must be reachable from the netcore.
            const RAM_REGION_SIZE: usize = 8 * 1024;
            const RAM_BASE: u32 = 0x20000000;
            let icmsg_rx_start_region = (0x20070000 - RAM_BASE) as usize / RAM_REGION_SIZE;
            let icmsg_rx_end_region = (0x20072000 - RAM_BASE) as usize / RAM_REGION_SIZE;
            let icmsg_tx_start_region = (0x20078000 - RAM_BASE) as usize / RAM_REGION_SIZE;
            let icmsg_tx_end_region = (0x2007A000 - RAM_BASE) as usize / RAM_REGION_SIZE;

            for i in icmsg_rx_start_region..icmsg_rx_end_region {
                spu.ramregion(i).perm().write(|w| {
                    w.set_read(true);
                    w.set_write(true);
                    w.set_execute(false);
                    w.set_secattr(false);
                    w.set_lock(false);
                });
            }
            for i in icmsg_tx_start_region..icmsg_tx_end_region {
                spu.ramregion(i).perm().write(|w| {
                    w.set_read(true);
                    w.set_write(true);
                    w.set_execute(false);
                    w.set_secattr(false);
                    w.set_lock(false);
                });
            }

            cortex_m::asm::dsb();
            cortex_m::asm::isb();

            (0x50005618 as *mut u32).write_volatile(1);
            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::RELEASE));
            cortex_m::asm::delay(5 * 64);
            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::HOLD));
            cortex_m::asm::delay(1 * 64);
            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::RELEASE));
            (0x50005618 as *mut u32).write_volatile(0);
        }
        defmt::debug!("Network core bootloader started");

        // The net core reports its own copy failures, so this only bounds a net
        // core that never answers. shortcut: 30 s assumes a ~10 s copy
        // (estimated, not measured), raise it if a good install is cut off.
        let poll = async {
            let mut ticks: u32 = 0;
            const MAX_TICKS: u32 = 300; // 30 s at 100 ms per tick
            loop {
                // The status word survives a reset while its magic does not, so a
                // stale terminal code from an earlier update must not end the wait.
                let status = if pcd.status.is_valid() {
                    pcd.status.get_status()
                } else {
                    None
                };
                if let Some(status) = status {
                    defmt::debug!("Netcore DFU status from net bootloader: {}", status as u32);
                    if status == PcdStatusCode::Success {
                        if let Err(_) = clear_netcore_dfu_info(&flash) {
                            defmt::warn!("Failed to clear persisted netcore DFU info after status");
                        }
                        pcd.cmd.clear();
                        pcd.status.clear();
                        netcore_dfu_pending = false;
                        break;
                    }
                    if matches!(
                        status,
                        PcdStatusCode::FailedCopy | PcdStatusCode::FailedVerify
                    ) {
                        // The destination may be half-written. The staged image and command stay
                        // so the next boot retries; the attempt limit ends in DFU.
                        defmt::error!("Netcore copy failed - resetting to retry");
                        pcd.status.clear();
                        cortex_m::peripheral::SCB::sys_reset();
                    }
                }

                // No answer in time: reset and retry on the next boot.
                if ticks >= MAX_TICKS {
                    defmt::error!(
                        "Netcore DFU taking too long (>30 s) - resetting to retry on next boot"
                    );
                    cortex_m::peripheral::SCB::sys_reset();
                }

                Timer::after(Duration::from_millis(100)).await;
                ticks += 1;
            }
        };
        poll.await;
    }

    // The runtime's DFU detach sets a flag in locked RAM to force DFU.
    let mut ram_dfu_request = false;
    let app_dfu_flag = get_app_dfu_flag();
    if app_dfu_flag.magic == APP_DFU_FLAG_MAGIC && app_dfu_flag.dfu_requested != 0 {
        ram_dfu_request = true;
        app_dfu_flag.dfu_requested = 0;
        defmt::debug!("DFU requested via RAM flag");
    }

    // A pending netcore update takes precedence over app DFU, even with SWAP_MAGIC set.
    let mut should_enter_dfu = ram_dfu_request || check_dfu_state(&flash) || netcore_gave_up;
    if netcore_dfu_pending && should_enter_dfu {
        defmt::debug!("Clearing app DFU request because netcore DFU is pending");
        should_enter_dfu = false;
        // Clear SWAP_MAGIC, otherwise every boot lands in DFU.
        let mut state = read_boot_state(&flash);
        state.boot_counter = 0;
        let _ = write_boot_state(&flash, &state);
    }
    defmt::debug!(
        "DFU decision: should_enter_dfu={} netcore_dfu_pending={}",
        should_enter_dfu,
        netcore_dfu_pending
    );

    // Judged on the state as stored at entry, before `select_boot_bank` counts a
    // trial boot or rolls a slot back. A DFU request that nobody answers within
    // DFU_ENTRY_TIMEOUT may only fall back to an application that was already
    // confirmed (not on trial), whose own vectors are still intact. The net core
    // being mid-update or having been given up on rules this out.
    let entry_state = stored_boot_state();
    let confirmed_application = entry_state.is_valid()
        && entry_state.active_slot == entry_state.pending_slot
        && entry_state.boot_counter == 0
        && matches!(entry_state.active_slot, SLOT_A | SLOT_B);
    let resume_after_timeout = should_enter_dfu
        && confirmed_application
        && !netcore_was_pending
        && !netcore_gave_up
        && valid_resume_slot(entry_state.active_slot);

    let (boot_offset, boot_bank_name, boot_state) = select_boot_bank(&flash);
    if boot_offset.is_none() {
        should_enter_dfu = true;
    }

    // Stage1 swaps a new bootloader in on trial and puts the old one back at the
    // next reset unless the new one confirms it booted. Without that, a bootloader
    // update lasts until the first reset, which is the next thing the host does.
    //
    // Erasing the stage1 state page has disturbed the neighbouring stage2 state
    // page on this part, so the write happens only when stage1 is waiting for the
    // confirmation (once per bootloader update, never on an ordinary boot), and
    // stage2's own state page is rewritten afterwards to refresh its cells.
    {
        // SAFETY: Linker script defines the stage-one state page in mapped flash.
        unsafe extern "C" {
            static __stage1_state_start: u32;
        }
        // SAFETY: Linker symbol points to aligned stage-one state flash.
        let stage1_state_addr = unsafe { &__stage1_state_start as *const u32 as usize };
        // SAFETY: Stage-one state address is aligned readable flash.
        let stage1_magic = unsafe { core::ptr::read_volatile(stage1_state_addr as *const u32) };
        if stage1_magic != better_boot::BOOT_MAGIC && stage1_magic != ERASED_WORD {
            defmt::debug!(
                "Confirming this bootloader to stage1 (state word was {:#010x})",
                stage1_magic
            );
            let (flag_word, state) = snapshot_state_words();
            let confirmed: Result<(), ()> = flash.lock(|flash_cell| {
                let mut flash = flash_cell.borrow_mut();
                let page_start = stage1_state_addr & !(PAGE_SIZE - 1);
                flash
                    .erase(page_start as u32, (page_start + PAGE_SIZE) as u32)
                    .map_err(|_| ())?;
                flash
                    .write(
                        stage1_state_addr as u32,
                        &better_boot::BOOT_MAGIC.to_le_bytes(),
                    )
                    .map_err(|_| ())?;
                cortex_m::asm::dsb();
                cortex_m::asm::isb();
                Ok(())
            });
            if confirmed.is_err() {
                defmt::error!("Failed to confirm the bootloader to stage1");
            }
            if rewrite_state_page_with_netcore(&flash, flag_word, state, None, ERASED_WORD).is_err()
            {
                defmt::warn!("Failed to refresh the stage2 state page");
            }
        }
    }

    if should_enter_dfu {
        defmt::info!("DFU mode");

        let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
        static SERIAL_BUF: static_cell::ConstStaticCell<[u8; 12]> =
            static_cell::ConstStaticCell::new([0; 12]);
        let serial_buf = SERIAL_BUF.take();
        let ficr = embassy_nrf::pac::FICR;
        let low = ficr.info().deviceid(0).read().to_le_bytes();
        let high = ficr.info().deviceid(1).read().to_le_bytes();
        let id_bytes = [low[0], low[1], low[2], low[3], high[0], high[1]];
        for (i, &b) in id_bytes.iter().enumerate() {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            serial_buf[i * 2] = HEX[(b >> 4) as usize];
            serial_buf[i * 2 + 1] = HEX[(b & 0xf) as usize];
        }
        // SAFETY: serial_buf holds only ASCII hex digits.
        let serial = unsafe { core::str::from_utf8_unchecked(serial_buf) };

        // The bootloader enumerates under its own product id so a host can tell
        // it apart from the application without opening the device, which also
        // frees bcdDevice to mean this bootloader's version rather than the
        // application's. The id is the application's plus 0x10.
        let mut config = embassy_usb::Config::new(0x1915, 0x5221);
        config.manufacturer = Some("Odyssey Arm");
        config.product = Some("USB-DFU Bootloader");
        config.serial_number = Some(serial);
        config.device_release = BOOTLOADER_BCD_DEVICE;

        let mut config_descriptor = [0; 256];
        let mut bos_descriptor = [0; 256];
        let mut msos_descriptor = [0; 512];
        let mut control_buf = [0; 4096];

        // The active bank decides which alt interface is offered: Bank B for
        // download when Bank A is active, and the reverse.
        let active_bank_is_a = boot_bank_name.starts_with("Bank A");
        defmt::debug!("Active bank for DFU: {}", boot_bank_name);

        let dual_bank_writer = DualBankFlashWriter::new(&flash);

        // Marks DFU requested on detach or reset.
        let mut dfu_flagger = {
            let boot_state = boot_state;
            let flash_ref = &flash;
            move || flag_dfu_requested(flash_ref, boot_state)
        };

        let download_started = Cell::new(false);
        let mut state = Control::new(
            dual_bank_writer,
            // Self-detach: the device resets itself after manifest.
            DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
            ResetImmediate,
            active_bank_is_a,
            [
                release_signing::Component::Lite1SecureApp,
                release_signing::Component::Lite1SecureNetCore,
                release_signing::Component::Lite1SecureBootloader,
            ],
        );
        state.set_trust_component(release_signing::Component::Lite1TrustList);
        state.set_firmware_version(env!("CARGO_PKG_VERSION"));
        state.set_state_flagger(&mut dfu_flagger);
        state.set_bootloader_label(concat!("BOOTLOADER ", env!("CARGO_PKG_VERSION")));
        state.set_download_started_flag(&download_started);

        let mut builder = Builder::new(
            driver,
            config,
            &mut config_descriptor,
            &mut bos_descriptor,
            &mut msos_descriptor,
            &mut control_buf,
        );

        // MSOS headers get the WinUSB driver bound on Windows without Zadig.
        // They are needed at device level, and composite devices need them at
        // function level as well.
        builder.msos_descriptor(msos::windows_version::WIN8_1, 2);
        builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
        ));

        usb_dfu::<_, _, _, 4096>(&mut builder, &mut state, |func| {
            // Function-level headers matter only for a composite device.
            func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
            func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
                "DeviceInterfaceGUIDs",
                msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
            ));
        });

        let mut dev = builder.build();

        if !resume_after_timeout {
            dev.run().await;
        } else {
            select(
                dev.run(),
                dfu_timeout::abandoned_entry(Timer::after(DFU_ENTRY_TIMEOUT), &download_started),
            )
            .await;
            dev.disable().await;
            if write_boot_state(&flash, &entry_state).is_err() {
                defmt::error!("Cannot clear abandoned DFU request; staying in recovery");
                dev.run().await;
            } else {
                defmt::debug!("No DFU download within 60 seconds; booting confirmed application");
            }
        }
    }
    let Some(boot_offset) = boot_offset else {
        loop {
            cortex_m::asm::wfi();
        }
    };

    // Booting the app while the netcore copy is still running would leave it half-written.
    {
        let pcd = get_pcd_region();
        if let Some(status) = pcd.status.get_status() {
            if status == PcdStatusCode::Updating {
                defmt::error!("CRITICAL: Netcore DFU still updating before app boot - resetting!");
                cortex_m::asm::dsb();
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
    }

    // The app starts the network core itself. Leaving it running would initialize it twice.
    {
        use embassy_nrf::pac::reset::vals::Forceoff;

        defmt::debug!("Stopping network core before booting app");
        embassy_nrf::pac::RESET_S
            .network()
            .forceoff()
            .write(|w| w.set_forceoff(Forceoff::HOLD));
        defmt::debug!("Network core stopped");
    }

    // Locked RAM is made read-only so the app cannot tamper with it.
    {
        // SAFETY: Linker script defines the locked RAM boundaries.
        unsafe extern "C" {
            static __locked_ram_start: u32;
            static __locked_ram_end: u32;
        }

        let spu = embassy_nrf::pac::SPU_S;
        let locked_ram_start = &raw const __locked_ram_start as usize;
        let locked_ram_end = &raw const __locked_ram_end as usize;

        defmt::debug!(
            "Locking RAM region 0x{:08x}-0x{:08x}",
            locked_ram_start,
            locked_ram_end
        );

        // nRF5340 SPU RAM regions are 8 KB.
        const REGION_SIZE: usize = 8192;
        let start_region = (locked_ram_start - 0x2000_0000) / REGION_SIZE;
        let end_region = (locked_ram_end - 0x2000_0000 + REGION_SIZE - 1) / REGION_SIZE;

        for region in start_region..end_region {
            spu.ramregion(region).perm().modify(|w| {
                w.set_write(false);
            });
            defmt::debug!("Locked RAM region {}", region);
        }
    }

    defmt::debug!(
        "Booting application from {} at 0x{:08x}",
        boot_bank_name,
        boot_offset
    );

    // SAFETY: Bootloader owns core peripherals until transferring control to selected valid application.
    unsafe {
        let mut p = cortex_m::Peripherals::steal();
        cortex_m::interrupt::disable();
        cortex_m::interrupt::enable();
        p.SCB.invalidate_icache();
        p.SCB.vtor.write(boot_offset as u32);
        cortex_m::asm::bootload(boot_offset as *const u32)
    }
}

/// # Safety
///
/// Called only by the Cortex-M exception vector with active interrupt state.
#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    // SAFETY: SCB_ICSR is the architected, readable interrupt-control register.
    unsafe {
        const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
        let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;
        defmt::panic!("DefaultHandler #{}", irqn);
    }
}
