use std::fmt::Write as _;
use std::path::PathBuf;
use std::str::FromStr;
use std::{env, fs, io};

pub fn build(bootloader: bool) -> io::Result<()> {
    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets its manifest directory"),
    );
    let map = manifest.join("../memory/memory-marker-control-board.kdl");
    println!("cargo:rerun-if-changed={}", map.display());
    println!("cargo:rerun-if-changed=../marker-control-board/build_support.rs");
    let spec = memory_spec::MemorySpec::from_str(&fs::read_to_string(map)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let regions = spec.regions();
    let flash = &regions["flash"];
    let active = &flash["app_flash"];
    let dfu = &flash["dfu"];
    let state = &flash["bootloader_state"];
    let loader = &flash["bootloader"];
    let bonds = &flash["owner_bonds"];
    let ram = &regions["ram"];
    assert_eq!(loader.origin(), flash.origin());
    assert_eq!(loader.end(), bonds.origin());
    assert_eq!(bonds.end(), state.origin());
    assert_eq!(bonds.length(), 8192);
    assert_eq!(state.end(), active.origin());
    assert_eq!(active.end(), dfu.origin());
    assert_eq!(dfu.end(), flash.end());
    assert_eq!(dfu.length(), active.length() + 4096);
    assert!(state.length() >= (2 + 4 * (active.length() / 4096)) * 4);
    for region in [loader, bonds, state, active, dfu] {
        assert_eq!(region.origin() % 4096, 0);
        assert_eq!(region.length() % 4096, 0);
    }
    let code = if bootloader { loader } else { active };
    let mut script = String::from("MEMORY {\n");
    for (name, region) in [("FLASH", code), ("RAM", ram)] {
        writeln!(
            script,
            "  {name} : ORIGIN = {:#x}, LENGTH = {:#x}",
            region.origin(),
            region.length()
        )
        .expect("formatting into String cannot fail");
    }
    script.push_str("}\n");
    script.push_str(&spec.render_symbols());
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets its build output directory"));
    fs::write(out.join("memory.x"), script)?;
    fs::write(
        out.join("layout.rs"),
        format!(
            "pub const ACTIVE_START: u32 = {:#x};\npub const ACTIVE_END: u32 = {:#x};\npub const RAM_START: u32 = {:#x};\npub const RAM_END: u32 = {:#x};\n",
            active.origin(),
            active.end(),
            ram.origin(),
            ram.end(),
        ),
    )?;
    fs::write(
        out.join("owner_bonds.rs"),
        format!(
            "pub const OWNER_BONDS_START: u32 = {:#x};\n",
            bonds.origin()
        ),
    )?;
    let mut version = [0; 3];
    for (index, part) in ["MAJOR", "MINOR", "PATCH"].into_iter().enumerate() {
        version[index] = env::var(format!("CARGO_PKG_VERSION_{part}"))
            .expect("Cargo sets numeric semver components")
            .parse::<u16>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    }
    fs::write(
        out.join("version.rs"),
        format!("pub const FIRMWARE_VERSION: [u16; 3] = {version:?};\n"),
    )?;
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    Ok(())
}
