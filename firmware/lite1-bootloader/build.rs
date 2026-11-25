use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs};

use memory_spec::MemorySpec;

fn main() {
    println!("cargo:rerun-if-changed=../memory/memory-lite1.kdl");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_kdl_path = manifest_dir.join("../memory/memory-lite1.kdl");

    let content = fs::read_to_string(&memory_kdl_path)
        .unwrap_or_else(|e| panic!("Failed to read memory-lite1.kdl: {}", e));

    let memoryspec = match MemorySpec::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to parse memory-atslite.kdl: {}", e);
            std::process::exit(1);
        }
    };

    let regions = memoryspec.regions();
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_journal_start={:#x}",
        regions["flash"]["boot_journal"].origin()
    );

    let bootloader = &regions["flash"]["stage2_bootloader"];
    let stage1_state = &regions["flash"]["stage1_state"];
    let stage2_state = &regions["flash"]["stage2_state"];
    let bank_a = &regions["flash"]["app_bank_a"];
    let bank_b = &regions["flash"]["app_bank_b"];
    let dfu = &regions["flash"]["dfu"];
    let settings = &regions["flash"]["settings"];
    // SPU flash regions are 16 KiB; stage2 opens only these to the network core.
    // Staging uses the network image size, not all of DFU, so the boot-state
    // pages after DFU stay secure. DFU's unaligned start also exposes trust page B,
    // which only the update-time network bootloader can then reach.
    let netcore_primary = &regions["netcore_flash"]["netcore_primary"];
    let staging_end = dfu.origin() + netcore_primary.length() - 0x1000;
    assert!(staging_end <= dfu.origin() + dfu.length());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(
        out.join("dfu_spu.rs"),
        format!(
            "const DFU_SPU_REGIONS: core::ops::Range<usize> = {}..{};\n",
            dfu.origin() / 0x4000,
            staging_end.div_ceil(0x4000)
        ),
    )
    .unwrap();
    let low_ram = &regions["ram"]["low_ram"];
    let locked_ram = &regions["ram"]["high_ram"]["locked_ram"];
    let icmsg_rx = &regions["ram"]["high_ram"]["icmsg_rx"];
    let icmsg_tx = &regions["ram"]["high_ram"]["icmsg_tx"];

    // Dual-bank: ACTIVE is bank A and DFU is bank B. embassy-boot asserts DFU is
    // at least one page larger than ACTIVE, so DFU gets an extra page it never uses.
    let active_len = bank_a.length();
    let dfu_len = bank_b.length() + 0x1000;

    let memory_x = format!(
        "\
MEMORY
{{
  FLASH                             : ORIGIN = {:#010x}, LENGTH = {:#08x}
  STAGE2_STATE                      : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ACTIVE                            : ORIGIN = {:#010x}, LENGTH = {:#08x}
  DFU                               : ORIGIN = {:#010x}, LENGTH = {:#08x}
  SETTINGS                          : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM                         (rwx) : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ICMSG_RX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
  ICMSG_TX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
}}

__bootloader_state_start = ORIGIN(STAGE2_STATE);
__bootloader_state_end = ORIGIN(STAGE2_STATE) + LENGTH(STAGE2_STATE);

__stage1_state_start = {:#010x};
__stage1_state_end = {:#010x};

__bootloader_active_start = ORIGIN(ACTIVE);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE);

__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);

__bootloader_app_a_start = {:#010x};
__bootloader_app_a_end = {:#010x};
__bootloader_app_b_start = {:#010x};
__bootloader_app_b_end = {:#010x};
__bootloader_dfu_region_start = {:#010x};
__bootloader_dfu_region_end = {:#010x};
__stage2_bootloader_start = {:#010x};
__stage2_bootloader_end = {:#010x};
__netcore_primary_start = {netcore_start:#010x};
__netcore_primary_end = {netcore_end:#010x};
__trust_a_start = {trust_a_start:#010x};
__trust_a_end = {trust_a_end:#010x};

__locked_ram_start = {:#010x};
__locked_ram_end = {:#010x};
__policy_handoff_start = {:#010x};
__icmsg_tx_start = ORIGIN(ICMSG_TX);
__icmsg_tx_end = ORIGIN(ICMSG_TX) + LENGTH(ICMSG_TX);
__icmsg_rx_start = ORIGIN(ICMSG_RX);
__icmsg_rx_end = ORIGIN(ICMSG_RX) + LENGTH(ICMSG_RX);

ASSERT(ORIGIN(ACTIVE) + LENGTH(ACTIVE) <= ORIGIN(DFU), \"
ERROR(bootloader): the ACTIVE and DFU regions are overlapping\");
",
        bootloader.origin() + 512,
        bootloader.length() - 512,
        stage2_state.origin(),
        stage2_state.length(),
        bank_a.origin(),
        active_len,
        bank_b.origin(),
        dfu_len,
        settings.origin(),
        settings.length(),
        low_ram.origin(),
        low_ram.length(),
        icmsg_rx.origin(),
        icmsg_rx.length(),
        icmsg_tx.origin(),
        icmsg_tx.length(),
        stage1_state.origin(),
        stage1_state.origin() + stage1_state.length(),
        bank_a.origin(),
        bank_a.origin() + bank_a.length(),
        bank_b.origin(),
        bank_b.origin() + bank_b.length(),
        dfu.origin(),
        dfu.origin() + dfu.length(),
        bootloader.origin(),
        bootloader.origin() + bootloader.length(),
        locked_ram.origin(),
        locked_ram.origin() + locked_ram.length(),
        regions["ram"]["shared_verifier"]["policy_handoff"].origin(),
        netcore_start = regions["netcore_flash"]["netcore_primary"].origin(),
        netcore_end = regions["netcore_flash"]["netcore_primary"].origin()
            + regions["netcore_flash"]["netcore_primary"].length(),
        trust_a_start = regions["flash"]["trust_a"].origin(),
        trust_a_end = regions["flash"]["trust_a"].origin() + regions["flash"]["trust_a"].length(),
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("memory.x"), memory_x)
        .unwrap_or_else(|e| panic!("Failed to write memory.x: {}", e));

    // Fixed-address record at the end of the stage2 region (see bootloader-info). It goes
    // after the last FLASH-loaded section of cortex-m-rt's link.x: lld wants load addresses
    // to rise in script order, and an earlier slot would also move the packing of .text.
    let info_x = format!(
        "SECTIONS {{\n  .bootloader_info {:#010x} : {{ KEEP(*(.bootloader_info)) }} > FLASH\n}}\nINSERT AFTER .gnu.sgstubs;\n",
        bootloader_info::address(u32::try_from(bootloader.end()).expect("flash addresses fit u32"))
    );
    fs::write(out_dir.join("bootloader_info.x"), info_x)
        .unwrap_or_else(|e| panic!("Failed to write bootloader_info.x: {}", e));

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tbootloader_info.x");
    if env::var("CARGO_FEATURE_DEFMT").is_ok() {
        println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    }
}
