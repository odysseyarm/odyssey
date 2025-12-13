use memory_spec::MemorySpec;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

fn main() {
    let kdl_file = "memory-atslite.kdl";

    println!("cargo:info=Building netcore-bootloader using {}", kdl_file);
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

    let netcore_bootloader = &regions["netcore_flash"]["netcore_bootloader"];
    let netcore_primary = &regions["netcore_flash"]["netcore_primary"];
    let netcore_secondary = &regions["flash"]["netcore_secondary"];
    let netcore_ram = &regions["netcore_ram"];

    // For netcore bootloader, swap TX and RX from the app core's perspective
    // App's RX becomes netcore's TX (for DFU metadata access)
    let icmsg_tx = &regions["ram"]["high_ram"]["icmsg_rx"]; // App RX -> Net TX
    let pcd_region = icmsg_tx; // PCD lives in app RX from netcore perspective

    // DFU slot (in app flash) must be >= active. Reserve one page for state at end of primary.
    let page_size: u64 = 4096;
    let active_len = netcore_primary.length().saturating_sub(page_size);
    let state_origin = netcore_primary.origin() + active_len;
    let state_len = page_size;

    let memory_x = format!(
        "\
MEMORY
{{
  FLASH : ORIGIN = {:#010x}, LENGTH = {}K
  ACTIVE : ORIGIN = {:#010x}, LENGTH = {:#06x}
  DFU : ORIGIN = {:#010x}, LENGTH = {:#06x}
  STATE : ORIGIN = {:#010x}, LENGTH = {:#06x}
  RAM : ORIGIN = {:#010x}, LENGTH = {}K
}}

__bootloader_active_start = {:#010x};
__bootloader_active_end = {:#010x};
__bootloader_dfu_start = {:#010x};
__bootloader_dfu_end = {:#010x};
__bootloader_state_start = {:#010x};
__bootloader_state_end = {:#010x};

/* DFU metadata location (in app RX = netcore TX) */
PROVIDE(__dfu_metadata_start = {:#010x});
PROVIDE(__pcd_region_start = {:#010x});
PROVIDE(__pcd_region_end = {:#010x});
",
        netcore_bootloader.origin(),
        netcore_bootloader.length() / 1024,
        netcore_primary.origin(),
        active_len,
        netcore_secondary.origin(),
        netcore_secondary.length(),
        state_origin,
        state_len,
        netcore_ram.origin(),
        netcore_ram.length() / 1024,
        netcore_primary.origin(),
        netcore_primary.origin() + active_len,
        netcore_secondary.origin(),
        netcore_secondary.origin() + netcore_secondary.length(),
        state_origin,
        state_origin + state_len,
        icmsg_tx.origin(),
        pcd_region.origin(),
        pcd_region.origin() + pcd_region.length(),
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("memory.x"), memory_x)
        .unwrap_or_else(|e| panic!("Failed to write memory.x: {}", e));

    // Also emit --defsym so symbols are always available even if linker ignores memory.x include search paths.
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
        netcore_secondary.origin()
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_dfu_end={:#x}",
        netcore_secondary.origin() + netcore_secondary.length()
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_state_start={:#x}",
        state_origin
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__bootloader_state_end={:#x}",
        state_origin + state_len
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__pcd_region_start={:#x}",
        pcd_region.origin()
    );
    println!(
        "cargo:rustc-link-arg-bins=--defsym=__pcd_region_end={:#x}",
        pcd_region.origin() + pcd_region.length()
    );

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    if env::var("CARGO_FEATURE_DEFMT").is_ok() {
        println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    }
}
