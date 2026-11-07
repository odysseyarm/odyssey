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
    /// Flash a device over SWD with sector erase only: bootloader and app, or
    /// the app alone. Select a probe with PROBE_RS_PROBE=VID:PID:SERIAL.
    Flash {
        device: Device,
        /// Program the app image only, not the bootloader. Boards that keep swap state clear it.
        #[arg(long)]
        app_only: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Device {
    Lite1,
    LegacyAtslite,
    LegacyVm,
    MarkerControlBoard,
    /// The app only: its Fanstel bootloader is not built here, so SWD never touches it.
    Dongle,
}

fn flash(device: Device, app_only: bool) -> eros::Result<()> {
    use firmware::{Cmd, Task};
    let task = if app_only {
        Task::FlashApp
    } else {
        Task::Flash
    };
    match device {
        Device::Lite1 => firmware::run(Cmd::Lite1(task)),
        Device::LegacyAtslite => firmware::run(Cmd::LegacyAtslite(task)),
        Device::LegacyVm => firmware::run(Cmd::LegacyVm(task)),
        Device::MarkerControlBoard => firmware::run(Cmd::MarkerControlBoard(task)),
        Device::Dongle => dongle::run(dongle::Cmd::FlashSwd),
    }
}

fn main() -> eros::Result<()> {
    match Cli::parse().command {
        Command::Firmware(command) => firmware::run(command),
        Command::Dongle(command) => dongle::run(command),
        Command::Desktop(command) => desktop::run(command),
        Command::Sdk(command) => sdk::run(command),
        Command::Flash { device, app_only } => flash(device, app_only),
        Command::Autocalib(command) => autocalib::run(command),
    }
}
