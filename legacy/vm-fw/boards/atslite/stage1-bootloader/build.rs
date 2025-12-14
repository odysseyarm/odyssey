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

    let stage1_bootloader = &regions["flash"]["stage1_bootloader"];
    let stage2_bootloader = &regions["flash"]["stage2_bootloader"];
    let bootloader_state = &regions["flash"]["bootloader_state"];
    let dfu = &regions["flash"]["dfu"];
    let low_ram = &regions["ram"]["low_ram"];

    // embassy-boot expects:
    // - ACTIVE: primary stage2 bootloader slot
    // - DFU: secondary slot for bootloader updates
    // - STATE: swap state partition
    let memory_x = format!(
        "\
MEMORY
{{
  FLASH  : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ACTIVE : ORIGIN = {:#010x}, LENGTH = {:#08x}
  DFU    : ORIGIN = {:#010x}, LENGTH = {:#08x}
  STATE  : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM    : ORIGIN = {:#010x}, LENGTH = {:#08x}
}}

__bootloader_state_start = ORIGIN(STATE);
__bootloader_state_end = ORIGIN(STATE) + LENGTH(STATE);
__bootloader_active_start = ORIGIN(ACTIVE);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE);
__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);
",
        stage1_bootloader.origin(),
        stage1_bootloader.length(),
        stage2_bootloader.origin(),
        stage2_bootloader.length(),
        dfu.origin(),
        dfu.length(),
        bootloader_state.origin(),
        bootloader_state.length(),
        low_ram.origin(),
        low_ram.length(),
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
