//! Repository automation: `cargo xtask <command>`, run from the repository root.
//!
//! External tools (probe-rs, mergehex, nrfutil, gh, git, cargo-packager, dotnet)
//! are still invoked as programs; this crate replaces the shell glue around them.

mod autocalib;
mod cmd;
mod desktop;
mod dongle;
mod firmware;
mod sdk;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Odyssey repository automation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// lite1, legacy-atslite and legacy-vm: build, flash, attach, releases, manifests.
    #[command(subcommand)]
    Firmware(firmware::Cmd),
    /// The USB dongle firmware.
    #[command(subcommand)]
    Dongle(dongle::Cmd),
    /// Odyssey desktop installers and releases.
    #[command(subcommand)]
    Desktop(desktop::Cmd),
    /// Client SDK bindings.
    #[command(subcommand)]
    Sdk(sdk::Cmd),
    /// Scale-board artwork and the Orbbec FFI bindings.
    #[command(subcommand)]
    Autocalib(autocalib::Cmd),
}

fn main() -> eros::Result<()> {
    match Cli::parse().command {
        Command::Firmware(command) => firmware::run(command),
        Command::Dongle(command) => dongle::run(command),
        Command::Desktop(command) => desktop::run(command),
        Command::Sdk(command) => sdk::run(command),
        Command::Autocalib(command) => autocalib::run(command),
    }
}
