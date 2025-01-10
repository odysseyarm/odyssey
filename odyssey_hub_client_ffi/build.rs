use std::env;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut config: cbindgen::Config = Default::default();
    config.language = cbindgen::Language::Cxx;

    cbindgen::Builder::new()
        .with_crate(crate_dir.clone())
        .with_config(config)
        .with_include_guard("ODYSSEY_HUB_CLIENT_LIB_HPP")
        .with_namespace("OdysseyHubClient")
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file("include/odyssey_hub_client_lib.hpp");

    let mut config: cbindgen::Config = Default::default();
    config.language = cbindgen::Language::C;
    config.cpp_compat = true;
    config.enumeration.prefix_with_name = true;
    config.enumeration.rename_variants = cbindgen::RenameRule::ScreamingSnakeCase;

    cbindgen::Builder::new()
        .with_crate(crate_dir.clone())
        .with_config(config)
        .with_include_guard("ODYSSEY_HUB_CLIENT_LIB_H")
        .with_item_prefix("OdysseyHubClient")
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file("include/odyssey_hub_client_lib.h");

    csbindgen::Builder::default()
        .input_extern_file("src/lib.rs")
        .input_extern_file("src/client.rs")
        .input_extern_file("src/ffi_common.rs")
        .input_extern_file("src/funny.rs")
        .csharp_dll_name("odyssey_hub_client_ffi")
        .generate_csharp_file(
            "dotnet/OdysseyHubClient/OdysseyHubClient/generated/NativeMethods.g.cs",
        )
        .unwrap();
}
