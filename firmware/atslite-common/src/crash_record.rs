//! Crash record kept in `.uninit` RAM across the reset that follows a panic or hard fault,
//! and logged on the next boot.
//!
//! The handlers reset with SYSRESETREQ, which keeps RAM; power loss does not. The
//! bootloaders run in between, so a checksum rejects a record they overwrote.

use core::mem::MaybeUninit;
use core::panic::PanicInfo;

use cortex_m::peripheral::SCB;
use cortex_m_rt::ExceptionFrame;

const MAGIC: u32 = 0x4352_4153;

/// What ended the previous boot.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u32)]
pub enum CrashKind {
    Panic = 1,
    /// A hard fault, including a UsageFault escalated from an MSPLIM stack overflow.
    Fault = 2,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct CrashRecord {
    magic: u32,
    check: u32,
    kind: u32,
    pc: u32,
    lr: u32,
    cfsr: u32,
}

impl CrashRecord {
    fn checksum(&self) -> u32 {
        MAGIC ^ self.kind ^ self.pc ^ self.lr ^ self.cfsr
    }

    fn kind(&self) -> Option<CrashKind> {
        match self.kind {
            1 => Some(CrashKind::Panic),
            2 => Some(CrashKind::Fault),
            _ => None,
        }
    }
}

#[unsafe(link_section = ".uninit.CRASH_RECORD")]
static mut RECORD: MaybeUninit<CrashRecord> = MaybeUninit::uninit();

fn read() -> Option<CrashRecord> {
    // SAFETY: every bit pattern is a valid `CrashRecord` (plain integers), and the only
    // writers are the panic and fault handlers, which never run concurrently with this.
    let record = unsafe { (&raw const RECORD).cast::<CrashRecord>().read_volatile() };
    let valid =
        record.magic == MAGIC && record.check == record.checksum() && record.kind().is_some();
    valid.then_some(record)
}

fn store(kind: CrashKind, pc: u32, lr: u32) {
    let mut record = CrashRecord {
        magic: MAGIC,
        check: 0,
        kind: kind as u32,
        pc,
        lr,
        // SAFETY: a read of the always-mapped System Control Block.
        cfsr: unsafe { (*SCB::PTR).cfsr.read() },
    };
    record.check = record.checksum();
    // SAFETY: the record is static and only touched by this module's handlers.
    unsafe {
        (&raw mut RECORD)
            .cast::<CrashRecord>()
            .write_volatile(record)
    };
}

/// Logs and clears the record left by the previous boot, with the reset reason.
pub fn report_previous_boot() {
    // Read only: the boards' boot VBUS policy reads and clears RESETREAS after this.
    let reason = embassy_nrf::pac::RESET.resetreas().read();
    defmt::info!("reset reason RESETREAS={=u32:#010x}", reason.0);

    let Some(record) = read() else { return };
    defmt::warn!(
        "previous boot crashed: {} pc={=u32:#010x} lr={=u32:#010x} cfsr={=u32:#010x}",
        record.kind(),
        record.pc,
        record.lr,
        record.cfsr
    );
    // SAFETY: runs once from thread mode during init, before any handler can store.
    unsafe { (&raw mut RECORD).cast::<u32>().write_volatile(0) };
}

// shortcut: records no panic location. Reading `PanicInfo` keeps every panic site's
// `Location` and file path alive through LTO, +61 KB that lite1's flash lacks; build lite1
// with `-Z location-detail=line` like legacy-atslite to afford it. A probe still halts here
// with the message, and `defmt::panic!` / `defmt::unwrap!` log theirs over RTT.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    store(CrashKind::Panic, 0, 0);
    // UsageFault is disabled, so `udf` escalates to HardFault: an attached probe halts there
    // with the panic's stack intact, and otherwise the handler below resets.
    cortex_m::asm::udf()
}

// An attached probe with the default hardfault vector catch halts before this runs, so
// backtraces survive. Always reset rather than `bkpt` when DHCSR.C_DEBUGEN is set: the bit
// stays set after the probe detaches, and the watchdog pauses during debug halt.
#[cortex_m_rt::exception]
unsafe fn HardFault(frame: &ExceptionFrame) -> ! {
    // A panic already stored its record before raising this fault; keep it.
    if read().is_none() {
        store(CrashKind::Fault, frame.pc(), frame.lr());
    }
    SCB::sys_reset()
}
