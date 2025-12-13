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
    let bank_a = &regions["flash"]["app_bank_a"];
    let bank_b = &regions["flash"]["app_bank_b"];
    let low_ram = &regions["ram"]["low_ram"];

    let memory_x = format!(
        "\
MEMORY
{{
  FLASH                             : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM                         (rwx) : ORIGIN = {:#010x}, LENGTH = {:#08x}
}}

__stage2_bootloader_start = {:#010x};
__stage2_bootloader_end = {:#010x};
__bootloader_state_start = {:#010x};
__bootloader_state_end = {:#010x};
__app_bank_a_start = {:#010x};
__app_bank_a_end = {:#010x};
__app_bank_b_start = {:#010x};
__app_bank_b_end = {:#010x};
",
        stage1_bootloader.origin(),
        stage1_bootloader.length(),
        low_ram.origin(),
        low_ram.length(),
        stage2_bootloader.origin(),
        stage2_bootloader.origin() + stage2_bootloader.length(),
        bootloader_state.origin(),
        bootloader_state.origin() + bootloader_state.length(),
        bank_a.origin(),
        bank_a.origin() + bank_a.length(),
        bank_b.origin(),
        bank_b.origin() + bank_b.length(),
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
