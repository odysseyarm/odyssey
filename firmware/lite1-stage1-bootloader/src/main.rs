#![no_std]
#![no_main]
#![expect(
    unsafe_code,
    reason = "Stage-one boot transfer and exception handlers require raw peripheral access"
)]

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_boot_nrf::*;
use embassy_nrf::nvmc::Nvmc;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
#[cfg(not(feature = "defmt"))]
use panic_halt as _;
#[cfg(feature = "defmt")]
use panic_probe as _;
use release_signing::Component;

mod netcore_hash {
    include!(concat!(env!("OUT_DIR"), "/netcore_bootloader_hash.rs"));
}
/// Read only by provisioning: the assembler refuses a network bootloader other
/// than the one this stage1 was built with. The app core cannot read network
/// flash; the network bootloader write-protects itself instead.
#[used]
#[unsafe(link_section = ".netcore_bootloader_hash")]
static NETCORE_BOOTLOADER_HASH_RECORD: [u8; 32] = netcore_hash::NETCORE_BOOTLOADER_SHA256;

#[used]
#[unsafe(link_section = ".recovery_anchors")]
static RECOVERY_ANCHORS: [release_signing::PublicKey; release_signing::KEY_SLOTS] = [
    release_signing::RELEASE_KEYS[0],
    release_signing::RELEASE_KEYS[1],
    release_signing::RELEASE_KEYS[2],
    {
        #[cfg(feature = "dev-signing-key")]
        {
            release_signing::DEV_KEY
        }
        #[cfg(not(feature = "dev-signing-key"))]
        {
            release_signing::RELEASE_KEYS[3]
        }
    },
];

#[used]
#[unsafe(link_section = ".recovery_threshold")]
static RECOVERY_THRESHOLD: u32 = if cfg!(feature = "dev-signing-key") {
    1
} else {
    2
};

#[entry]
fn main() -> ! {
    use embassy_nrf::pac::reset::vals::Forceoff;

    embassy_nrf::pac::RESET_S
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::HOLD));
    cc312_verifier::initialize_shared_verifier();
    let p = embassy_nrf::init(Default::default());
    lite1_secure_boot::self_test().unwrap_or_else(|_| panic!("crypto self-test failed"));
    let policy = lite1_secure_boot::hardware::boot_policy()
        .unwrap_or_else(|_| panic!("boot trust policy rejected"));

    let flash = Nvmc::new(p.NVMC);
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(flash));
    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();
    let bl: BootLoader = BootLoader::prepare(config);

    let slot = unsafe { core::slice::from_raw_parts(active_offset as *const u8, 28 * 1024) };
    lite1_secure_boot::verify_slot(
        slot,
        active_offset,
        Component::Lite1SecureBootloader,
        policy.keys(),
    )
    .unwrap_or_else(|_| panic!("active stage2 verification failed"));
    let staging_address = lite1_secure_boot::hardware::staging_address()
        .unwrap_or_else(|_| panic!("policy storage unavailable"));
    lite1_secure_boot::handoff::publish(&policy, staging_address);
    unsafe {
        let spu = embassy_nrf::pac::SPU_S;
        for region in 0..2 {
            spu.flashregion(region).perm().write(|w| {
                w.set_read(true);
                w.set_write(false);
                w.set_execute(true);
                w.set_secattr(true);
                w.set_lock(true);
            });
        }
        // The accepted trust policy lives in this unit; nothing after stage1 may write it.
        spu.flashregion(lite1_secure_boot::hardware::state_spu_region())
            .perm()
            .write(|w| {
                w.set_read(true);
                w.set_write(false);
                w.set_execute(false);
                w.set_secattr(true);
                w.set_lock(true);
            });
        spu.ramregion(0).perm().write(|w| {
            w.set_read(true);
            w.set_write(true);
            w.set_execute(false);
            w.set_secattr(true);
            w.set_lock(true);
        });
        // A successful boot gives the next failure a fresh set of attempts.
        let attempts = &raw mut BOOT_ATTEMPTS as *mut [u32; 2];
        attempts.write_volatile([ATTEMPTS_MAGIC, 0]);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        bl.load(active_offset + lite1_secure_boot::SLOT_HEADER_LEN as u32)
    }
}

/// Fatal boot failures retry this many times, then halt.
const MAX_BOOT_ATTEMPTS: u32 = 3;
const ATTEMPTS_MAGIC: u32 = 0x5331_4154;

/// Magic and failure count. Uninitialized RAM survives the soft reset of a retry
/// but not power loss, so a power cycle always gets fresh attempts and stage1
/// never writes flash or OTP to count.
#[unsafe(link_section = ".uninit.BOOT_ATTEMPTS")]
static mut BOOT_ATTEMPTS: core::mem::MaybeUninit<[u32; 2]> = core::mem::MaybeUninit::uninit();

/// Every fatal stage1 error ends here: panics abort into HardFault. A transient
/// fault (brown-out, glitch) is retried; a persistent one halts in a low-power
/// wait instead of a battery-draining reset loop. Nothing runs past stage1 until
/// a later boot passes every check; the network core stays held off.
///
/// # Safety
///
/// Called only by the Cortex-M HardFault vector while executing exception context.
#[unsafe(no_mangle)]
#[cfg_attr(target_os = "none", unsafe(link_section = ".HardFault.user"))]
// SAFETY: Exception ABI requires this unmangled symbol; the attempt record is a
// fixed, aligned uninitialized RAM word pair only stage1 uses.
unsafe extern "C" fn HardFault() {
    let record = unsafe { &raw mut BOOT_ATTEMPTS as *mut [u32; 2] };
    let [magic, count] = unsafe { record.read_volatile() };
    let failures = if magic == ATTEMPTS_MAGIC && count < MAX_BOOT_ATTEMPTS {
        count + 1
    } else {
        1
    };
    if failures < MAX_BOOT_ATTEMPTS {
        unsafe { record.write_volatile([ATTEMPTS_MAGIC, failures]) };
        cortex_m::asm::dsb();
        cortex_m::peripheral::SCB::sys_reset();
    }
    // A manual reset after the halt starts a fresh set of attempts.
    unsafe { record.write_volatile([ATTEMPTS_MAGIC, 0]) };
    cortex_m::asm::dsb();
    loop {
        cortex_m::asm::wfe();
    }
}

/// # Safety
///
/// Called only by the Cortex-M exception vector with the active interrupt state.
#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    // SAFETY: SCB_ICSR is the architected, readable interrupt-control register.
    unsafe {
        const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
        let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;
        panic!("DefaultHandler #{:?}", irqn);
    }
}
