use std::fmt::Write as _;
use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs};

use memory_spec::MemorySpec;
use sha2::{Digest, Sha256};

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

    let stage1_bootloader = &regions["flash"]["stage1_bootloader"];
    let stage2_bootloader = &regions["flash"]["stage2_bootloader"];
    let stage1_state = &regions["flash"]["stage1_state"];
    let dfu = &regions["flash"]["dfu"];
    let shared_verifier = &regions["ram"]["shared_verifier"];
    let low_ram = &regions["ram"]["low_ram"];
    let netcore_bootloader = &regions["netcore_flash"]["netcore_bootloader"];

    // embassy-boot's partitions: ACTIVE is the stage2 bootloader, DFU holds a
    // stage2 update, STATE tracks the swap.
    const SHARED_VERIFIER_SIZE: u64 = 0x2000;
    const SHARED_VERIFIER_START: u64 = 0x2000_0000;
    if stage1_bootloader.origin() != 0 || stage1_bootloader.length() != 0x8000 {
        panic!("stage1 flash must span 0x0000..0x8000");
    }
    if shared_verifier.origin() != SHARED_VERIFIER_START
        || shared_verifier.length() != SHARED_VERIFIER_SIZE
        || low_ram.origin() != SHARED_VERIFIER_START + SHARED_VERIFIER_SIZE
    {
        panic!("memory map must reserve verifier RAM before normal RAM");
    }
    if netcore_bootloader.origin() != 0x0100_0000 || netcore_bootloader.length() != 60 * 1024 {
        panic!("network bootloader region must be 60 KiB at 0x01000000");
    }
    // Stage1 write-protects the whole 16 KiB flash unit holding the trust state pages
    // before it hands off, so nothing that a later image writes may share that unit.
    let state_a = &regions["flash"]["trust_state_a"];
    let state_b = &regions["flash"]["trust_state_b"];
    let unit = state_a.origin() & !0x3fff;
    if state_a.origin() != unit || state_b.end() > unit + 0x4000 {
        panic!("trust state pages must sit in one 16 KiB flash unit");
    }
    for name in [
        "stage1_state",
        "stage2_state",
        "boot_journal",
        "settings",
        "dfu",
        "trust_a",
        "app_bank_a",
        "app_bank_b",
    ] {
        let other = &regions["flash"][name];
        if other.origin() < unit + 0x4000 && other.end() > unit {
            panic!("{name} shares the write-protected trust state flash unit");
        }
    }
    let linker_symbols = memoryspec
        .render_symbols()
        .lines()
        .filter(|line| !line.starts_with("__bootloader_state_"))
        .collect::<Vec<_>>()
        .join("\n");
    let memory_x = format!(
        "\
MEMORY
{{
  FLASH              : ORIGIN = {:#010x}, LENGTH = {:#08x}
  ACTIVE             : ORIGIN = {:#010x}, LENGTH = {:#08x}
  DFU                : ORIGIN = {:#010x}, LENGTH = {:#08x}
  STATE              : ORIGIN = {:#010x}, LENGTH = {:#08x}
  NETCORE_BOOTLOADER : ORIGIN = {:#010x}, LENGTH = {:#08x}
  RAM                : ORIGIN = {:#010x}, LENGTH = {:#08x}
}}
_stext = ALIGN(ORIGIN(FLASH) + SIZEOF(.vector_table), 8);
__bootloader_active_start = ORIGIN(ACTIVE);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE);
__bootloader_state_start = ORIGIN(STATE);
__bootloader_state_end = ORIGIN(STATE) + LENGTH(STATE);
{}
",
        stage1_bootloader.origin(),
        stage1_bootloader.length(),
        stage2_bootloader.origin(),
        stage2_bootloader.length(),
        dfu.origin(),
        dfu.length(),
        stage1_state.origin(),
        stage1_state.length(),
        netcore_bootloader.origin(),
        netcore_bootloader.length(),
        shared_verifier.origin(),
        shared_verifier.length() + low_ram.length(),
        linker_symbols,
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("memory.x"), memory_x)
        .unwrap_or_else(|e| panic!("Failed to write memory.x: {}", e));
    const NETCORE_BOOTLOADER_SIZE: usize = 60 * 1024;
    println!("cargo:rerun-if-env-changed=ODYSSEY_NETCORE_BOOTLOADER");
    let netcore_path = env::var_os("ODYSSEY_NETCORE_BOOTLOADER")
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("ODYSSEY_NETCORE_BOOTLOADER must name raw netboot .bin"));
    println!("cargo:rerun-if-changed={}", netcore_path.display());
    let raw_netcore = fs::read(&netcore_path)
        .unwrap_or_else(|error| panic!("Failed to read {}: {error}", netcore_path.display()));
    if raw_netcore.is_empty() || raw_netcore.len() > NETCORE_BOOTLOADER_SIZE {
        panic!("network bootloader must be 1..=61440 bytes");
    }
    let mut padded_netcore = vec![0xff; NETCORE_BOOTLOADER_SIZE];
    padded_netcore[..raw_netcore.len()].copy_from_slice(&raw_netcore);
    let digest = Sha256::digest(&padded_netcore);
    let mut hash_source = String::from("pub(super) const NETCORE_BOOTLOADER_SHA256: [u8; 32] = [");
    let mut digest_hex = String::with_capacity(64);
    for byte in digest {
        write!(hash_source, "0x{byte:02x},").unwrap();
        write!(digest_hex, "{byte:02x}").unwrap();
    }
    hash_source.push_str("];\n");
    fs::write(out_dir.join("netcore_bootloader_hash.rs"), hash_source)
        .unwrap_or_else(|error| panic!("Failed to write netcore hash: {error}"));
    println!("cargo:rerun-if-env-changed=ODYSSEY_NETCORE_BOOTLOADER_HASH_OUT");
    let hash_output = env::var_os("ODYSSEY_NETCORE_BOOTLOADER_HASH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| out_dir.join("stage1-netcore-bootloader.sha256"));
    if let Some(parent) = hash_output.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("Failed to create hash output directory: {error}"));
    }
    fs::write(hash_output, digest_hex)
        .unwrap_or_else(|error| panic!("Failed to write netcore hash sidecar: {error}"));
    let shared_sections = "\
SECTIONS
{
  .recovery_threshold __stage1_recovery_threshold :
  {
    KEEP(*(.recovery_threshold))
  } > FLASH
  ASSERT(SIZEOF(.recovery_threshold) == 4, \"recovery threshold must be one word\")

  .recovery_anchors __stage1_recovery_anchors_start :
  {
    KEEP(*(.recovery_anchors))
  } > FLASH
  ASSERT(SIZEOF(.recovery_anchors) == 128, \"recovery anchor table must contain four public keys\")
  ASSERT(ADDR(.recovery_anchors) + SIZEOF(.recovery_anchors) <= 0x7FD0, \"recovery anchors overlap netboot hash\")

  .netcore_bootloader_hash 0x7FD0 :
  {
    KEEP(*(.netcore_bootloader_hash))
  } > FLASH
  ASSERT(ADDR(.netcore_bootloader_hash) == 0x7FD0, \"netcore hash address changed\")
  ASSERT(SIZEOF(.netcore_bootloader_hash) == 32, \"netcore hash must be 32 bytes\")

  .shared_verify_entry 0x7FF0 :
  {
    KEEP(*(.shared_verify_entry))
  } > FLASH
  ASSERT(ADDR(.shared_verify_entry) == 0x7FF0, \"shared verifier entry address changed\")
  ASSERT(SIZEOF(.shared_verify_entry) == 16, \"shared verifier entry must be 16 bytes\")

  .shared_verifier_bss (NOLOAD) : ALIGN(8)
  {
    __shared_verifier_bss_start = .;
    KEEP(*(.shared_verifier_bss))
    __shared_verifier_bss_end = .;
  } > RAM
  ASSERT(__edata <= __policy_handoff_start, \"stage1 data overlaps policy handoff\")
  ASSERT(__ebss <= __policy_handoff_start, \"stage1 BSS overlaps policy handoff\")
  ASSERT(__euninit <= __policy_handoff_start, \"stage1 uninit overlaps policy handoff\")
  ASSERT(__shared_verifier_bss_start >= 0x20000000, \"shared verifier RAM base changed\")
  ASSERT(__shared_verifier_bss_end <= __policy_handoff_start, \"verifier state overlaps policy handoff\")
}
INSERT AFTER .bss;
";
    let shared_sections_path = out_dir.join("shared-verifier.x");
    fs::write(&shared_sections_path, shared_sections)
        .unwrap_or_else(|e| panic!("Failed to write shared verifier linker script: {e}"));

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!(
        "cargo:rustc-link-arg-bins=-T{}",
        shared_sections_path.display()
    );
    if env::var("CARGO_FEATURE_DEFMT").is_ok() {
        println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    }
}
