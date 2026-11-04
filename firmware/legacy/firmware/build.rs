//! Copies the memory layout and the RAM-executing linker script (`memory.x` and
//! `link-ram.x`, or their `-mcuboot` variants) into `OUT_DIR` and puts that
//! directory on the linker search path, so the linker finds them from a
//! workspace build too. Cargo reruns this script when either file changes.

use std::env;
use std::path::PathBuf;

fn main() {
    // The linker does not search the crate root from a workspace build.
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rustc-link-search={}", out.display());

    if cfg!(feature = "mcuboot") {
        std::fs::write(
            out.join("memory-mcuboot.x"),
            include_bytes!("memory-mcuboot.x"),
        )
        .unwrap();
        std::fs::write(
            out.join("link-ram-mcuboot.x"),
            include_bytes!("link-ram-mcuboot.x"),
        )
        .unwrap();
        println!("cargo:rerun-if-changed=memory-mcuboot.x");
        println!("cargo:rerun-if-changed=link-ram-mcuboot.x");
        println!("cargo:rustc-link-arg-bins=-Tlink-ram-mcuboot.x");
    } else {
        std::fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
        std::fs::write(out.join("link-ram.x"), include_bytes!("link-ram.x")).unwrap();
        println!("cargo:rerun-if-changed=memory.x");
        println!("cargo:rerun-if-changed=link-ram.x");
        println!("cargo:rustc-link-arg-bins=-Tlink-ram.x");
    }

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
}
