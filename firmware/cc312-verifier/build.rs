use std::env;
use std::path::PathBuf;

fn required_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must name the pinned source tree"))
}

fn main() {
    println!("cargo:rerun-if-changed=src/cc312.c");
    println!("cargo:rerun-if-changed=src/cc312_verifier_config.h");
    println!("cargo:rerun-if-env-changed=DEP_NRFXLIB_SYS_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=PSA_CRYPTO_ROOT");
    let sdk = required_path("DEP_NRFXLIB_SYS_SDK_ROOT");
    let psa = required_path("PSA_CRYPTO_ROOT");
    let cc312 = sdk.join("crypto/nrf_cc312_mbedcrypto");
    let platform = sdk.join("crypto/nrf_cc312_platform");

    cc::Build::new()
        .file("src/cc312.c")
        .flag("-fomit-frame-pointer")
        .include("src")
        .include(psa.join("include"))
        .include(psa.join("core"))
        .include(psa.join("drivers/builtin/include"))
        .include(cc312.join("include"))
        .include(platform.join("include"))
        .define("PSA_CRYPTO_DRIVER_CC3XX", None)
        .define(
            "TF_PSA_CRYPTO_USER_CONFIG_FILE",
            Some("\"cc312_verifier_config.h\""),
        )
        .warnings(true)
        .compile("cc312_verifier");
}
