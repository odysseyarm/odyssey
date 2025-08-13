use std::{env, fs, path::Path};

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
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(crate_dir.to_string() + "/generated/include/ohc.h");

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
