use memory_spec::MemorySpec;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

fn main() {
    println!("cargo:rerun-if-changed=../../../memory/memory-atslite.kdl");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_kdl_path = manifest_dir.join("../../../memory/memory-atslite.kdl");

    let content = fs::read_to_string(&memory_kdl_path)
        .unwrap_or_else(|e| panic!("Failed to read memory-atslite.kdl: {}", e));

    let memoryspec = match MemorySpec::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to parse memory-atslite.kdl: {}", e);
            std::process::exit(1);
        }
    };

    let regions = memoryspec.regions();

    let bootloader = &regions["flash"]["bootloader"];
    let bootloader_state = &regions["flash"]["bootloader_state"];
    let active = &regions["flash"]["app_flash"];
    let dfu = &regions["flash"]["dfu"];
    let settings = &regions["flash"]["settings"];
    let low_ram = &regions["ram"]["low_ram"];
    let icmsg_rx = &regions["ram"]["high_ram"]["icmsg_rx"];
    let icmsg_tx = &regions["ram"]["high_ram"]["icmsg_tx"];

    let memory_x = format!(
        "\
MEMORY
{{
  FLASH                             : ORIGIN = {:#010x}, LENGTH = {:#08x}
  BOOTLOADER_STATE                  : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ACTIVE                            : ORIGIN = {:#010x}, LENGTH = {:#08x}
  DFU                               : ORIGIN = {:#010x}, LENGTH = {:#08x}
  SETTINGS                          : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM                         (rwx) : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ICMSG_RX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
  ICMSG_TX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
}}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);

__bootloader_active_start = ORIGIN(ACTIVE);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE);

__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);

ASSERT(ORIGIN(ACTIVE) + LENGTH(ACTIVE) <= ORIGIN(DFU), \"
ERROR(bootloader): the ACTIVE and DFU regions are overlapping\");
",
        bootloader.origin(),
        bootloader.length(),
        bootloader_state.origin(),
        bootloader_state.length(),
        active.origin(),
        active.length(),
        dfu.origin(),
        dfu.length(),
        settings.origin(),
        settings.length(),
        low_ram.origin(),
        low_ram.length(),
        icmsg_rx.origin(),
        icmsg_rx.length(),
        icmsg_tx.origin(),
        icmsg_tx.length()
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
