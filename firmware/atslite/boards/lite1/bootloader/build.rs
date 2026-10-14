use memory_spec::MemorySpec;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

fn main() {
    println!("cargo:rerun-if-changed=../../../memory/memory-lite1.kdl");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_kdl_path = manifest_dir.join("../../../memory/memory-lite1.kdl");

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

    // Now using stage2_bootloader instead of bootloader
    let bootloader = &regions["flash"]["stage2_bootloader"];
    let stage1_state = &regions["flash"]["stage1_state"];
    let stage2_state = &regions["flash"]["stage2_state"];
    let bank_a = &regions["flash"]["app_bank_a"];
    let bank_b = &regions["flash"]["app_bank_b"];
    let dfu = &regions["flash"]["dfu"];
    let settings = &regions["flash"]["settings"];
    let low_ram = &regions["ram"]["low_ram"];
    let locked_ram = &regions["ram"]["high_ram"]["locked_ram"];
    let icmsg_rx = &regions["ram"]["high_ram"]["icmsg_rx"];
    let icmsg_tx = &regions["ram"]["high_ram"]["icmsg_tx"];

    // For dual-bank, ACTIVE and DFU point to bank_a and bank_b
    // We make DFU slightly larger to satisfy embassy-boot's assertion (even though unused)
    let active_len = bank_a.length();
    let dfu_len = bank_b.length() + 0x1000; // Add one page to satisfy assertion

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

__locked_ram_start = {:#010x};
__locked_ram_end = {:#010x};
__icmsg_tx_start = ORIGIN(ICMSG_TX);
__icmsg_tx_end = ORIGIN(ICMSG_TX) + LENGTH(ICMSG_TX);
__icmsg_rx_start = ORIGIN(ICMSG_RX);
__icmsg_rx_end = ORIGIN(ICMSG_RX) + LENGTH(ICMSG_RX);

ASSERT(ORIGIN(ACTIVE) + LENGTH(ACTIVE) <= ORIGIN(DFU), \"
ERROR(bootloader): the ACTIVE and DFU regions are overlapping\");
",
        bootloader.origin(),
        bootloader.length(),
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
        locked_ram.origin(),
        locked_ram.origin() + locked_ram.length()
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("memory.x"), memory_x)
        .unwrap_or_else(|e| panic!("Failed to write memory.x: {}", e));

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    if env::var("CARGO_FEATURE_DEFMT").is_ok() {
        println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    }
}
