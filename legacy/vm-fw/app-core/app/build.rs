use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs};

use memory_spec::MemorySpec;

fn main() {
    let context = env::var("CARGO_CFG_CONTEXT").unwrap();
    let is_atslite = context == "atslite1";

    let kdl_file = if is_atslite {
        "memory-atslite.kdl"
    } else {
        "memory-vm.kdl"
    };

    println!("cargo:info=Building with context='{}', using {}", context, kdl_file);

    println!("cargo:rerun-if-changed=../../memory/{}", kdl_file);

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_kdl_path = manifest_dir.join("../../memory").join(kdl_file);

    let content = fs::read_to_string(&memory_kdl_path).unwrap_or_else(|e| panic!("Failed to read {}: {}", kdl_file, e));

    let memoryspec = match MemorySpec::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to parse {}: {}", kdl_file, e);
            std::process::exit(1);
        }
    };

    let regions = memoryspec.regions();
    let symbols = memoryspec.render_symbols();

    let memory_x = if is_atslite {
        let app_flash = &regions["flash"]["app_flash"];
        let settings = &regions["flash"]["settings"];
        let low_ram = &regions["ram"]["low_ram"];
        let icmsg_rx = &regions["ram"]["high_ram"]["icmsg_rx"];
        let icmsg_tx = &regions["ram"]["high_ram"]["icmsg_tx"];

        format!(
            "\
MEMORY
{{
  FLASH                             : ORIGIN = {:#010x}, LENGTH = {:#08x}
  SETTINGS                          : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM                         (rwx) : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ICMSG_RX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
  ICMSG_TX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
}}

{symbols}

ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) <= ORIGIN(SETTINGS), \"
ERROR: the FLASH and SETTINGS regions are overlapping\");

ASSERT(ORIGIN(RAM) + LENGTH(RAM) <= ORIGIN(ICMSG_RX), \"
ERROR: the RAM and ICMSG_RX regions are overlapping\");

ASSERT(ORIGIN(ICMSG_RX) + LENGTH(ICMSG_RX) <= ORIGIN(ICMSG_TX), \"
ERROR: the ICMSG_TX and ICMSG_RX regions are overlapping\");
",
            app_flash.origin(),
            app_flash.length(),
            settings.origin(),
            settings.length(),
            low_ram.origin(),
            low_ram.length(),
            icmsg_rx.origin(),
            icmsg_rx.length(),
            icmsg_tx.origin(),
            icmsg_tx.length()
        )
    } else {
        let app_flash = &regions["flash"]["app_flash"];
        let settings = &regions["flash"]["settings"];
        let ram = &regions["ram"];

        format!(
            "\
MEMORY
{{
  FLASH                             : ORIGIN = {:#010x}, LENGTH = {:#08x}
  SETTINGS                          : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM                         (rwx) : ORIGIN = {:#010x}, LENGTH = {:#08x}
}}

{symbols}

ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) <= ORIGIN(SETTINGS), \"
ERROR: the FLASH and SETTINGS regions are overlapping\");
",
            app_flash.origin(),
            app_flash.length(),
            settings.origin(),
            settings.length(),
            ram.origin(),
            ram.length()
        )
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("memory.x"), memory_x).unwrap_or_else(|e| panic!("Failed to write memory.x: {}", e));

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    println!("cargo:rustc-link-arg-bins=--print-memory-usage");
}
