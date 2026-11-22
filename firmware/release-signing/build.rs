//! Refuses an optimized firmware build that trusts the development key unless the
//! builder opted in. Firmware that trusted it would accept images anyone can sign,
//! because the dev key's seed is public; see "Signed releases" in
//! `firmware/manifests/README.md`. Host tools (xtask signs with the dev key in
//! tests) are not firmware and are exempt.

fn main() {
    println!("cargo:rerun-if-env-changed=ODYSSEY_DEV_SIGNING");
    let dev_key = std::env::var_os("CARGO_FEATURE_DEV_SIGNING_KEY").is_some();
    let optimized = std::env::var("PROFILE").is_ok_and(|profile| profile == "release");
    let firmware = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "none");
    let opted_in = std::env::var("ODYSSEY_DEV_SIGNING").is_ok_and(|value| value == "1");
    assert!(
        !(dev_key && optimized && firmware && !opted_in),
        "release-signing: `dev-signing-key` in a release firmware build trusts a public key. \
         Set ODYSSEY_DEV_SIGNING=1 only for firmware you flash yourself."
    );
}
