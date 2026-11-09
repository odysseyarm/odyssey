use std::path::PathBuf;
use std::str::FromStr as _;
use std::{env, fs};

use memory_spec::MemorySpec;

fn main() {
    let memory =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets manifest directory"))
            .join("../../firmware/dongle-fw/memory-dongle.kdl");
    println!("cargo:rerun-if-changed={}", memory.display());
    let text = fs::read_to_string(memory).expect("dongle memory map is present");
    let spec = MemorySpec::from_str(&text).expect("dongle memory map is valid");
    let regions = spec.regions();
    let flash = &regions["flash"];
    let app = &flash["app_flash"];
    let settings = &flash["settings"];
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets output directory"));
    fs::write(out.join("dongle_layout.rs"), format!(
        "pub(super) const APP_ORIGIN: u32 = {};\npub(super) const APPLICATION_SIZE: u32 = {};\npub(super) const SETTINGS_ORIGIN: u32 = {};\npub(super) const FLASH_END: u32 = {};\n",
        app.origin(), app.length(), settings.origin(), flash.end(),
    )).expect("write generated dongle layout");
}
