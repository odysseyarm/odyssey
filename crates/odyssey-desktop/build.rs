fn main() {
    println!("cargo:rerun-if-changed=icon.rc");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    #[cfg(windows)]
    embed_resource::compile("icon.rc", embed_resource::NONE)
        .manifest_required()
        .expect("the icon resource must compile into the .exe");
}
