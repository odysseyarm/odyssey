use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // cbindgen probes rustc for target cfg info via a raw `rustc` invocation
    // that doesn't pass `--target`, so it always resolves to the host. Under
    // cargo-xwin, CARGO_ENCODED_RUSTFLAGS/RUSTFLAGS carry the cross target's
    // linker flags (e.g. -C linker-flavor=lld-link), which that host-mode
    // probe then applies against the host toolchain and fails. Header
    // generation never needs to link anything, so just drop them.
    unsafe {
        env::remove_var("CARGO_ENCODED_RUSTFLAGS");
        env::remove_var("RUSTFLAGS");
    }

    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    build_c(&crate_dir)?;

    Ok(())
}

fn build_c(crate_dir: &String) -> Result<(), Box<dyn std::error::Error>> {
    // C header
    let mut config: cbindgen::Config = Default::default();
    config.language = cbindgen::Language::C;
    config.cpp_compat = true;
    config.enumeration.prefix_with_name = true;
    config.enumeration.rename_variants = cbindgen::RenameRule::ScreamingSnakeCase;

    cbindgen::Builder::new()
        .with_crate(crate_dir.clone())
        .with_config(config)
        .with_include_guard("OHC_H")
        .with_item_prefix("ohc_")
        .with_parse_deps(true)
        .with_parse_include(&["odyssey_hub_common"])
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(crate_dir.to_string() + "/generated/include/ohc.h");

    // C++ header
    let mut config: cbindgen::Config = Default::default();
    config.language = cbindgen::Language::Cxx;
    config.cpp_compat = true;
    config.enumeration.rename_variants = cbindgen::RenameRule::ScreamingSnakeCase;
    config.header = Some(
        indoc::indoc! {"
            #if 0
            ''' '
            #endif

            #ifdef __cplusplus
            template <typename T>
            using MaybeUninit = T;
            #endif

            #if 0
            ' '''
            #endif
        "}
        .to_string(),
    );
    config.export.exclude = vec![String::from("MaybeUninit")];

    cbindgen::Builder::new()
        .with_crate(crate_dir.clone())
        .with_config(config)
        .with_namespace("ohc")
        .with_include_guard("OHC_H")
        .with_parse_deps(true)
        .with_parse_include(&["odyssey_hub_common"])
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(crate_dir.to_string() + "/generated/include/ohc.hpp");

    let src_include = Path::new(crate_dir).join("include");
    let dst_include = Path::new(crate_dir).join("generated/include");

    for entry in fs::read_dir(&src_include)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let filename = path.file_name().unwrap();
            let dest_path = dst_include.join(filename);
            fs::copy(&path, &dest_path)?;
        }
    }

    Ok(())
}
