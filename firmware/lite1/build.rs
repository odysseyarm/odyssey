use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs};

use memory_spec::MemorySpec;

fn main() {
    let kdl_file = "memory-lite1.kdl";

    println!("cargo:info=Building lite1 app using {}", kdl_file);
    println!("cargo:rerun-if-changed=../memory/{}", kdl_file);
    println!("cargo:rerun-if-env-changed=APP_BANK");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let memory_kdl_path = manifest_dir.join("../memory").join(kdl_file);

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

    // APP_BANK picks which flash bank this image links for; bank_a when unset.
    let app_bank = env::var("APP_BANK").unwrap_or_else(|_| "bank_a".to_string());
    let app_flash = match app_bank.as_str() {
        "bank_a" => &regions["flash"]["app_bank_a"],
        "bank_b" => &regions["flash"]["app_bank_b"],
        _ => panic!(
            "Invalid APP_BANK value: {}. Must be 'bank_a' or 'bank_b'",
            app_bank
        ),
    };
    println!("cargo:info=Building for {}", app_bank);
    let app_payload_origin = app_flash.origin() + 512;
    let app_payload_length = app_flash.length() - 512;

    let stage2_state = &regions["flash"]["stage2_state"];
    let settings = &regions["flash"]["settings"];
    let low_ram = &regions["ram"]["low_ram"];
    let icmsg_rx = &regions["ram"]["high_ram"]["icmsg_rx"];
    let icmsg_tx = &regions["ram"]["high_ram"]["icmsg_tx"];

    let memory_x = format!(
        "\
MEMORY
{{
  STAGE2_STATE                      : ORIGIN = {:#010x}, LENGTH = {:#08x}
  FLASH                             : ORIGIN = {:#010x}, LENGTH = {:#08x}
  SETTINGS                          : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM                         (rwx) : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ICMSG_RX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
  ICMSG_TX                          : ORIGIN = {:#010x}, LENGTH = {:#06x}
}}

{symbols}

/* The vector table is four-byte aligned; compiled text can require eight. */
_stext = ALIGN(ADDR(.vector_table) + SIZEOF(.vector_table), 8);

ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) <= ORIGIN(SETTINGS), \"
ERROR: the FLASH and SETTINGS regions are overlapping\");

ASSERT(ORIGIN(RAM) + LENGTH(RAM) <= ORIGIN(ICMSG_RX), \"
ERROR: the RAM and ICMSG_RX regions are overlapping\");

ASSERT(ORIGIN(ICMSG_RX) + LENGTH(ICMSG_RX) <= ORIGIN(ICMSG_TX), \"
ERROR: the ICMSG_TX and ICMSG_RX regions are overlapping\");
",
        stage2_state.origin(),
        stage2_state.length(),
        app_payload_origin,
        app_payload_length,
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
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    let app_a = &regions["flash"]["app_bank_a"];
    let app_b = &regions["flash"]["app_bank_b"];
    let dfu = &regions["flash"]["dfu"];
    let netcore_primary = &regions["netcore_flash"]["netcore_primary"];
    let netcore_ram = &regions["netcore_ram"];
    for (name, address) in [
        ("__ota_app_ram_start", low_ram.origin()),
        ("__ota_app_ram_end", low_ram.origin() + low_ram.length()),
        ("__ota_netcore_ram_start", netcore_ram.origin()),
        (
            "__ota_netcore_ram_end",
            netcore_ram.origin() + netcore_ram.length(),
        ),
        ("__ota_app_a_slot_start", app_a.origin()),
        ("__ota_app_a_slot_end", app_a.origin() + app_a.length()),
        ("__ota_app_a_payload_start", app_a.origin() + 512),
        ("__ota_app_a_payload_end", app_a.origin() + app_a.length()),
        ("__ota_app_b_slot_start", app_b.origin()),
        ("__ota_app_b_slot_end", app_b.origin() + app_b.length()),
        ("__ota_app_b_payload_start", app_b.origin() + 512),
        ("__ota_app_b_payload_end", app_b.origin() + app_b.length()),
        ("__ota_netcore_slot_start", netcore_primary.origin()),
        (
            "__ota_netcore_slot_end",
            netcore_primary.origin() + netcore_primary.length() - 4096,
        ),
        (
            "__ota_netcore_payload_start",
            netcore_primary.origin() + 512,
        ),
        (
            "__ota_netcore_payload_end",
            netcore_primary.origin() + netcore_primary.length() - 4096,
        ),
        ("__ota_dfu_slot_start", dfu.origin()),
        ("__ota_dfu_slot_end", dfu.origin() + dfu.length()),
        ("__ota_dfu_payload_start", dfu.origin() + 512),
        ("__ota_dfu_payload_end", dfu.origin() + dfu.length()),
        ("__ota_netcore_stage_slot_start", dfu.origin()),
        (
            "__ota_netcore_stage_slot_end",
            dfu.origin() + netcore_primary.length() - 4096,
        ),
        ("__ota_netcore_stage_payload_start", dfu.origin() + 512),
        (
            "__ota_netcore_stage_payload_end",
            dfu.origin() + netcore_primary.length() - 4096,
        ),
    ] {
        println!("cargo:rustc-link-arg-bins=--defsym={name}={address:#x}");
    }
    println!("cargo:rustc-link-arg-bins=--print-memory-usage");
}
