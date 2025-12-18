#![no_std]
#![no_main]

use core::cell::RefCell;
use core::future::Future;
use core::task::Poll;

use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, DfuForwarder, DfuTarget, ResetImmediate, usb_dfu};
use cortex_m_rt::exception;
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::ipc::{Ipc, IpcChannel};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::nvmc::PAGE_SIZE;
use embassy_nrf::{
    Peri, peripherals,
    usb::{Driver, vbus_detect::HardwareVbusDetect},
};
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

/// Locked RAM structure for bootloader communication
use pcd::{PcdRegion, PcdStatusCode};

#[repr(C)]
#[derive(Copy, Clone)]
struct BootState {
    magic: u32,
    active_slot: u8,
    pending_slot: u8,
    boot_counter: u8,
    _reserved: u8,
}

// Simple app core DFU request flag in high RAM
#[repr(C)]
struct AppDfuFlag {
    magic: u32,
    dfu_requested: u32,
}

const BOOT_STATE_MAGIC: u32 = 0xB007_10AD;
const BOOT_STATE_SLOT_A: u8 = 0;
const BOOT_STATE_SLOT_B: u8 = 1;
const APP_DFU_FLAG_MAGIC: u32 = 0xDF00B071;
const NETCORE_DFU_MAGIC: u32 = 0x4E45_5444; // "NETD"
const ERASED_WORD: u32 = 0xFFFF_FFFF;

// PCD region in ICMSG shared SRAM (app core RX region) for netcore DFU
const PCD_REGION_ADDR: u32 = 0x20070000;

// App DFU flag in high RAM (separate from PCD region)
const APP_DFU_FLAG_ADDR: u32 = 0x2006FF00;

fn get_pcd_region() -> &'static mut PcdRegion {
    unsafe { PcdRegion::at_mut(PCD_REGION_ADDR as usize) }
}

fn get_app_dfu_flag() -> &'static mut AppDfuFlag {
    unsafe { &mut *(APP_DFU_FLAG_ADDR as *mut AppDfuFlag) }
}

unsafe fn get_icmsg_config() -> icmsg::MemoryConfig {
    extern "C" {
        static mut __icmsg_tx_start: u32;
        static __icmsg_tx_end: u32;
        static mut __icmsg_rx_start: u32;
        static __icmsg_rx_end: u32;
    }

    const ALIGN: usize = 4;
    let send_buffer_len = (&raw const __icmsg_tx_end).byte_offset_from(&raw const __icmsg_tx_start)
        as u32
        - core::mem::size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>() as u32;
    let recv_buffer_len = (&raw const __icmsg_rx_end).byte_offset_from(&raw const __icmsg_rx_start)
        as u32
        - core::mem::size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>() as u32;

    // App core perspective (opposite of netcore)
    icmsg::MemoryConfig {
        send_region: (&raw mut __icmsg_tx_start as *mut u32).cast(),
        recv_region: (&raw mut __icmsg_rx_start as *mut u32).cast(),
        send_buffer_len,
        recv_buffer_len,
    }
}

/// Async IPC forwarder for network core DFU updates
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
        let config = unsafe { get_icmsg_config() };

        let mut ipc = Ipc::new(ipc_peripheral, Irqs);
        ipc.event0.configure_trigger([IpcChannel::Channel0]);
        ipc.event0.configure_wait([IpcChannel::Channel1]);

        let notify = IpcNotify {
            trigger: ipc.event0.trigger_handle(),
        };

        let wait = IpcWait { event: ipc.event0 };

        // Block on the async init by manually polling
        let init_future = unsafe { icmsg::IcMsg::init(config, notify, wait, Delay) };
        futures::pin_mut!(init_future);

        let waker = futures::task::noop_waker();
        let mut cx = core::task::Context::from_waker(&waker);

        // Poll until ready - this is a busy loop but necessary for sync context
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
                    // Yield to allow other tasks/interrupts
                    cortex_m::asm::nop();
                }
            }
        }
    }
}

impl DfuForwarder for IpcForwarder {
    fn start_forward(&mut self, total_size: usize) -> Result<(), ()> {
        defmt::info!("Starting network core DFU forward, size: {}", total_size);

        // Start network core
        defmt::info!("Starting network core for DFU");
        use embassy_nrf::pac::reset::vals::Forceoff;
        use embassy_nrf::pac::spu::vals;

        unsafe {
            // Configure SPU to allow network core access
            let spu = embassy_nrf::pac::SPU_S;

            // Configure all flash regions to be non-secure and accessible by network core
            // This includes both app flash (0x00000000+) and network core flash (0x01000000+)
            const NUM_FLASH_REGIONS: usize = 64; // Maximum number of flash regions
            for i in 0..NUM_FLASH_REGIONS {
                spu.flashregion(i).perm().write(|w| {
                    w.set_read(true);
                    w.set_write(true);
                    w.set_execute(true);
                    w.set_secattr(false); // Non-secure
                    w.set_lock(false); // Unlock to allow netcore access
                });
            }

            // Configure EXTDOMAIN[0] for network core access
            spu.extdomain(0).perm().write(|w| {
                w.set_securemapping(vals::ExtdomainPermSecuremapping::NON_SECURE);
                w.set_lock(false);
            });

            // Ensure SPU configuration completes before starting netcore
            cortex_m::asm::dsb();
            cortex_m::asm::isb();

            // Erratum 161 workaround
            (0x50005618 as *mut u32).write_volatile(1);

            // Release network core reset
            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::RELEASE));

            // Delay 5us (at 64MHz)
            cortex_m::asm::delay(5 * 64);

            // Hold
            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::HOLD));

            // Delay 1us (at 64MHz)
            cortex_m::asm::delay(1 * 64);

            // Release again
            embassy_nrf::pac::RESET_S
                .network()
                .forceoff()
                .write(|w| w.set_forceoff(Forceoff::RELEASE));

            // Clear erratum workaround
            (0x50005618 as *mut u32).write_volatile(0);
        }
        defmt::info!("Network core started for DFU");

        // Write PCD command to indicate network core DFU is pending
        let pcd = get_pcd_region();

        extern "C" {
            static __bootloader_dfu_region_start: u32;
        }
        let dfu_addr = unsafe { &__bootloader_dfu_region_start as *const u32 as u32 };

        pcd.cmd.set_update(dfu_addr, total_size as u32);
        defmt::info!("Set PCD command: addr={:#x}, size={}", dfu_addr, total_size);

        // Initialize IPC using blocking poll
        self.initialize_ipc_blocking()?;

        // Send start message to network core bootloader via IPC
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

        // Note: IPC initialization is async but this is a sync trait method
        // We can't actually send chunks without IPC being initialized
        // This will fail if IPC isn't ready
        if self.icmsg.is_none() {
            defmt::error!("IPC not initialized, cannot forward chunk");
            return Err(());
        }

        if let Some(ref mut icmsg) = self.icmsg {
            // Send chunk via IPC in smaller pieces if needed
            const MAX_CHUNK_SIZE: usize = 256;
            for (chunk_offset, chunk) in data.chunks(MAX_CHUNK_SIZE).enumerate() {
                let actual_offset = offset + chunk_offset * MAX_CHUNK_SIZE;

                // Message format: [1 (chunk marker), offset (4 bytes), length (2 bytes), data...]
                let mut msg = [0u8; MAX_CHUNK_SIZE + 7];
                msg[0] = 1; // Chunk marker
                msg[1..5].copy_from_slice(&(actual_offset as u32).to_le_bytes());
                msg[5..7].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
                msg[7..7 + chunk.len()].copy_from_slice(chunk);

                // Send is not async, it returns Result immediately
                icmsg.send(&msg[..7 + chunk.len()]).map_err(|_| ())?;
            }
        }

        Ok(())
    }

    fn finish_forward(&mut self) -> Result<(), ()> {
        defmt::info!("Finishing network core DFU forward");

        if let Some(ref mut icmsg) = self.icmsg {
            // Send finish message
            let msg = [2u8]; // Finish marker

            // Send is not async, it returns Result immediately
            icmsg.send(&msg).map_err(|_| ())?;
        }

        // Note: We don't clear the PCD command here - the netcore bootloader
        // will clear it after processing the update
        defmt::info!("Network core DFU forwarding complete");

        Ok(())
    }
}

/// Dual-bank flash writer for handling writes to either bank A or bank B
/// When active_bank_is_a is true, writes to AppBankA go to bank B (inactive)
/// and writes to AppBankB go to bank A (inactive) - swapping the targets
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

    /// Map the DFU target to the actual flash bank, swapping based on active bank
    /// In the current scheme, the handler already chooses the inactive bank, so this is identity.
    fn map_target(&self, target: DfuTarget) -> DfuTarget {
        match target {
            DfuTarget::NetCore => DfuTarget::NetCore,
            _ => target,
        }
    }

    fn get_bank_range(&self, target: DfuTarget) -> (usize, usize) {
        extern "C" {
            static __bootloader_app_a_start: u32;
            static __bootloader_app_a_end: u32;
            static __bootloader_app_b_start: u32;
            static __bootloader_app_b_end: u32;
            static __bootloader_dfu_region_start: u32;
            static __bootloader_dfu_region_end: u32;
        }

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
                    // Netcore DFU writes to the shared DFU region
                    let start = &__bootloader_dfu_region_start as *const u32 as usize;
                    let end = &__bootloader_dfu_region_end as *const u32 as usize;
                    (start, end)
                }
                DfuTarget::Bootloader => {
                    // Bootloader DFU writes to the shared DFU region (swap-based update)
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
        // Map the target to the actual bank (swap if needed)
        let actual_target = self.map_target(target);
        defmt::debug!(
            "DFU write: interface={:?}, actual_target={:?}",
            target,
            actual_target
        );

        let (bank_start, bank_end) = self.get_bank_range(actual_target);

        if bank_start == 0 {
            defmt::error!("Invalid target for dual-bank write: {:?}", target);
            return Err(());
        }

        let write_addr = bank_start + offset;

        if write_addr + data.len() > bank_end {
            defmt::error!(
                "Write would exceed bank boundary: addr={:#x}, len={}, end={:#x}",
                write_addr,
                data.len(),
                bank_end
            );
            return Err(());
        }

        defmt::debug!(
            "Writing {} bytes to {:?} at offset {:#x}",
            data.len(),
            target,
            write_addr
        );

        // Track netcore DFU progress
        if target == DfuTarget::NetCore {
            self.netcore_dfu_size = (offset + data.len()).max(self.netcore_dfu_size);

            // Emit coarse progress into the PCD reserved fields for debugging without RTT.
            let pcd = unsafe { pcd::PcdRegion::at_mut(PCD_REGION_ADDR as usize) };
            unsafe {
                core::ptr::write_volatile(
                    &mut pcd.status.reserved[0],
                    (offset + data.len()) as u32,
                );
                core::ptr::write_volatile(&mut pcd.status.reserved[1], 0x4E_5450u32); // 'NTP' Netcore progress
            }
        }

        self.flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();

            // Erase pages if needed (align to page boundaries)
            let page_start = write_addr & !(PAGE_SIZE - 1);
            let page_end = (write_addr + data.len() + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

            for page_addr in (page_start..page_end).step_by(PAGE_SIZE) {
                if let Err(_) = flash.erase(page_addr as u32, (page_addr + PAGE_SIZE) as u32) {
                    defmt::error!("Failed to erase page at {:#x}", page_addr);
                    return Err(());
                }
            }

            // Write data
            if let Err(_) = flash.write(write_addr as u32, data) {
                defmt::error!("Failed to write at {:#x}", write_addr);
                return Err(());
            }

            Ok(())
        })
    }

    fn mark_updated_for_target(&mut self, target: DfuTarget) -> Result<(), ()> {
        defmt::info!(
            "Marking {:?} as updated (direct XIP, no swap needed)",
            target
        );

        // Debug: dump the first two words of the target bank to verify vectors are present.
        let (bank_start, _) = self.get_bank_range(target);
        self.flash.lock(|flash_cell| {
            let _flash = flash_cell.borrow();
            let word0 = unsafe { core::ptr::read_volatile(bank_start as *const u32) };
            let word1 = unsafe { core::ptr::read_volatile((bank_start + 4) as *const u32) };
            defmt::info!(
                "Bank {:?} vectors: [0]={:#010x}, [1]={:#010x}",
                target,
                word0,
                word1
            );
        });

        // Persist which bank should be preferred on next boot.
        let actual_target = self.map_target(target);
        let target_slot = match actual_target {
            DfuTarget::AppBankA => BOOT_STATE_SLOT_A,
            DfuTarget::AppBankB => BOOT_STATE_SLOT_B,
            DfuTarget::NetCore => {
                defmt::info!(
                    "Setting netcore DFU pending via PCD, size: {}",
                    self.netcore_dfu_size
                );

                extern "C" {
                    static __bootloader_dfu_region_start: u32;
                }
                let dfu_addr = unsafe { &__bootloader_dfu_region_start as *const u32 as u32 };

                // Set PCD command in RAM (for soft reset)
                let pcd = get_pcd_region();
                pcd.status.clear();
                pcd.cmd.set_update(dfu_addr, self.netcore_dfu_size as u32);
                // Pad image size to page boundary to be safe for netcore copy.
                let padded = (self.netcore_dfu_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                unsafe {
                    core::ptr::write_volatile(&mut pcd.status.reserved[0], padded as u32);
                    core::ptr::write_volatile(&mut pcd.status.reserved[1], 0x4E_54_44u32); // 'NTD' marker
                }
                defmt::info!(
                    "PCD command set in RAM: addr={:#x}, size={} (padded {})",
                    dfu_addr,
                    self.netcore_dfu_size,
                    padded
                );

                // Also write to flash so it survives hard reset (USB DFU uses WILL_DETACH)
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
                defmt::info!("Bootloader DFU completed, will swap on reset");
                // Bootloader update uses swap mechanism via stage1
                // The DFU region already contains the new bootloader
                // Stage1 will perform the swap on next boot
                // Just trigger a reset to let stage1 handle it
                return Ok(());
            }
        };

        // Determine active slot based on which bank is currently running
        let active_slot = if self.active_bank_is_a {
            BOOT_STATE_SLOT_A
        } else {
            BOOT_STATE_SLOT_B
        };

        // Create boot state with correct active and pending slots
        let state = BootState {
            magic: BOOT_STATE_MAGIC,
            active_slot,
            pending_slot: target_slot,
            boot_counter: 0,
            _reserved: 0,
        };

        // Clear DFU state so the bootloader doesn't re-enter DFU on next boot.
        // Writing BOOT_MAGIC matches the state machine in check_dfu_state.
        // Bootloader state page symbols
        extern "C" {
            static __bootloader_state_start: u32;
        }
        let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };
        defmt::info!("Writing BOOT_MAGIC to state page at 0x{:08x}", state_addr);

        self.flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();

            // Erase containing page, then write BOOT_MAGIC.
            let page_start = state_addr & !(PAGE_SIZE - 1);
            let page_end = page_start + PAGE_SIZE;

            if let Err(_) = flash.erase(page_start as u32, page_end as u32) {
                defmt::error!("Failed to erase state page at {:#x}", page_start);
                return Err(());
            }

            // Write BOOT_MAGIC + boot state (active/pending/attempts)
            if let Err(_) = flash.write(state_addr as u32, &0xB007_0000u32.to_le_bytes()) {
                defmt::error!("Failed to write BOOT_MAGIC at {:#x}", state_addr);
                return Err(());
            }

            // Write boot state immediately after BOOT_MAGIC
            let state_bytes = unsafe {
                core::slice::from_raw_parts(&state as *const BootState as *const u8, core::mem::size_of::<BootState>())
            };
            if let Err(_) = flash.write((state_addr + 4) as u32, state_bytes) {
                defmt::error!("Failed to write boot state at {:#x}", state_addr + 4);
                return Err(());
            }

            // Read back to confirm
            let verify_magic = unsafe { core::ptr::read_volatile(state_addr as *const u32) };
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

/// Determine which bank to boot from based on validity and version
fn select_boot_bank(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> (usize, &'static str, BootState) {
    extern "C" {
        static __bootloader_app_a_start: u32;
        static __bootloader_app_b_start: u32;
    }

    unsafe {
        let bank_a_start = &__bootloader_app_a_start as *const u32 as usize;
        let bank_b_start = &__bootloader_app_b_start as *const u32 as usize;

        // Read the stack pointer from each bank's vector table
        let bank_a_sp = core::ptr::read_volatile(bank_a_start as *const u32);
        let bank_b_sp = core::ptr::read_volatile(bank_b_start as *const u32);

        // Check if stack pointer is valid (should be in RAM range 0x2000_0000 - 0x2008_0000)
        let bank_a_valid = bank_a_sp >= 0x2000_0000 && bank_a_sp < 0x2008_0000;
        let bank_b_valid = bank_b_sp >= 0x2000_0000 && bank_b_sp < 0x2008_0000;

        defmt::info!(
            "Bank A: addr={:#x}, sp={:#x}, valid={}",
            bank_a_start,
            bank_a_sp,
            bank_a_valid
        );
        defmt::info!(
            "Bank B: addr={:#x}, sp={:#x}, valid={}",
            bank_b_start,
            bank_b_sp,
            bank_b_valid
        );

        // Read boot state to honor pending/active preference with simple rollback.
        let mut state = read_boot_state(flash);
        let active_slot = state.active_slot;
        let pending_slot = state.pending_slot;

        defmt::info!(
            "Boot state: active_slot={}, pending_slot={}, attempts={}",
            active_slot,
            pending_slot,
            state.boot_counter
        );

        // Map slots to validity
        let slot_a_valid = bank_a_valid;
        let slot_b_valid = bank_b_valid;

        // Decide which slot to boot
        let mut chosen_slot = active_slot;

        // Try pending slot if it's different, valid, and attempts below limit
        if pending_slot != active_slot {
            let pending_valid = match pending_slot {
                BOOT_STATE_SLOT_A => slot_a_valid,
                BOOT_STATE_SLOT_B => slot_b_valid,
                _ => false,
            };

            if pending_valid {
                // Try pending slot once; increment boot_counter so rollback can occur on next boot if not confirmed.
                chosen_slot = pending_slot;
                state.boot_counter = state.boot_counter.saturating_add(1);
                defmt::info!("Booting pending slot {}", pending_slot);
            } else {
                // Pending invalid; clear pending
                state.pending_slot = active_slot;
                state.boot_counter = 0;
                defmt::warn!(
                    "Pending slot invalid; sticking with active slot {}",
                    active_slot
                );
            }
        }

        // Write back updated boot state (e.g., incremented attempts or cleared pending)
        if let Err(_) = write_boot_state(flash, &state) {
            defmt::warn!("Failed to persist boot state");
        }

        let (offset, name) = match chosen_slot {
            BOOT_STATE_SLOT_A if slot_a_valid => (bank_a_start, "Bank A"),
            BOOT_STATE_SLOT_B if slot_b_valid => (bank_b_start, "Bank B"),
            _ => {
                // Fallback behavior if chosen slot invalid
                if slot_a_valid {
                    defmt::warn!("Chosen slot invalid, falling back to Bank A");
                    (bank_a_start, "Bank A (fallback)")
                } else if slot_b_valid {
                    defmt::warn!("Chosen slot invalid, falling back to Bank B");
                    (bank_b_start, "Bank B (fallback)")
                } else {
                    defmt::warn!("No valid banks found! Attempting to boot Bank A anyway");
                    (bank_a_start, "Bank A (fallback)")
                }
            }
        };

        (offset, name, state)
    }
}

/// Check if bootloader should enter DFU mode by reading the state partition
fn check_dfu_state(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> bool {
    extern "C" {
        static __bootloader_state_start: u32;
    }

    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };
    const SWAP_MAGIC: u32 = 0x00BAD00D;

    // Read the state partition to check for DFU detach marker
    // Embassy-boot state format:
    // - 0xFFFFFFFF = Erased (no state) - boot normally
    // - 0xB0070000 = Booted - boot normally
    // - 0x00BAD00D = Swap/DFU requested - enter DFU mode
    flash.lock(|flash_cell| {
        let _flash = flash_cell.borrow();
        let state_word = unsafe { core::ptr::read_volatile(state_addr as *const u32) };
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

        // Enter DFU only if explicitly requested.
        state_word == SWAP_MAGIC
    })
}

fn read_boot_state(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> BootState {
    extern "C" {
        static __bootloader_state_start: u32;
    }
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };
    let state_ptr = state_addr + core::mem::size_of::<u32>();

    flash.lock(|_flash_cell| {
        let state = unsafe { core::ptr::read_volatile(state_ptr as *const BootState) };
        if state.magic == BOOT_STATE_MAGIC {
            state
        } else {
            BootState {
                magic: BOOT_STATE_MAGIC,
                active_slot: BOOT_STATE_SLOT_A,
                pending_slot: BOOT_STATE_SLOT_A,
                boot_counter: 0,
                _reserved: 0,
            }
        }
    })
}

fn write_boot_state(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    state: &BootState,
) -> Result<(), ()> {
    extern "C" {
        static __bootloader_state_start: u32;
    }
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };
    let state_ptr = state_addr + core::mem::size_of::<u32>();

    flash.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();

        // Erase containing page, then write state
        let page_start = state_addr & !(PAGE_SIZE - 1);
        let page_end = page_start + PAGE_SIZE;

        flash
            .erase(page_start as u32, page_end as u32)
            .map_err(|_| ())?;

        // First word reserved for BOOT_MAGIC (DFU state)
        flash
            .write(state_addr as u32, &0xB007_0000u32.to_le_bytes())
            .map_err(|_| ())?;

        let bytes = unsafe {
            core::slice::from_raw_parts(
                state as *const BootState as *const u8,
                core::mem::size_of::<BootState>(),
            )
        };

        flash.write(state_ptr as u32, bytes).map_err(|_| ())?;
        Ok(())
    })
}

fn snapshot_state_words() -> (u32, BootState) {
    extern "C" {
        static __bootloader_state_start: u32;
    }

    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };
    let flag_word = unsafe { core::ptr::read_volatile(state_addr as *const u32) };
    let state_ptr = state_addr + core::mem::size_of::<u32>();
    let raw_state = unsafe { core::ptr::read_volatile(state_ptr as *const BootState) };
    let state = if raw_state.magic == BOOT_STATE_MAGIC {
        raw_state
    } else {
        BootState {
            magic: BOOT_STATE_MAGIC,
            active_slot: BOOT_STATE_SLOT_A,
            pending_slot: BOOT_STATE_SLOT_A,
            boot_counter: 0,
            _reserved: 0,
        }
    };

    (flag_word, state)
}

fn rewrite_state_page_with_netcore(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    flag_word: u32,
    state: BootState,
    netcore_info: Option<(u32, u32)>,
) -> Result<(), ()> {
    extern "C" {
        static __bootloader_state_start: u32;
    }

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

        Ok(())
    })
}

fn clear_netcore_dfu_info(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
) -> Result<(), ()> {
    let (flag_word, state) = snapshot_state_words();
    rewrite_state_page_with_netcore(flash, flag_word, state, None)
}

fn persist_netcore_dfu_info(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    dfu_addr: u32,
    dfu_size: u32,
) -> Result<(), ()> {
    let (flag_word, state) = snapshot_state_words();
    rewrite_state_page_with_netcore(flash, flag_word, state, Some((dfu_addr, dfu_size)))
}

// Write DFU requested (SWAP_MAGIC) to the state word.
fn flag_dfu_requested(
    flash: &Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc<'_>>>,
    preserved_state: BootState,
) {
    // Set a RAM flag so the next boot will enter DFU even if flash writes fail.
    let app_dfu_flag = get_app_dfu_flag();
    app_dfu_flag.magic = APP_DFU_FLAG_MAGIC;
    app_dfu_flag.dfu_requested = 1;

    extern "C" {
        static __bootloader_state_start: u32;
    }
    const SWAP_MAGIC: u32 = 0x00BAD00D;
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };

    flash.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        // Preserve existing boot state while flipping the DFU flag word.
        // Prefer the state we already selected for boot; fall back to reading from flash.
        let state_ptr = state_addr + core::mem::size_of::<u32>();
        let mut current_state = preserved_state;
        let flash_state = unsafe { core::ptr::read_volatile(state_ptr as *const BootState) };
        if flash_state.magic == BOOT_STATE_MAGIC {
            current_state = flash_state;
        }
        if current_state.magic != BOOT_STATE_MAGIC {
            defmt::warn!(
                "State magic invalid when flagging DFU; RAM flag set, skipping flash update"
            );
            return;
        }

        let page_start = state_addr & !(PAGE_SIZE - 1);
        let page_end = page_start + PAGE_SIZE;

        // Erase the containing page so we can program the DFU flag word.
        if let Err(_) = flash.erase(page_start as u32, page_end as u32) {
            defmt::warn!("Failed to erase state page at {:#x}", page_start);
            return;
        }

        // Write SWAP_MAGIC to request DFU on next boot.
        if let Err(_) = flash.write(state_addr as u32, &SWAP_MAGIC.to_le_bytes()) {
            defmt::warn!("Failed to write SWAP_MAGIC at {:#x}", state_addr);
            return;
        }

        // Restore boot state immediately after the flag word.
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

        // Read back flag word for confirmation.
        let verify_flag = unsafe { core::ptr::read_volatile(state_addr as *const u32) };

        defmt::info!(
            "Flagged DFU requested (SWAP_MAGIC) at {:#x} (read-back={:#010x}); preserved state active={}, pending={}, attempts={}",
            state_addr,
            verify_flag,
            current_state.active_slot,
            current_state.pending_slot,
            current_state.boot_counter
        );

        // Ensure flash writes complete before reset.
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    });
}

/// Background task to feed the watchdog
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

    // Start watchdog (60s timeout for DFU operations)
    // This will continue running in the app, which MUST feed it
    let mut wdt_config = embassy_nrf::wdt::Config::default();
    wdt_config.timeout_ticks = 32768 * 60; // 60 second timeout
    wdt_config.action_during_sleep = embassy_nrf::wdt::SleepConfig::RUN;
    wdt_config.action_during_debug_halt = embassy_nrf::wdt::HaltConfig::PAUSE;
    let (_wdt, [wdt_handle]) = match embassy_nrf::wdt::Watchdog::try_new(p.WDT0, wdt_config) {
        Ok(wdt) => wdt,
        Err(_) => {
            defmt::error!("Failed to initialize watchdog");
            cortex_m::peripheral::SCB::sys_reset();
        }
    };

    // Spawn watchdog feeder task
    spawner
        .spawn(watchdog_feeder(wdt_handle))
        .expect("Failed to spawn watchdog feeder");

    let flash = Mutex::new(RefCell::new(Nvmc::new(p.NVMC)));

    // ============================================================================
    // DEBUG: Uncomment this block to always start the network core for debugging
    // ============================================================================
    // {
    //     defmt::info!("[DEBUG] Starting network core for debugging");
    //     use embassy_nrf::pac::reset::vals::Forceoff;
    //     use embassy_nrf::pac::spu::vals;

    //     unsafe {
    //         // Configure SPU to allow network core access
    //         let spu = embassy_nrf::pac::SPU_S;

    //         // Configure EXTDOMAIN[0] for network core access
    //         spu.extdomain(0).perm().write(|w| {
    //             w.set_securemapping(vals::ExtdomainPermSecuremapping::NON_SECURE);
    //         });

    //         // Erratum 161 workaround
    //         (0x50005618 as *mut u32).write_volatile(1);

    //         // Release network core reset
    //         embassy_nrf::pac::RESET_S
    //             .network()
    //             .forceoff()
    //             .write(|w| w.set_forceoff(Forceoff::RELEASE));

    //         // Delay 5us (at 64MHz)
    //         cortex_m::asm::delay(5 * 64);

    //         // Hold
    //         embassy_nrf::pac::RESET_S
    //             .network()
    //             .forceoff()
    //             .write(|w| w.set_forceoff(Forceoff::HOLD));

    //         // Delay 1us (at 64MHz)
    //         cortex_m::asm::delay(1 * 64);

    //         // Release again
    //         embassy_nrf::pac::RESET_S
    //             .network()
    //             .forceoff()
    //             .write(|w| w.set_forceoff(Forceoff::RELEASE));

    //         // Clear erratum workaround
    //         (0x50005618 as *mut u32).write_volatile(0);
    //     }
    //     defmt::info!("[DEBUG] Network core started for debugging");
    // }
    // ============================================================================

    // Check if network core DFU is pending via PCD
    // First check any status left by the netcore bootloader, then populate RAM from flash
    let pcd = get_pcd_region();

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

    // If there is no RAM command, repopulate from flash (survives reset)
    if !pcd.cmd.is_valid() {
        unsafe {
            extern "C" {
                static __bootloader_state_start: u32;
            }
            let state_addr = &__bootloader_state_start as *const u32 as usize;
            let netcore_magic = core::ptr::read_volatile((state_addr + 12) as *const u32);

            if netcore_magic == NETCORE_DFU_MAGIC {
                // Read from flash and populate PCD in RAM
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

    // Always print PCD status for debugging
    defmt::info!(
        "PCD region: magic={:#x}, addr={:#x}, size={}",
        unsafe { core::ptr::read_volatile(&pcd.cmd.magic as *const _ as *const u32) },
        pcd.cmd.get_image_addr(),
        pcd.cmd.get_image_size()
    );

    // If netcore bootloader already reported a terminal status, clear persistence and skip DFU.
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

    // If both cmd and status are cleared, there is no pending netcore DFU.
    // If both cmd and status are cleared, there is no pending netcore DFU.
    let mut netcore_dfu_pending = pcd.cmd.is_valid() && pcd.status.is_valid();

    if netcore_dfu_pending {
        defmt::info!("PCD command VALID - will start netcore bootloader");
    } else {
        defmt::info!("PCD command INVALID - skipping netcore DFU");
    }

    // Start network core if there's a pending DFU update
    if netcore_dfu_pending {
        defmt::info!("Network core DFU pending, starting network core bootloader");
        use embassy_nrf::pac::reset::vals::Forceoff;
        use embassy_nrf::pac::spu::vals;

        unsafe {
            let spu = embassy_nrf::pac::SPU_S;

            // Open all flash to the net core (app + net)
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

            // EXTDOMAIN for net core
            spu.extdomain(0).perm().write(|w| {
                w.set_securemapping(vals::ExtdomainPermSecuremapping::NON_SECURE);
                w.set_lock(false);
            });

            // Open ICMSG SRAM regions
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

            // Erratum 161
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

        // Poll for netcore bootloader result without timeout.
        // For large images (up to 196KB), this could take significant time.
        // We poll indefinitely but with a safety mechanism: if we exceed 2 minutes,
        // we trigger a system reset to retry on next boot.
        let poll = async {
            let mut ticks: u32 = 0;
            const MAX_TICKS: u32 = 1200; // 2 minutes at 100ms per tick
            loop {
                if let Some(status) = pcd.status.get_status() {
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

                // Safety mechanism: if we've been waiting too long (2 minutes), reset
                if ticks >= MAX_TICKS {
                    defmt::error!(
                        "Netcore DFU taking too long (>2 min) - resetting to retry on next boot"
                    );
                    cortex_m::peripheral::SCB::sys_reset();
                }

                if ticks % 10 == 0 {
                    let magic = unsafe {
                        core::ptr::read_volatile(&pcd.status.magic as *const _ as *const u32)
                    };
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
        // Poll indefinitely (with internal safety reset at 2 minutes)
        poll.await;
    }

    // Check locked RAM flag set by runtime detach to force DFU entry.
    let mut ram_dfu_request = false;
    let app_dfu_flag = get_app_dfu_flag();
    if app_dfu_flag.magic == APP_DFU_FLAG_MAGIC && app_dfu_flag.dfu_requested != 0 {
        ram_dfu_request = true;
        app_dfu_flag.dfu_requested = 0;
        defmt::info!("DFU requested via RAM flag");
    }

    // If netcore DFU is pending via PCD, do not enter app DFU even if SWAP_MAGIC is set.
    let mut should_enter_dfu = ram_dfu_request || check_dfu_state(&flash);
    if netcore_dfu_pending && should_enter_dfu {
        defmt::info!("Clearing app DFU request because netcore DFU is pending");
        should_enter_dfu = false;
        // Clear SWAP_MAGIC so we don't loop in DFU.
        let mut state = read_boot_state(&flash);
        state.boot_counter = 0;
        let _ = write_boot_state(&flash, &state);
    }
    defmt::info!(
        "DFU decision: should_enter_dfu={} netcore_dfu_pending={}",
        should_enter_dfu,
        netcore_dfu_pending
    );

    // Select which bank to boot from (dual-bank logic)
    let (boot_offset, boot_bank_name, _boot_state) = select_boot_bank(&flash);

    if should_enter_dfu {
        defmt::info!("DFU mode");

        defmt::info!("Creating USB driver with VBUS detect");
        let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
        defmt::info!("USB driver created");
        let mut config = embassy_usb::Config::new(0x1915, 0x5211);
        config.manufacturer = Some("Nordic");
        config.product = Some("USB-DFU Bootloader");
        config.serial_number = Some("1235678");

        // Create a dummy updater - we use the legacy symbols which point to bank_a/bank_b
        // This satisfies the Control type signature but won't be used for dual-bank -
        // all actual writes go through DualBankWriter
        let fw_config =
            embassy_boot::FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
        let mut buffer = embassy_boot::AlignedBuffer([0; 4]);
        let updater = embassy_boot::BlockingFirmwareUpdater::new(fw_config, &mut buffer.0[..]);

        let mut config_descriptor = [0; 256];
        let mut bos_descriptor = [0; 256];
        let mut msos_descriptor = [0; 256];
        let mut control_buf = [0; 4096];

        // Create DFU control with dual-bank writer and IPC forwarder
        defmt::info!("Creating DFU control with dual-bank and network core support");

        // Determine which bank is active (the one we would have booted)
        // This determines which alt interfaces to show:
        // - If Bank A is active, show "Bank B" for download (upload to inactive bank)
        // - If Bank B is active, show "Bank A" for download (upload to inactive bank)
        let (_, boot_bank_name, boot_state) = select_boot_bank(&flash);
        let active_bank_is_a = boot_bank_name.starts_with("Bank A");
        defmt::info!("Active bank for DFU: {}", boot_bank_name);

        // Create IPC forwarder (will initialize on demand when alt=2 is selected)
        let forwarder = IpcForwarder::new(p.IPC);

        // Create dual-bank flash writer for bank A and B
        let dual_bank_writer = DualBankFlashWriter::new(&flash, active_bank_is_a);

        // Flagger to mark DFU requested on detach/reset
        let mut dfu_flagger = {
            let boot_state = boot_state;
            let flash_ref = &flash;
            move || flag_dfu_requested(flash_ref, boot_state)
        };

        let mut state = Control::new(
            updater,
            // Request self-detach so the device resets itself after manifest.
            DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
            ResetImmediate,
            forwarder,
            dual_bank_writer,
            active_bank_is_a,
        );
        state.set_state_flagger(&mut dfu_flagger);

        let mut builder = Builder::new(
            driver,
            config,
            &mut config_descriptor,
            &mut bos_descriptor,
            &mut msos_descriptor,
            &mut control_buf,
        );

        builder.msos_descriptor(msos::windows_version::WIN8_1, 2);
        builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));

        usb_dfu::<_, _, _, _, _, _, 4096>(&mut builder, &mut state, |_func| {});

        let mut dev = builder.build();

        defmt::info!("USB device built, starting run loop");
        dev.run().await;
    }

    // Final safety check: ensure netcore DFU is not in progress
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

    // Stop the network core before booting the app to prevent double initialization
    // The app will start the network core itself
    {
        use embassy_nrf::pac::reset::vals::Forceoff;

        defmt::info!("Stopping network core before booting app");
        embassy_nrf::pac::RESET_S
            .network()
            .forceoff()
            .write(|w| w.set_forceoff(Forceoff::HOLD));
        defmt::info!("Network core stopped");
    }

    // Lock the locked RAM region before booting to prevent app from tampering
    {
        extern "C" {
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

        // Calculate which RAM region(s) to lock (each region is 8KB on nRF5340)
        const REGION_SIZE: usize = 8192;
        let start_region = (locked_ram_start - 0x2000_0000) / REGION_SIZE;
        let end_region = (locked_ram_end - 0x2000_0000 + REGION_SIZE - 1) / REGION_SIZE;

        // Lock all regions that contain the locked RAM
        for region in start_region..end_region {
            spu.ramregion(region).perm().modify(|w| {
                w.set_write(false); // Lock writes
            });
            defmt::info!("Locked RAM region {}", region);
        }
    }

    // Mark this boot as successful to prevent stage1 from rolling back
    // IMPORTANT: This must be called ONLY when booting the app, NOT when entering DFU mode
    // Otherwise it would clear the DFU request flag (SWAP_MAGIC) that the app wrote
    {
        use embassy_boot::{AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig};

        let config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
        let mut aligned_buf = AlignedBuffer([0; 4]);
        let mut updater = BlockingFirmwareUpdater::new(config, &mut aligned_buf.0);

        defmt::info!("Marking boot as successful before booting app");
        if let Err(e) = updater.mark_booted() {
            defmt::error!("Failed to mark boot: {:?}", e);
        }
    }

    defmt::info!(
        "Booting application from {} at 0x{:08x}",
        boot_bank_name,
        boot_offset
    );

    unsafe {
        let mut p = cortex_m::Peripherals::steal();
        cortex_m::interrupt::disable();
        cortex_m::interrupt::enable();
        p.SCB.invalidate_icache();
        p.SCB.vtor.write(boot_offset as u32);
        cortex_m::asm::bootload(boot_offset as *const u32)
    }
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
    let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;
    panic!("DefaultHandler #{:?}", irqn);
}
