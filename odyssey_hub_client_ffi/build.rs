use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    build_c(&crate_dir)?;

    Ok(())
}

fn build_c(crate_dir: &String) -> Result<(), Box<dyn std::error::Error>> {
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
        .with_parse_expand_all_features(true)
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file("../odyssey_hub_client_ffi/include/odyssey_hub_client_lib.h");

    Ok(())
}
