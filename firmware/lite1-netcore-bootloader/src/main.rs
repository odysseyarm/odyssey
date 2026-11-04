#![no_std]
#![no_main]

use core::cell::RefCell;
#[cfg(feature = "defmt")]
use core::sync::atomic::Ordering;

#[cfg(feature = "rtt")]
use defmt_rtt as _;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use lite1_pcd::{PcdRegion, PcdStatusCode};
use nrf_pac as pac;
#[cfg(not(feature = "rtt"))]
use panic_halt as _;
#[cfg(feature = "rtt")]
use panic_probe as _;

const FLASH_BASE: u32 = 0x0100_0000;
const FLASH_SIZE: u32 = 256 * 1024;

// The PCD region sits in the app core's ICMSG RX buffer, which is the netcore's TX side.
fn pcd_region_addr() -> usize {
    unsafe extern "C" {
        static __pcd_region_start: u32;
    }
    unsafe { &__pcd_region_start as *const u32 as usize }
}

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let p = embassy_nrf::init(Default::default());

    // With a debugger or RTT attached, touching flash this soon after boot
    // hard-faults; enable this delay when debugging.
    // for _ in 0..10000000 {
    //     cortex_m::asm::nop();
    // }

    #[cfg(feature = "defmt")]
    defmt::info!("NetCore bootloader starting");

    let flash: Mutex<NoopRawMutex, RefCell<Nvmc<'_>>> = Mutex::new(RefCell::new(Nvmc::new(p.NVMC)));

    let pcd = unsafe { PcdRegion::at(pcd_region_addr()) };

    if pcd.cmd.is_valid() {
        pcd.status.set_status(PcdStatusCode::Updating);

        let image_addr = pcd.cmd.get_image_addr();
        let image_size = pcd.cmd.get_image_size();
        #[cfg(feature = "defmt")]
        defmt::info!(
            "PCD valid: addr={:#x}, size={} (reserved0={}, reserved1={})",
            image_addr,
            image_size,
            pcd.status.reserved[0].load(Ordering::Acquire),
            pcd.status.reserved[1].load(Ordering::Acquire)
        );

        let result = flash.lock(|flash_cell| {
            let mut nvmc = flash_cell.borrow_mut();
            perform_update(&mut nvmc, image_addr, image_size)
        });

        pcd.cmd.clear();

        match result {
            Ok(()) => {
                #[cfg(feature = "defmt")]
                defmt::info!("Update copy/verify complete");
                pcd.status.set_status(PcdStatusCode::Success);
                // The app core must see the status before the state page is erased.
                cortex_m::asm::dsb();
                cortex_m::asm::isb();
                clear_state_flag(&flash);
            }
            Err(()) => {
                #[cfg(feature = "defmt")]
                defmt::warn!("Update failed (copy/verify)");
                pcd.status.set_status(PcdStatusCode::FailedCopy);
            }
        }
    } else {
        #[cfg(feature = "defmt")]
        defmt::info!(
            "No PCD cmd: magic=0x{:08x}",
            pcd.cmd.magic.load(Ordering::Acquire)
        );
    }

    #[cfg(feature = "defmt")]
    defmt::info!("Booting net application");
    boot_app()
}

/// Jumps to the network core application at `__bootloader_active_start`.
fn boot_app() -> ! {
    unsafe extern "C" {
        static __bootloader_active_start: u32;
    }

    unsafe {
        let mut p = cortex_m::Peripherals::steal();

        cortex_m::interrupt::disable();
        cortex_m::interrupt::enable();

        p.SCB.invalidate_icache();

        let active = &__bootloader_active_start as *const u32 as u32;
        p.SCB.vtor.write(active);

        cortex_m::asm::bootload(active as *const u32)
    }
}

fn clear_state_flag(flash: &Mutex<NoopRawMutex, RefCell<Nvmc<'_>>>) {
    unsafe extern "C" {
        static __bootloader_state_start: u32;
        static __bootloader_state_end: u32;
    }

    let start = unsafe { &__bootloader_state_start as *const u32 as u32 };
    let end = unsafe { &__bootloader_state_end as *const u32 as u32 };

    flash.lock(|_flash_cell| {
        let _ = erase_abs(start, end);
    });
}

/// Copies the image from the DFU slot in app flash into the netcore active slot.
fn perform_update(_nvmc: &mut Nvmc<'_>, src_addr: u32, size: u32) -> Result<(), ()> {
    unsafe extern "C" {
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
    #[cfg(feature = "defmt")]
    defmt::info!(
        "Dest range {:#x}-{:#x}, active_len {}, src range {:#x}-{:#x}, dfu_len {}",
        dst_addr,
        dst_end,
        active_len,
        dfu_start,
        dfu_end,
        _dfu_len
    );

    if size == 0 || size % 4 != 0 || size > active_len {
        #[cfg(feature = "defmt")]
        defmt::warn!("Invalid size {}, active_len {}", size, active_len);
        return Err(());
    }
    if src_addr < dfu_start || src_addr > dfu_end || size > dfu_end - src_addr {
        #[cfg(feature = "defmt")]
        defmt::warn!(
            "Src out of range: addr {:#x} len {} (dfu {:#x}-{:#x})",
            src_addr,
            size,
            dfu_start,
            dfu_end
        );
        return Err(());
    }

    let end = dst_addr + size;
    let page_aligned_end = (end + PAGE_SIZE as u32 - 1) & !(PAGE_SIZE as u32 - 1);
    #[cfg(feature = "defmt")]
    defmt::info!(
        "Erase/write dst {:#x}-{:#x}, src {:#x}-{:#x}",
        dst_addr,
        page_aligned_end,
        src_addr,
        src_addr + size
    );
    if let Err(_e) = erase_abs(dst_addr, page_aligned_end) {
        #[cfg(feature = "defmt")]
        defmt::warn!("NVMC erase error");
        return Err(());
    }

    let src = unsafe { core::slice::from_raw_parts(src_addr as *const u8, size as usize) };
    if let Err(_e) = write_abs(dst_addr, src) {
        #[cfg(feature = "defmt")]
        defmt::warn!("NVMC write error");
        return Err(());
    }
    // Flash writes must complete before reading the copy back.
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    // Both complete ranges were validated above; do not confirm an image based on its vector
    // table alone, which can match even when a later page was not copied correctly.
    let dst = unsafe { core::slice::from_raw_parts(dst_addr as *const u8, size as usize) };
    if dst == src {
        #[cfg(feature = "defmt")]
        defmt::info!("Verify OK ({} bytes checked)", size);
        Ok(())
    } else {
        #[cfg(feature = "defmt")]
        defmt::warn!("Verify failed: copied image differs from source");
        Err(())
    }
}

fn check_bounds(start: u32, end: u32) -> Result<(), ()> {
    if start < FLASH_BASE || end > FLASH_BASE + FLASH_SIZE || end < start {
        return Err(());
    }
    if !start.is_multiple_of(PAGE_SIZE as u32) || !end.is_multiple_of(PAGE_SIZE as u32) {
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
    if !(FLASH_BASE..=FLASH_BASE + FLASH_SIZE).contains(&addr)
        || bytes.len() > (FLASH_BASE + FLASH_SIZE - addr) as usize
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
