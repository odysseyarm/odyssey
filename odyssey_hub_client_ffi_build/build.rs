use std::env;
use interoptopus::backend::NamespaceMappings;
use interoptopus::inventory::Bindings;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    build_c(&crate_dir)?;
    build_csharp(&crate_dir)?;

    Ok(())
}

fn build_c(crate_dir: &String) -> Result<(), Box<dyn std::error::Error>> {
    use interoptopus_backend_c::InteropBuilder;

    InteropBuilder::default()
        .inventory(odyssey_hub_client_ffi::ffi_inventory())
        .ifndef(String::from("ODYSSEY_HUB_CLIENT_LIB_H"))
        .function_parameter_naming(interoptopus_backend_c::NameCase::Snake)
        .type_naming(interoptopus_backend_c::NameCase::Snake)
        .enum_variant_naming(interoptopus_backend_c::NameCase::ShoutySnake)
        .build()?
        .write_file(std::path::PathBuf::from(crate_dir)
        .join("include/odyssey_hub_client_lib.h"))?;

    Ok(())
}

fn build_csharp(crate_dir: &String) -> Result<(), Box<dyn std::error::Error>> {
    use interoptopus_backend_csharp::InteropBuilder;
    
    InteropBuilder::default()
        .inventory(odyssey_hub_client_ffi::ffi_inventory())
        .dll_name("odyssey_hub_client_ffi")
        .namespace_mappings(NamespaceMappings::new("Radiosity.OdysseyHubClient"))
        .build()?
        .write_file(std::path::PathBuf::from(crate_dir)
        .join("dotnet/OdysseyHubClient/OdysseyHubClient/generated/NativeMethods.g.cs"))?;

    Ok(())
}
