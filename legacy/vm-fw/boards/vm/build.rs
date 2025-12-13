use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs};

use memory_spec::MemorySpec;

fn main() {
    let kdl_file = "memory-vm.kdl";

    println!("cargo:info=Building VM app using {}", kdl_file);
    println!("cargo:rerun-if-changed=../../memory/{}", kdl_file);

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_kdl_path = manifest_dir.join("../../memory").join(kdl_file);

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
    let symbols = memoryspec.render_symbols();

    let app_flash = &regions["flash"]["app_flash"];
    let settings = &regions["flash"]["settings"];
    let ram = &regions["ram"];

    let memory_x = format!(
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
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("memory.x"), memory_x)
        .unwrap_or_else(|e| panic!("Failed to write memory.x: {}", e));

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    println!("cargo:rustc-link-arg-bins=--print-memory-usage");
}
