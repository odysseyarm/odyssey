#![no_std]
#![no_main]
#![expect(
    unsafe_code,
    reason = "Bootloader owns linker-defined flash, shared SRAM, and peripheral register access"
)]
mod dfu_timeout;

use core::cell::{Cell, RefCell};
use core::future::Future;
use core::sync::atomic::Ordering;
use core::task::Poll;

use better_boot::{BootState, BootStateManager, STATE_MAGIC};
use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, DfuForwarder, DfuTarget, ResetImmediate, usb_dfu};
use cortex_m_rt::exception;
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_nrf::ipc::{Ipc, IpcChannel};
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::{Peri, peripherals};
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Delay, Duration, Timer};
use embassy_usb::{Builder, msos};
use embedded_storage::nor_flash::NorFlash;
use panic_probe as _;

embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
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
const BOOTLOADER_BCD_DEVICE: u16 = odyssey_version::firmware_bcd_device(VERSION[0]);
const SLOT_A: u8 = 0;
const SLOT_B: u8 = 1;
const APP_DFU_FLAG_MAGIC: u32 = 0xDF00B071;
const NETCORE_DFU_MAGIC: u32 = 0x4E45_5444; // "NETD"
const ERASED_WORD: u32 = 0xFFFF_FFFF;

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

fn get_icmsg_config() -> icmsg::MemoryConfig {
    // SAFETY: Linker script defines these symbols as the app-owned ICMSG shared-memory regions.
    unsafe extern "C" {
        static mut __icmsg_tx_start: u32;
        static __icmsg_tx_end: u32;
        static mut __icmsg_rx_start: u32;
        static __icmsg_rx_end: u32;
    }

    const ALIGN: usize = 4;
    // Linker symbols delimit a region, not one Rust allocation.
    let header_len = core::mem::size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>();
    let send_buffer_len = ((&raw const __icmsg_tx_end).addr()
        - (&raw const __icmsg_tx_start).addr()
        - header_len) as u32;
    let recv_buffer_len = ((&raw const __icmsg_rx_end).addr()
        - (&raw const __icmsg_rx_start).addr()
        - header_len) as u32;

    // TX and RX as the app core sees them. The netcore has them swapped.
    icmsg::MemoryConfig {
        send_region: (&raw mut __icmsg_tx_start as *mut u32).cast(),
        recv_region: (&raw mut __icmsg_rx_start as *mut u32).cast(),
        send_buffer_len,
        recv_buffer_len,
    }
}

struct IpcForwarder {
    icmsg: Option<icmsg::IcMsg<IpcNotify, IpcWait, 4>>,
    ipc_peripheral: Option<Peri<'static, peripherals::IPC>>,
}

struct IpcNotify {
    trigger: embassy_nrf::ipc::EventTrigger<'static>,
}

impl icmsg::Notifier for IpcNotify {
    fn notify(&mut self) {
        self.trigger.trigger();
    }
}

struct IpcWait {
    event: embassy_nrf::ipc::Event<'static>,
}

impl icmsg::WaitForNotify for IpcWait {
    async fn wait_for_notify(&mut self) {
        self.event.wait().await;
    }
}

impl IpcForwarder {
    fn new(ipc_peripheral: Peri<'static, peripherals::IPC>) -> Self {
        Self {
            icmsg: None,
            ipc_peripheral: Some(ipc_peripheral),
        }
    }

    fn initialize_ipc_blocking(&mut self) -> Result<(), ()> {
        if self.icmsg.is_some() {
            return Ok(());
        }

        defmt::info!("Initializing IPC for network core DFU (blocking)");

        let ipc_peripheral = self.ipc_peripheral.take().ok_or(())?;
        let config = get_icmsg_config();

        let mut ipc = Ipc::new(ipc_peripheral, Irqs);
        ipc.event0.configure_trigger([IpcChannel::Channel0]);
        ipc.event0.configure_wait([IpcChannel::Channel1]);

        let notify = IpcNotify {
            trigger: ipc.event0.trigger_handle(),
        };

        let wait = IpcWait { event: ipc.event0 };

        // `send_dfu_start` is a sync trait method, so the async init is polled by hand.
        // SAFETY: ICMSG regions come from the linker map and both cores follow ICMSG's shared-memory protocol.
        let init_future = unsafe { icmsg::IcMsg::init(config, notify, wait, Delay) };
        futures::pin_mut!(init_future);

        let waker = futures::task::noop_waker();
        let mut cx = core::task::Context::from_waker(&waker);

        loop {
            match init_future.as_mut().poll(&mut cx) {
                Poll::Ready(Ok(icmsg)) => {
                    defmt::info!("IPC initialized successfully");
                    self.icmsg = Some(icmsg);
                    return Ok(());
                }
                Poll::Ready(Err(_)) => {
                    defmt::error!("Failed to initialize IPC");
                    return Err(());
                }
                Poll::Pending => {
                    cortex_m::asm::nop();
                }
            }
        }
    }
}

impl DfuForwarder for IpcForwarder {
    fn start_forward(&mut self, total_size: usize) -> Result<(), ()> {
        defmt::info!("Starting network core DFU forward, size: {}", total_size);

        defmt::info!("Starting network core for DFU");
        use embassy_nrf::pac::reset::vals::Forceoff;
        use embassy_nrf::pac::spu::vals;

        // SAFETY: Bootloader owns SPU configuration before releasing network core; writes configure its flash access.
        unsafe {
            let spu = embassy_nrf::pac::SPU_S;

            // The netcore reads both app flash (0x00000000+) and its own flash
            // (0x01000000+), so every flash region goes non-secure and unlocked.
            const NUM_FLASH_REGIONS: usize = 64;
            for i in 0..NUM_FLASH_REGIONS {
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

            // SPU configuration must take effect before the netcore runs.
            cortex_m::asm::dsb();
            cortex_m::asm::isb();

            // nRF5340 erratum 161: release, hold for 5 µs, then release again.
            (0x50005618 as *mut u32).write_volatile(1);

            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::RELEASE));

            // 5 µs at 64 MHz.
            cortex_m::asm::delay(5 * 64);

            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::HOLD));

            // 1 µs at 64 MHz.
            cortex_m::asm::delay(1 * 64);

            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::RELEASE));

            // End of the erratum 161 sequence.
            (0x50005618 as *mut u32).write_volatile(0);
        }
        defmt::info!("Network core started for DFU");

        // The PCD command tells the netcore bootloader an update is pending.
        let pcd = get_pcd_region();

        // SAFETY: Linker script defines this symbol as the shared DFU flash region start.
        unsafe extern "C" {
            static __bootloader_dfu_region_start: u32;
        }
        // SAFETY: The linker symbol points to the start of mapped DFU flash.
        let dfu_addr = unsafe { &__bootloader_dfu_region_start as *const u32 as u32 };

        pcd.cmd.set_update(dfu_addr, total_size as u32);
        defmt::info!("Set PCD command: addr={:#x}, size={}", dfu_addr, total_size);

        self.initialize_ipc_blocking()?;

        if let Some(ref mut icmsg) = self.icmsg {
            let msg = [
                0u8,
                0,
                0,
                0,
                (total_size >> 24) as u8,
                (total_size >> 16) as u8,
                (total_size >> 8) as u8,
                total_size as u8,
            ];

            icmsg.send(&msg).map_err(|_| ())?;
        }

        Ok(())
    }

    fn forward_chunk(&mut self, offset: usize, data: &[u8]) -> Result<(), ()> {
        defmt::debug!("Forwarding chunk at offset {}, len {}", offset, data.len());

        // IPC comes up in `send_dfu_start`. A chunk before that fails.
        if self.icmsg.is_none() {
            defmt::error!("IPC not initialized, cannot forward chunk");
            return Err(());
        }

        if let Some(ref mut icmsg) = self.icmsg {
            const MAX_CHUNK_SIZE: usize = 256;
            for (chunk_offset, chunk) in data.chunks(MAX_CHUNK_SIZE).enumerate() {
                let actual_offset = offset + chunk_offset * MAX_CHUNK_SIZE;

                // [1, offset: u32 LE, length: u16 LE, data...]
                let mut msg = [0u8; MAX_CHUNK_SIZE + 7];
                msg[0] = 1;
                msg[1..5].copy_from_slice(&(actual_offset as u32).to_le_bytes());
                msg[5..7].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
                msg[7..7 + chunk.len()].copy_from_slice(chunk);

                icmsg.send(&msg[..7 + chunk.len()]).map_err(|_| ())?;
            }
        }

        Ok(())
    }

    fn finish_forward(&mut self) -> Result<(), ()> {
        defmt::info!("Finishing network core DFU forward");

        if let Some(ref mut icmsg) = self.icmsg {
            let msg = [2u8];

            icmsg.send(&msg).map_err(|_| ())?;
        }

        // The PCD command stays set. The netcore bootloader clears it once it has
        // copied the image.
        defmt::info!("Network core DFU forwarding complete");

        Ok(())
    }
}

/// Writes DFU images into flash: the inactive app bank, or the shared DFU region
/// for netcore and bootloader images.
struct DualBankFlashWriter<'d> {
    flash: &'d Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'d>>>,
    active_bank_is_a: bool,
    netcore_dfu_size: usize,
}

impl<'d> DualBankFlashWriter<'d> {
    fn new(
        flash: &'d Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'d>>>,
        active_bank_is_a: bool,
    ) -> Self {
        Self {
            flash,
            active_bank_is_a,
            netcore_dfu_size: 0,
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
            static __bootloader_dfu_region_end: u32;
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
                    let end = &__bootloader_dfu_region_end as *const u32 as usize;
                    (start, end)
                }
                DfuTarget::Bootloader => {
                    // Stage1 swaps the bootloader in from here.
                    let start = &__bootloader_dfu_region_start as *const u32 as usize;
                    let end = &__bootloader_dfu_region_end as *const u32 as usize;
                    (start, end)
                }
            }
        }
    }
}

impl<'d> better_dfu::DualBankWriter for DualBankFlashWriter<'d> {
    fn write_for_target(
        &mut self,
        target: DfuTarget,
        offset: usize,
        data: &[u8],
    ) -> Result<(), ()> {
        let (bank_start, bank_end) = self.get_bank_range(target);

        if bank_start == 0 {
            defmt::error!("Invalid target for dual-bank write: {:?}", target);
            return Err(());
        }

        let bank_len = bank_end.saturating_sub(bank_start);
        if offset > bank_len || data.len() > bank_len - offset {
            defmt::error!(
                "Write would exceed bank boundary: offset={:#x}, len={}, capacity={:#x}",
                offset,
                data.len(),
                bank_len
            );
            return Err(());
        }
        let write_addr = bank_start + offset;

        defmt::debug!(
            "Writing {} bytes to {:?} at offset {:#x}",
            data.len(),
            target,
            write_addr
        );

        self.flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();

            // DFU appends chunks in order. The page containing an unaligned offset already holds
            // earlier bytes. Erase only pages first reached by this chunk.
            let page_start = (write_addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
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
            self.netcore_dfu_size = if offset == 0 {
                data.len()
            } else {
                (offset + data.len()).max(self.netcore_dfu_size)
            };
            let pcd = get_pcd_region();
            pcd.status.reserved[0].store(self.netcore_dfu_size as u32, Ordering::Release);
            pcd.status.reserved[1].store(0x4E_5450, Ordering::Release); // "NTP"
        }
        Ok(())
    }

    fn mark_updated_for_target(&mut self, target: DfuTarget) -> Result<(), ()> {
        defmt::info!(
            "Marking {:?} as updated (direct XIP, no swap needed)",
            target
        );

        // The bank written last is preferred on the next boot.
        let target_slot = match target {
            DfuTarget::AppBankA => SLOT_A,
            DfuTarget::AppBankB => SLOT_B,
            DfuTarget::NetCore => {
                defmt::info!(
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
                defmt::info!(
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
                    defmt::info!("Netcore DFU info written to flash for persistence across reset");
                }

                return Ok(());
            }
            DfuTarget::Bootloader => {
                defmt::info!("Bootloader DFU completed, marking for swap via stage1");
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

                    defmt::info!("Stage1 state marked for bootloader swap");
                    Ok(())
                })?;

                return Ok(());
            }
        };

        let active_slot = if self.active_bank_is_a {
            SLOT_A
        } else {
            SLOT_B
        };

        let state = BootState {
            magic: STATE_MAGIC,
            active_slot,
            pending_slot: target_slot,
            boot_counter: 0,
            reserved: 0,
        };

        // BOOT_MAGIC keeps the bootloader out of DFU on the next boot. See
        // check_dfu_state.
        // SAFETY: Linker script defines the boot-state flash partition.
        unsafe extern "C" {
            static __bootloader_state_start: u32;
        }
        // SAFETY: The linker symbol points to the boot-state partition.
        let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };
        defmt::info!("Writing BOOT_MAGIC to state page at 0x{:08x}", state_addr);

        self.flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();

            let page_start = state_addr & !(PAGE_SIZE - 1);
            let page_end = page_start + PAGE_SIZE;

            if let Err(_) = flash.erase(page_start as u32, page_end as u32) {
                defmt::error!("Failed to erase state page at {:#x}", page_start);
                return Err(());
            }

            // Layout: BOOT_MAGIC, then the BootState.
            if let Err(_) = flash.write(state_addr as u32, &better_boot::BOOT_MAGIC.to_le_bytes()) {
                defmt::error!("Failed to write BOOT_MAGIC at {:#x}", state_addr);
                return Err(());
            }

            // SAFETY: `state` is initialized and remains live for this exact byte slice length.
            let state_bytes = unsafe {
                core::slice::from_raw_parts(&state as *const BootState as *const u8, core::mem::size_of::<BootState>())
            };
            if let Err(_) = flash.write((state_addr + 4) as u32, state_bytes) {
                defmt::error!("Failed to write boot state at {:#x}", state_addr + 4);
                return Err(());
            }

            // SAFETY: State address is linker-defined, aligned, and readable after completed flash writes.
            let verify_magic = unsafe { core::ptr::read_volatile(state_addr as *const u32) };
            // SAFETY: State address plus magic word is aligned and contains initialized BootState after write.
            let verify_state = unsafe { core::ptr::read_volatile((state_addr + 4) as *const BootState) };
            defmt::info!(
                "State page read-back after write: magic={:#010x}, active={}, pending={}, attempts={}",
                verify_magic,
                verify_state.active_slot,
                verify_state.pending_slot,
                verify_state.boot_counter
            );

            Ok(())
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

fn valid_resume_vectors(slot: u8) -> bool {
    // SAFETY: Linker script defines app-bank boundaries in mapped flash.
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
    // SAFETY: Linker-defined application banks contain aligned, readable vector tables.
    let sp = unsafe { core::ptr::read_volatile(bank.start as *const u32) };
    // SAFETY: The reset vector is the second aligned word in the application bank.
    let reset = unsafe { core::ptr::read_volatile((bank.start + 4) as *const u32) };
    better_boot::valid_vectors(bank, sp, reset)
}

/// Selects only banks with valid stack and reset vectors; rolls back failed pending slot.
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
    // SAFETY: Linker symbols delimit readable, aligned application vector tables.
    let bank_a_sp = unsafe { core::ptr::read_volatile(bank_a_start as *const u32) };
    // SAFETY: Reset vector is second aligned word inside bank A.
    let bank_a_reset = unsafe { core::ptr::read_volatile((bank_a_start + 4) as *const u32) };
    // SAFETY: Linker symbols delimit readable, aligned application vector tables.
    let bank_b_sp = unsafe { core::ptr::read_volatile(bank_b_start as *const u32) };
    // SAFETY: Reset vector is second aligned word inside bank B.
    let bank_b_reset = unsafe { core::ptr::read_volatile((bank_b_start + 4) as *const u32) };
    let bank_a_valid =
        better_boot::valid_vectors(bank_a_start..bank_a_end, bank_a_sp, bank_a_reset);
    let bank_b_valid =
        better_boot::valid_vectors(bank_b_start..bank_b_end, bank_b_sp, bank_b_reset);

    defmt::info!(
        "Bank A: addr={:#x}, sp={:#x}, reset={:#x}, valid={}",
        bank_a_start,
        bank_a_sp,
        bank_a_reset,
        bank_a_valid
    );
    defmt::info!(
        "Bank B: addr={:#x}, sp={:#x}, reset={:#x}, valid={}",
        bank_b_start,
        bank_b_sp,
        bank_b_reset,
        bank_b_valid
    );

    let mut state = read_boot_state(flash);
    let active_slot = state.active_slot;
    defmt::info!(
        "Boot state: active_slot={}, pending_slot={}, attempts={}",
        active_slot,
        state.pending_slot,
        state.boot_counter
    );
    let chosen_slot = better_boot::select_boot_slot(&mut state, [bank_a_valid, bank_b_valid]);
    let (offset, name) = match chosen_slot {
        Some(SLOT_A) => (Some(bank_a_start), "Bank A"),
        Some(SLOT_B) => (Some(bank_b_start), "Bank B"),
        None => {
            defmt::warn!("No valid application banks; entering DFU recovery");
            if active_slot == SLOT_B {
                (None, "Bank B (DFU)")
            } else {
                (None, "Bank A (DFU)")
            }
        }
        Some(_) => unreachable!(),
    };

    match write_boot_state(flash, &state) {
        Ok(()) => {
            let verify_state = read_boot_state(flash);
            defmt::info!(
                "Boot state persisted: active={}, pending={}, attempts={}",
                verify_state.active_slot,
                verify_state.pending_slot,
                verify_state.boot_counter
            );
        }
        Err(_) => defmt::warn!("Failed to persist boot state"),
    }

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
        // SAFETY: BootState follows the state word at its linker-defined aligned flash address.
        let state_struct = unsafe {
            core::ptr::read_volatile((state_addr + core::mem::size_of::<u32>()) as *const BootState)
        };

        defmt::info!(
            "Boot state word: {:#010x}, active={}, pending={}, attempts={}",
            state_word,
            state_struct.active_slot,
            state_struct.pending_slot,
            state_struct.boot_counter
        );

        state_word == better_boot::SWAP_MAGIC
    })
}

fn get_boot_state_manager() -> BootStateManager {
    // SAFETY: Linker script defines the boot-state flash partition.
    unsafe extern "C" {
        static __bootloader_state_start: u32;
    }
    // SAFETY: The linker symbol points to the boot-state flash partition.
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as u32 };
    BootStateManager::new(state_addr)
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

        defmt::info!(
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

    defmt::info!("Watchdog feeder task started (feeding every 30s)");

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
            defmt::info!("PCD status from previous net DFU: {}", status as u32);
            if matches!(
                status,
                PcdStatusCode::Success
                    | PcdStatusCode::FailedCopy
                    | PcdStatusCode::FailedVerify
                    | PcdStatusCode::AppConfirmed
            ) {
                if let Err(_) = clear_netcore_dfu_info(&flash) {
                    defmt::warn!("Failed to clear persisted netcore DFU info after status");
                }
                pcd.cmd.clear();
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

                defmt::info!(
                    "Found netcore DFU in flash: addr={:#x}, size={}",
                    dfu_addr,
                    dfu_size
                );
                pcd.status.clear();
                pcd.cmd.set_update(dfu_addr, dfu_size);
            }
        }
    }

    defmt::info!(
        "PCD region: magic={:#x}, addr={:#x}, size={}",
        // SAFETY: `pcd` points to valid shared SRAM and the field is an aligned atomic word.
        unsafe { core::ptr::read_volatile(&pcd.cmd.magic as *const _ as *const u32) },
        pcd.cmd.get_image_addr(),
        pcd.cmd.get_image_size()
    );

    let netcore_was_pending = netcore_recovery || pcd.copy_pending();
    // A terminal status from the netcore bootloader settles the update: clear the persisted command and skip DFU.
    if pcd.status.is_valid() {
        if let Some(status) = pcd.status.get_status() {
            defmt::info!("Netcore PCD status pre-check: {}", status as u32);
            if matches!(
                status,
                PcdStatusCode::Success
                    | PcdStatusCode::FailedCopy
                    | PcdStatusCode::FailedVerify
                    | PcdStatusCode::AppConfirmed
            ) {
                if let Err(_) = clear_netcore_dfu_info(&flash) {
                    defmt::warn!("Failed to clear persisted netcore DFU info after status");
                }
                pcd.cmd.clear();
                pcd.status.clear();
            }
        }
    }

    // A command nothing has answered yet is pending, whether or not the net core
    // bootloader has written a status. Terminal statuses were settled above.
    let mut netcore_dfu_pending = pcd.copy_pending();

    if netcore_dfu_pending {
        defmt::info!("PCD command VALID - will start netcore bootloader");
    } else {
        defmt::info!("PCD command INVALID - skipping netcore DFU");
    }

    // A copy that never reports back is retried a few times, then abandoned: the
    // pending command survives power cycles, so an unbounded retry would trap the
    // unit before DFU. Giving up enters DFU so the update can be run again.
    let mut netcore_gave_up = false;
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
                defmt::info!(
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
        defmt::info!("Network core DFU pending, starting network core bootloader");
        use embassy_nrf::pac::reset::vals::Forceoff;
        use embassy_nrf::pac::spu::vals;

        // SAFETY: This bootloader owns SPU configuration before releasing the network core.
        unsafe {
            let spu = embassy_nrf::pac::SPU_S;

            // The netcore reads app flash and its own.
            const NUM_FLASH_REGIONS: usize = 64;
            for i in 0..NUM_FLASH_REGIONS {
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
        defmt::info!("Network core bootloader started");

        // Images up to 196 KB take a while to copy. After 2 minutes without a
        // terminal status the system resets and retries on the next boot.
        let poll = async {
            let mut ticks: u32 = 0;
            const MAX_TICKS: u32 = 1200; // 2 minutes at 100 ms per tick
            loop {
                // The status word survives a reset while its magic does not, so a
                // stale terminal code from an earlier update must not end the wait.
                let status = if pcd.status.is_valid() {
                    pcd.status.get_status()
                } else {
                    None
                };
                if let Some(status) = status {
                    defmt::info!("Netcore DFU status from net bootloader: {}", status as u32);
                    if matches!(
                        status,
                        PcdStatusCode::Success
                            | PcdStatusCode::FailedCopy
                            | PcdStatusCode::FailedVerify
                    ) {
                        if let Err(_) = clear_netcore_dfu_info(&flash) {
                            defmt::warn!("Failed to clear persisted netcore DFU info after status");
                        }
                        pcd.cmd.clear();
                        pcd.status.clear();
                        netcore_dfu_pending = false;
                        break;
                    }
                }

                // 2 minutes up: reset.
                if ticks >= MAX_TICKS {
                    defmt::error!(
                        "Netcore DFU taking too long (>2 min) - resetting to retry on next boot"
                    );
                    cortex_m::peripheral::SCB::sys_reset();
                }

                if ticks % 10 == 0 {
                    // SAFETY: `pcd` points to valid shared SRAM; atomic fields are aligned and initialized.
                    let magic = unsafe {
                        core::ptr::read_volatile(&pcd.status.magic as *const _ as *const u32)
                    };
                    // SAFETY: `pcd` points to valid shared SRAM; atomic fields are aligned and initialized.
                    let raw_status = unsafe {
                        core::ptr::read_volatile(&pcd.status.status as *const _ as *const u32)
                    };
                    defmt::info!(
                        "Waiting for netcore DFU status... ticks={}/{} magic={:#x} raw_status={:#x}",
                        ticks,
                        MAX_TICKS,
                        magic,
                        raw_status
                    );
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
        defmt::info!("DFU requested via RAM flag");
    }

    // A pending netcore update takes precedence over app DFU, even with SWAP_MAGIC set.
    let mut should_enter_dfu = ram_dfu_request || check_dfu_state(&flash) || netcore_gave_up;
    if netcore_dfu_pending && should_enter_dfu {
        defmt::info!("Clearing app DFU request because netcore DFU is pending");
        should_enter_dfu = false;
        // Clear SWAP_MAGIC, otherwise every boot lands in DFU.
        let mut state = read_boot_state(&flash);
        state.boot_counter = 0;
        let _ = write_boot_state(&flash, &state);
    }
    defmt::info!(
        "DFU decision: should_enter_dfu={} netcore_dfu_pending={}",
        should_enter_dfu,
        netcore_dfu_pending
    );

    let entry_state = stored_boot_state();
    let confirmed_application = entry_state.is_valid()
        && entry_state.active_slot == entry_state.pending_slot
        && entry_state.boot_counter == 0
        && matches!(entry_state.active_slot, SLOT_A | SLOT_B);
    let resume_after_timeout = should_enter_dfu
        && confirmed_application
        && !netcore_was_pending
        && !netcore_gave_up
        && valid_resume_vectors(entry_state.active_slot);

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
            defmt::info!(
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

        defmt::info!("Creating USB driver with VBUS detect");
        let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
        defmt::info!("USB driver created");
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

        // `Control` needs an updater. The default linker symbols point at
        // bank A and B, but every write goes through DualBankFlashWriter.
        let fw_config =
            embassy_boot::FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
        let mut buffer = embassy_boot::AlignedBuffer([0; 4]);
        let updater = embassy_boot::BlockingFirmwareUpdater::new(fw_config, &mut buffer.0[..]);

        let mut config_descriptor = [0; 256];
        let mut bos_descriptor = [0; 256];
        let mut msos_descriptor = [0; 512];
        let mut control_buf = [0; 4096];

        defmt::info!("Creating DFU control with dual-bank and network core support");

        // The active bank decides which alt interface is offered: Bank B for
        // download when Bank A is active, and the reverse.
        let active_bank_is_a = boot_bank_name.starts_with("Bank A");
        defmt::info!("Active bank for DFU: {}", boot_bank_name);

        let forwarder = IpcForwarder::new(p.IPC);

        let dual_bank_writer = DualBankFlashWriter::new(&flash, active_bank_is_a);

        // Marks DFU requested on detach or reset.
        let mut dfu_flagger = {
            let boot_state = boot_state;
            let flash_ref = &flash;
            move || flag_dfu_requested(flash_ref, boot_state)
        };

        let download_started = Cell::new(false);
        let mut state = Control::new(
            updater,
            // Self-detach: the device resets itself after manifest.
            DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
            ResetImmediate,
            forwarder,
            dual_bank_writer,
            active_bank_is_a,
        );
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

        usb_dfu::<_, _, _, _, _, _, 4096>(&mut builder, &mut state, |func| {
            // Function-level headers matter only for a composite device.
            func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
            func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
                "DeviceInterfaceGUIDs",
                msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
            ));
        });

        let mut dev = builder.build();

        defmt::info!("USB device built, starting run loop");
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
                defmt::info!("No DFU download within 60 seconds; booting confirmed application");
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

        defmt::info!("Stopping network core before booting app");
        embassy_nrf::pac::RESET_S
            .network()
            .forceoff()
            .write(|w| w.set_forceoff(Forceoff::HOLD));
        defmt::info!("Network core stopped");
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

        defmt::info!(
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
            defmt::info!("Locked RAM region {}", region);
        }
    }

    // Dead: the stage1_state write happens right after select_boot_bank, since
    // erasing just before the app jump corrupted flash.
    if false {
        // SAFETY: Linker script defines the stage-one state page in mapped flash.
        unsafe extern "C" {
            static __stage1_state_start: u32;
        }

        // SAFETY: Linker symbol points to the aligned stage-one state word.
        let stage1_state_addr = unsafe { &__stage1_state_start as *const u32 as u32 };

        // Same value embassy-boot's mark_booted() writes: BOOT_MAGIC, 0xD0D0D0D0.

        defmt::info!(
            "Marking stage2 bootloader boot as successful for stage1 (writing to state at {:#010x})",
            stage1_state_addr
        );

        // stage2_state is read before and after to see whether erasing stage1_state disturbs it.
        // SAFETY: Linker script defines the stage-two boot-state partition.
        unsafe extern "C" {
            static __bootloader_state_start: u32;
        }
        // SAFETY: Linker symbol points to the aligned stage-two state word.
        let stage2_state_addr = unsafe { &__bootloader_state_start as *const u32 as u32 };
        // SAFETY: The state word is linker-defined, aligned, and readable RAM-mapped flash.
        let stage2_before = unsafe { core::ptr::read_volatile(stage2_state_addr as *const u32) };
        defmt::info!(
            "[PRE-ERASE] stage2_state at {:#010x} = {:#010x}",
            stage2_state_addr,
            stage2_before
        );

        flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();

            let page_start = stage1_state_addr & !(PAGE_SIZE as u32 - 1);
            let page_end = page_start + PAGE_SIZE as u32;

            defmt::info!(
                "[ERASE] Erasing stage1 page: start={:#010x}, end={:#010x}, size={}",
                page_start,
                page_end,
                page_end - page_start
            );

            if let Err(_) = flash.erase(page_start, page_end) {
                defmt::error!("Failed to erase stage1 state page at {:#x}", page_start);
                return;
            }

            defmt::info!("[ERASE] Stage1 state page erased successfully");

            // The erase must complete before anything else touches flash.
            cortex_m::asm::dsb();
            cortex_m::asm::isb();

            let stage2_after_erase =
            // SAFETY: State address is linker-defined aligned flash and erase operation has completed.
                unsafe { core::ptr::read_volatile(stage2_state_addr as *const u32) };
            defmt::info!(
                "[POST-ERASE] stage2_state at {:#010x} = {:#010x}",
                stage2_state_addr,
                stage2_after_erase
            );

            if let Err(_) = flash.write(stage1_state_addr, &better_boot::BOOT_MAGIC.to_le_bytes()) {
                defmt::error!(
                    "Failed to write BOOT_MAGIC to stage1 state at {:#x}",
                    stage1_state_addr
                );
                return;
            }

            // SAFETY: Stage-one state address is linker-defined aligned flash; write completed before read-back.
            let verify = unsafe { core::ptr::read_volatile(stage1_state_addr as *const u32) };
            defmt::info!("Stage1 state marked booted: magic={:#010x}", verify);

            // SAFETY: Stage-two state address is linker-defined aligned readable flash.
            let stage2_final = unsafe { core::ptr::read_volatile(stage2_state_addr as *const u32) };
            defmt::info!(
                "[FINAL] stage2_state at {:#010x} = {:#010x}",
                stage2_state_addr,
                stage2_final
            );
        });
    }

    defmt::info!(
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
        panic!("DefaultHandler #{:?}", irqn);
    }
}
