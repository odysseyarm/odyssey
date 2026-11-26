//! nRF5340 stage1 policy storage: the accepted trust policy sits in two flash pages
//! that stage1 alone writes, then write-protects before it hands off.
#![expect(unsafe_code, reason = "nRF5340 memory-mapped flash and NVMC")]

use release_signing::{Component, KEY_SLOTS, PublicKey, RootSet, TrustList};

use crate::state::{self, STATE_LEN};
use crate::{Error, verify_slot};

const NVMC_READY: usize = 0x5003_9400;
const NVMC_CONFIG: usize = 0x5003_9504;
const NVMC_WRITE_ENABLE: u32 = 1;
const NVMC_ERASE_ENABLE: u32 = 2;
const BOOT_MAGIC: u32 = 0xd0d0_d0d0;
const PAGE_LEN: usize = 4096;

unsafe extern "C" {
    static __trust_a_start: u32;
    static __trust_a_end: u32;
    static __trust_state_a_start: u32;
    static __trust_state_b_start: u32;
    static __stage1_state_start: u32;
    static __stage2_bootloader_start: u32;
    static __stage2_bootloader_end: u32;
    static __bootloader_app_a_start: u32;
    static __bootloader_app_a_end: u32;
    static __bootloader_app_b_start: u32;
    static __bootloader_app_b_end: u32;
    static __stage1_recovery_anchors_start: [PublicKey; KEY_SLOTS];
    static __stage1_recovery_threshold: u32;
}

fn recovery_roots() -> Result<RootSet, Error> {
    // SAFETY: stage1 retains the fixed anchor table and threshold in protected flash.
    let anchors = unsafe { core::ptr::read_volatile(&raw const __stage1_recovery_anchors_start) };
    let threshold = u8::try_from(read_word(&raw const __stage1_recovery_threshold as usize))
        .map_err(|_| Error::Storage)?;
    RootSet::new(threshold, anchors).map_err(Error::Policy)
}

fn read_word(address: usize) -> u32 {
    // SAFETY: callers provide aligned, fixed nRF5340 register or flash addresses.
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

fn write_word(address: usize, value: u32) {
    // SAFETY: caller serializes NVMC access before handing off from stage1.
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}

fn wait_ready() {
    while read_word(NVMC_READY) & 1 == 0 {
        core::hint::spin_loop();
    }
}

fn flash(start: usize, end: usize) -> &'static [u8] {
    // SAFETY: linker bounds describe mapped flash, which stage1 alone accesses during policy commit.
    unsafe { core::slice::from_raw_parts(start as *const u8, end - start) }
}

fn state_page_address(page: usize) -> usize {
    match page {
        0 => &raw const __trust_state_a_start as usize,
        _ => &raw const __trust_state_b_start as usize,
    }
}

fn state_pages() -> [&'static [u8]; 2] {
    [0, 1].map(|page| {
        let start = state_page_address(page);
        flash(start, start + PAGE_LEN)
    })
}

/// Programs one state page: erase, write, then read back. A power cut anywhere in
/// here leaves a record that fails its hash, so the other page stays in force.
fn program_state(page: usize, record: &[u8; STATE_LEN]) -> Result<(), Error> {
    let destination = state_page_address(page);
    wait_ready();
    write_word(NVMC_CONFIG, NVMC_ERASE_ENABLE);
    wait_ready();
    write_word(destination, u32::MAX);
    wait_ready();
    write_word(NVMC_CONFIG, NVMC_WRITE_ENABLE);
    wait_ready();
    for (index, chunk) in record.chunks_exact(4).enumerate() {
        let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        write_word(destination + index * 4, word);
        wait_ready();
    }
    write_word(NVMC_CONFIG, 0);
    wait_ready();
    if flash(destination, destination + STATE_LEN) != record {
        return Err(Error::Storage);
    }
    Ok(())
}

/// The unprotected page where the next signed trust record is staged.
pub fn staging_address() -> Result<u32, Error> {
    u32::try_from(&raw const __trust_a_start as usize).map_err(|_| Error::Storage)
}

/// The 16 KiB SPU flash region holding both state pages; the memory map keeps nothing else in it.
pub fn state_spu_region() -> usize {
    (&raw const __trust_state_a_start as usize) >> 14
}

fn usable_with(policy: &TrustList) -> Result<(), Error> {
    if read_word(&raw const __stage1_state_start as usize) != BOOT_MAGIC {
        return Err(Error::Recovery);
    }
    let stage2 = &raw const __stage2_bootloader_start as usize;
    verify_slot(
        flash(stage2, &raw const __stage2_bootloader_end as usize),
        stage2 as u32,
        Component::Lite1SecureBootloader,
        policy.keys(),
    )?;
    let app_a = &raw const __bootloader_app_a_start as usize;
    let app_b = &raw const __bootloader_app_b_start as usize;
    let a = verify_slot(
        flash(app_a, &raw const __bootloader_app_a_end as usize),
        app_a as u32,
        Component::Lite1SecureApp,
        policy.keys(),
    );
    if a.is_err() {
        verify_slot(
            flash(app_b, &raw const __bootloader_app_b_end as usize),
            app_b as u32,
            Component::Lite1SecureApp,
            policy.keys(),
        )?;
    }
    // The app core cannot read network flash. The network bootloader locks the
    // network slot instead, and only installs images stage2 authenticated.
    Ok(())
}

/// Decides the policy for this boot and persists a change before using it.
///
/// A staged record replaces the stored policy only when it is the next generation,
/// signed by a threshold of the stored roots, and the installed images verify under
/// its keys.
pub fn boot_policy() -> Result<TrustList, Error> {
    let anchors = recovery_roots()?;
    let (policy, commit) = state::plan(
        state_pages(),
        flash(
            &raw const __trust_a_start as usize,
            &raw const __trust_a_end as usize,
        ),
        &anchors,
        usable_with,
    )?;
    if let Some(commit) = commit {
        program_state(commit.page, &state::encode(commit.sequence, &commit.policy))?;
    }
    Ok(policy)
}
