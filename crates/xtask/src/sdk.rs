use crate::cmd;

const DOTNET_DIR: &str = "sdk/odyssey-hub-client-uniffi/dotnet/OdysseyHubClient";
/// The uniffi-bindgen-cs tag must match the uniffi crate version in
/// sdk/odyssey-hub-client-uniffi/Cargo.toml (tag format: v<tool>+v<uniffi>).
/// nix/sdk.nix pins the same tag (and commit) for CI.
const UNIFFI_BINDGEN_CS_TAG: &str = "v0.11.0+v0.31.0";
const UNIFFI_BINDGEN_CS_GIT: &str = "https://github.com/NordSecurity/uniffi-bindgen-cs";
const LINUX_TARGETS: [&str; 2] = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];

#[derive(clap::Subcommand)]
pub(crate) enum Cmd {
    /// The C# (.NET) client, run on Windows.
    #[command(subcommand)]
    Dotnet(Dotnet),
}

#[derive(clap::Subcommand)]
pub(crate) enum Dotnet {
    /// One-time setup: install uniffi-bindgen-cs.
    InstallTools,
    /// Build the native library for windows (cargo) and linux (cross), and
    /// generate the C# bindings.
    BuildLib,
    /// Build and pack OdysseyArm.HubClient into nugets/, and publish
    /// OdysseyArm.HubClient.Test into output/.
    BuildProjects,
    /// build-lib, then build-projects.
    Build,
    /// build, then run-only.
    Run,
    /// Run the published OdysseyArm.HubClient.Test.
    RunOnly,
}

pub(crate) fn run(command: Cmd) -> eros::Result<()> {
    let Cmd::Dotnet(command) = command;
    match command {
        Dotnet::InstallTools => cmd::run(cmd::at(DOTNET_DIR, "cargo").args([
            "install",
            "uniffi-bindgen-cs",
            "--git",
            UNIFFI_BINDGEN_CS_GIT,
            "--tag",
            UNIFFI_BINDGEN_CS_TAG,
        ])),
        Dotnet::BuildLib => build_lib(),
        Dotnet::BuildProjects => build_projects(),
        Dotnet::Build => {
            build_lib()?;
            build_projects()
        }
        Dotnet::Run => {
            build_lib()?;
            build_projects()?;
            run_only()
        }
        Dotnet::RunOnly => run_only(),
    }
}

fn build_lib() -> eros::Result<()> {
    println!("OdysseyHubClient");
    println!("Building native library (Rust)");
    cmd::run(cmd::at(DOTNET_DIR, "cargo").args([
        "build",
        "--manifest-path=../../Cargo.toml",
        "-r",
        "--target=x86_64-pc-windows-msvc",
    ]))?;
    cmd::run(cmd::at(DOTNET_DIR, "cargo").args([
        "run",
        "--manifest-path=../../Cargo.toml",
        "-r",
        "--bin",
        "cs_bindgen",
        "--",
        "../../target/x86_64-pc-windows-msvc/release/ohc_uniffi.dll",
        "OdysseyHubClient/generated",
    ]))?;
    // See Cross.toml: cross has to mount the repository root the crates inherit from.
    for target in LINUX_TARGETS {
        cmd::run(
            cmd::at(DOTNET_DIR, "cross")
                .args(["build", "--manifest-path=../../Cargo.toml", "-r"])
                .arg(format!("--target={target}"))
                .env("CROSS_CONFIG", "../../Cross.toml")
                .env("ODYSSEY_REPO", cmd::root()),
        )?;
    }
    Ok(())
}

fn build_projects() -> eros::Result<()> {
    println!("Building OdysseyArm.HubClient");
    cmd::run(cmd::at(DOTNET_DIR, "dotnet").args(["build", "OdysseyHubClient", "-c", "Release"]))?;
    cmd::run(cmd::at(DOTNET_DIR, "dotnet").args(["pack", "OdysseyHubClient", "-o", "nugets"]))?;

    println!("Publishing OdysseyArm.HubClient.Test");
    cmd::run(cmd::at(DOTNET_DIR, "dotnet").args(["build", "OdysseyHubClient.Test"]))?;
    cmd::run(cmd::at(DOTNET_DIR, "dotnet").args([
        "publish",
        "OdysseyHubClient.Test",
        "-o",
        "output",
    ]))
}

fn run_only() -> eros::Result<()> {
    println!("Running published OdysseyArm.HubClient.Test");
    println!();
    let dir = cmd::root().join(DOTNET_DIR);
    let test = dir.join("output").join(format!(
        "OdysseyArm.HubClient.Test{}",
        std::env::consts::EXE_SUFFIX
    ));
    cmd::run(std::process::Command::new(test).current_dir(dir))
}
