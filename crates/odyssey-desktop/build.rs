fn main() {
    println!("cargo:rerun-if-changed=icon.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    // The target, not `cfg!(windows)` (the host): the Windows exe is also cross-built from Linux.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("icon.rc", embed_resource::NONE)
            .manifest_required()
            .expect("the icon resource must compile into the .exe");
    }
}
