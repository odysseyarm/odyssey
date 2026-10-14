use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs};

use memory_spec::MemorySpec;

fn main() {
    let kdl_file = "memory-atslite.kdl";

    println!("cargo:info=Building net-core using {}", kdl_file);
    println!("cargo:rerun-if-changed=../../../memory/{}", kdl_file);

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_kdl_path = manifest_dir.join("../../../memory").join(kdl_file);

    let content = fs::read_to_string(&memory_kdl_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", kdl_file, e));

    let memoryspec = match MemorySpec::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to parse {}: {}", kdl_file, e);
            std::process::exit(1);
        }
    };

    let regions = memoryspec.regions();

    let netcore_primary = &regions["netcore_flash"]["netcore_primary"];
    let dfu_region = &regions["flash"]["dfu"];
    let netcore_ram = &regions["netcore_ram"];

    // For net-core, swap TX and RX from the app core's perspective
    // App's TX becomes net's RX, and app's RX becomes net's TX
    let icmsg_rx = &regions["ram"]["high_ram"]["icmsg_tx"]; // App TX -> Net RX
    let icmsg_tx = &regions["ram"]["high_ram"]["icmsg_rx"]; // App RX -> Net TX

    // NVMC pages are 4096 bytes.
    let page_size: u64 = 4096;
    let active_len = netcore_primary.length().saturating_sub(page_size);
    let dfu_len = dfu_region.length();
    if dfu_len < active_len + page_size {
        eprintln!("DFU slot must be at least one page larger than active");
        std::process::exit(1);
    }
    let state_origin = netcore_primary.origin() + active_len;
    let state_len = page_size;

    let memory_x = format!(
        "\
MEMORY
{{
  /* Active is trimmed so DFU is one page larger */
  FLASH : ORIGIN = {:#010x}, LENGTH = {:#06x}
  DFU : ORIGIN = {:#010x}, LENGTH = {:#06x}
  STATE : ORIGIN = {:#010x}, LENGTH = {:#06x}
  ICMSG_TX : ORIGIN = {:#010x}, LENGTH = {:#06x}
  ICMSG_RX : ORIGIN = {:#010x}, LENGTH = {:#06x}
  RAM : ORIGIN = {:#010x}, LENGTH = {}K
}}

__icmsg_tx_start = ORIGIN(ICMSG_TX);
__icmsg_tx_end = ORIGIN(ICMSG_TX) + LENGTH(ICMSG_TX);
__icmsg_rx_start = ORIGIN(ICMSG_RX);
__icmsg_rx_end = ORIGIN(ICMSG_RX) + LENGTH(ICMSG_RX);

__bootloader_active_start = ORIGIN(FLASH);
__bootloader_active_end = ORIGIN(FLASH) + LENGTH(FLASH);
__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);
__bootloader_state_start = ORIGIN(STATE);
__bootloader_state_end = ORIGIN(STATE) + LENGTH(STATE);

ASSERT(ORIGIN(ICMSG_TX) + LENGTH(ICMSG_TX) <= ORIGIN(RAM), \"
ERROR: the RAM and ICMSG_TX regions are overlapping\");
ASSERT(ORIGIN(ICMSG_TX) + LENGTH(ICMSG_TX) <= ORIGIN(ICMSG_RX), \"
ERROR: the ICMSG_TX and ICMSG_RX regions are overlapping\");
",
        netcore_primary.origin(),
        active_len,
        dfu_region.origin(),
        dfu_region.length(),
        state_origin,
        state_len,
        icmsg_tx.origin(),
        icmsg_tx.length(),
        icmsg_rx.origin(),
        icmsg_rx.length(),
        netcore_ram.origin(),
        netcore_ram.length() / 1024,
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("memory.x"), memory_x)
        .unwrap_or_else(|e| panic!("Failed to write memory.x: {}", e));

    // Provide defsyms so embassy-boot can always find the partition symbols.
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_active_start={:#x}",
        netcore_primary.origin()
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_active_end={:#x}",
        netcore_primary.origin() + active_len
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_dfu_start={:#x}",
        dfu_region.origin()
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_dfu_end={:#x}",
        dfu_region.origin() + dfu_region.length()
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_state_start={:#x}",
        state_origin
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_state_end={:#x}",
        state_origin + state_len
    );

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=--print-memory-usage");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
}
