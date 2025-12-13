#![no_std]
#![no_main]

use core::cell::RefCell;

use defmt::*;
use defmt_rtt as _;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_sync::blocking_mutex::{Mutex, raw::NoopRawMutex};
use embassy_time::{Duration, Timer};
use embedded_storage::nor_flash::NorFlash;
use nrf_pac as pac;
use panic_probe as _;
use pcd::{PcdRegion, PcdStatusCode};

const FLASH_BASE: u32 = 0x0100_0000;
const FLASH_SIZE: u32 = 256 * 1024;

// PCD region in ICMSG shared SRAM (netcore view: app RX)
fn pcd_region_addr() -> usize {
    extern "C" {
        static __pcd_region_start: u32;
    }
    unsafe { &__pcd_region_start as *const u32 as usize }
}

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let p = embassy_nrf::init(Default::default());

    // Uncomment this if you are debugging the bootloader with debugger/RTT attached,
    // as it prevents a hard fault when accessing flash 'too early' after boot.
    for _ in 0..10000000 {
        cortex_m::asm::nop();
    }

    info!("NetCore bootloader starting");

    // Allow debugger attach before touching flash.
    // Timer::after(Duration::from_secs(5)).await;
    // info!("Attach window done, proceeding");
    let flash: Mutex<NoopRawMutex, RefCell<Nvmc<'_>>> = Mutex::new(RefCell::new(Nvmc::new(p.NVMC)));

    // Access PCD region in shared SRAM
    let pcd = unsafe { PcdRegion::at_mut(pcd_region_addr()) };

    if pcd.cmd.is_valid() {
        pcd.status.set_status(PcdStatusCode::Updating);

        let image_addr = pcd.cmd.get_image_addr();
        let image_size = pcd.cmd.get_image_size();
        info!(
            "PCD valid: addr={:#x}, size={} (reserved0={}, reserved1={})",
            image_addr,
            image_size,
            unsafe { core::ptr::read_volatile(&pcd.status.reserved[0]) },
            unsafe { core::ptr::read_volatile(&pcd.status.reserved[1]) }
        );

        let result = flash.lock(|flash_cell| {
            let mut nvmc = flash_cell.borrow_mut();
            perform_update(&mut nvmc, image_addr, image_size)
        });

        pcd.cmd.clear();

        match result {
            Ok(()) => {
                info!("Update copy/verify complete");
                pcd.status.set_status(PcdStatusCode::Success);
                // Ensure status is visible to app core before any further actions.
                cortex_m::asm::dsb();
                cortex_m::asm::isb();
                clear_state_flag(&flash);
            }
            Err(()) => {
                warn!("Update failed (copy/verify)");
                pcd.status.set_status(PcdStatusCode::FailedCopy);
            }
        }
    } else {
        info!("No PCD cmd: magic=0x{:08x}", unsafe {
            core::ptr::read_volatile(&pcd.cmd.magic as *const _ as *const u32)
        });
    }

    info!("Booting net application");
    boot_app()
}

/// Boot the network core application
fn boot_app() -> ! {
    extern "C" {
        static __bootloader_active_start: u32;
    }

    unsafe {
        let mut p = cortex_m::Peripherals::steal();

        cortex_m::interrupt::disable();
        cortex_m::interrupt::enable();

        // Invalidate instruction cache
        p.SCB.invalidate_icache();

        // Set vector table offset
        let active = &__bootloader_active_start as *const u32 as u32;
        p.SCB.vtor.write(active);

        // Jump to application
        cortex_m::asm::bootload(active as *const u32)
    }
}

fn clear_state_flag(flash: &Mutex<NoopRawMutex, RefCell<Nvmc<'_>>>) {
    extern "C" {
        static __bootloader_state_start: u32;
        static __bootloader_state_end: u32;
    }

    let start = unsafe { &__bootloader_state_start as *const u32 as u32 };
    let end = unsafe { &__bootloader_state_end as *const u32 as u32 };

    flash.lock(|_flash_cell| {
        let _ = erase_abs(start, end);
    });
}

/// Copy firmware image from app flash (dfu slot) into netcore active slot.
fn perform_update(_nvmc: &mut Nvmc<'_>, src_addr: u32, size: u32) -> Result<(), ()> {
    extern "C" {
        static __bootloader_active_start: u32;
        static __bootloader_active_end: u32;
        static __bootloader_dfu_start: u32;
        static __bootloader_dfu_end: u32;
    }

    let dst_addr = unsafe { &__bootloader_active_start as *const u32 as u32 };
    let dst_end = unsafe { &__bootloader_active_end as *const u32 as u32 };
    let dfu_start = unsafe { &__bootloader_dfu_start as *const u32 as u32 };
    let dfu_end = unsafe { &__bootloader_dfu_end as *const u32 as u32 };

    let active_len = dst_end.saturating_sub(dst_addr);
    let _dfu_len = dfu_end.saturating_sub(dfu_start);
    info!(
        "Dest range {:#x}-{:#x}, active_len {}, src range {:#x}-{:#x}, dfu_len {}",
        dst_addr, dst_end, active_len, dfu_start, dfu_end, _dfu_len
    );

    if size == 0 || size > active_len {
        warn!("Invalid size {}, active_len {}", size, active_len);
        return Err(());
    }
    if src_addr < dfu_start || src_addr + size > dfu_end {
        warn!(
            "Src out of range: addr {:#x} len {} (dfu {:#x}-{:#x})",
            src_addr, size, dfu_start, dfu_end
        );
        return Err(());
    }

    // Erase destination range page-by-page.
    let end = dst_addr + size;
    let page_aligned_end = (end + PAGE_SIZE as u32 - 1) & !(PAGE_SIZE as u32 - 1);
    info!(
        "Erase/write dst {:#x}-{:#x}, src {:#x}-{:#x}",
        dst_addr,
        page_aligned_end,
        src_addr,
        src_addr + size
    );
    if let Err(_e) = erase_abs(dst_addr, page_aligned_end) {
        warn!("NVMC erase error");
        return Err(());
    }

    let src = unsafe { core::slice::from_raw_parts(src_addr as *const u8, size as usize) };
    if let Err(_e) = write_abs(dst_addr, src) {
        warn!("NVMC write error");
        return Err(());
    }
    // Ensure flash write completes before verify.
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    verify_copy(dst_addr, src)
}

fn verify_copy(dst_addr: u32, src: &[u8]) -> Result<(), ()> {
    // Compare the first 16 bytes to catch obvious failures.
    let mut buf = [0u8; 16];
    let len = core::cmp::min(buf.len(), src.len());
    unsafe {
        core::ptr::copy_nonoverlapping(dst_addr as *const u8, buf.as_mut_ptr(), len);
    }
    info!("Verify len {}", len);
    info!("Flash head: {=[u8]:x}", &buf[..len]);
    info!("Src   head: {=[u8]:x}", &src[..len]);
    if buf[..len] == src[..len] {
        info!("Verify OK ({} bytes checked)", len);
        Ok(())
    } else {
        warn!(
            "Verify failed: flash {:?} != src {:?}",
            &buf[..len],
            &src[..len]
        );
        Err(())
    }
}

fn check_bounds(start: u32, end: u32) -> Result<(), ()> {
    if start < FLASH_BASE || end > FLASH_BASE + FLASH_SIZE || end < start {
        return Err(());
    }
    if start % PAGE_SIZE as u32 != 0 || end % PAGE_SIZE as u32 != 0 {
        return Err(());
    }
    Ok(())
}

fn wait_ready() {
    let regs = pac::NVMC_NS;
    while !regs.ready().read().ready() {}
}

fn wait_ready_write() {
    let regs = pac::NVMC_NS;
    while !regs.readynext().read().readynext() {}
}

fn erase_abs(start: u32, end: u32) -> Result<(), ()> {
    check_bounds(start, end)?;

    let regs = pac::NVMC_NS;
    regs.config()
        .write(|w| w.set_wen(pac::nvmc::vals::Wen::EEN));
    wait_ready();

    for page_addr in (start..end).step_by(PAGE_SIZE) {
        unsafe {
            (page_addr as *mut u32).write_volatile(0xFFFF_FFFF);
        }
        wait_ready();
    }

    let regs = pac::NVMC_NS;
    regs.config()
        .write(|w| w.set_wen(pac::nvmc::vals::Wen::REN));
    wait_ready();

    Ok(())
}

fn write_abs(addr: u32, bytes: &[u8]) -> Result<(), ()> {
    if addr < FLASH_BASE
        || addr as usize + bytes.len() > FLASH_BASE as usize + FLASH_SIZE as usize
        || addr % 4 != 0
        || bytes.len() % 4 != 0
    {
        return Err(());
    }

    let regs = pac::NVMC_NS;
    regs.config()
        .write(|w| w.set_wen(pac::nvmc::vals::Wen::WEN));
    wait_ready();

    unsafe {
        let p_src = bytes.as_ptr() as *const u32;
        let p_dst = addr as *mut u32;
        let words = bytes.len() / 4;
        for i in 0..words {
            let w = core::ptr::read_unaligned(p_src.add(i));
            core::ptr::write_volatile(p_dst.add(i), w);
            wait_ready_write();
        }
    }

    let regs = pac::NVMC_NS;
    regs.config()
        .write(|w| w.set_wen(pac::nvmc::vals::Wen::REN));
    wait_ready();

    Ok(())
}
