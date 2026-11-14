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
    let layout = |name: &str, value: u64| {
        let digits = value.to_string();
        let mut grouped = String::new();
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index) % 3 == 0 {
                grouped.push('_');
            }
            grouped.push(digit);
        }
        format!("pub(super) const {name}: u32 = {grouped};\n")
    };
    let layout_source = [
        layout("APP_ORIGIN", app.origin()),
        layout("APPLICATION_SIZE", app.length()),
        layout("SETTINGS_ORIGIN", settings.origin()),
        layout("FLASH_END", flash.end()),
    ]
    .concat();
    fs::write(out.join("dongle_layout.rs"), layout_source).expect("write generated dongle layout");
}
