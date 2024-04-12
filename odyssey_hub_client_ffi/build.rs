use std::env;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut config: cbindgen::Config = Default::default();
    config.language = cbindgen::Language::Cxx;

    cbindgen::Builder::new()
        .with_crate(crate_dir.clone())
        .with_config(config)
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file("include/bindings.hpp");

    let mut config: cbindgen::Config = Default::default();
    config.language = cbindgen::Language::C;

    cbindgen::Builder::new()
        .with_crate(crate_dir.clone())
        .with_config(config)
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file("include/bindings.h");

    csbindgen::Builder::default()
        .input_extern_file("src/lib.rs")
        .input_extern_file("src/client.rs")
        .input_extern_file("src/ffi_common.rs")
        .input_extern_file("src/funny.rs")
        
        .csharp_dll_name("odyssey_hub_client_ffi")
        .generate_csharp_file("dotnet/OdysseyHubClient/OdysseyHubClient/generated/NativeMethods.g.cs")
        .unwrap();
}
