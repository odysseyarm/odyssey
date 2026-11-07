use std::fs;
use std::path::Path;

use eros::Context as _;

mod uf2;

use crate::cmd;

const DIR: &str = "firmware/dongle-fw";
const TARGET: &str = "thumbv7em-none-eabihf";
const BIN: &str = "dongle-fw";
/// `target/<triple>/release`, relative to the crate directory.
const BUILT: &str = "target/thumbv7em-none-eabihf/release";
const CHIP: &str = "nRF52840_xxAA";

const HW_VERSION: &str = "52";
/// nrf-sdc links no SoftDevice, so the DFU package requires none.
const SD_REQ: &str = "0x00";
/// A dongle's stock bootloader is Nordic's open secure bootloader, which only
/// accepts packages signed with the vendor's own key; anything else is rejected
/// without a chip erase. Fanstel publishes its key at fanstel.com/s/private.pem,
/// Laird (Ezurio) in LairdCP/Nordic_DFU_BL654_USB as laird_dongle_public.pem.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum Vendor {
    Fanstel,
    Laird,
}

impl Vendor {
    const ALL: [Self; 2] = [Self::Fanstel, Self::Laird];

    const fn name(self) -> &'static str {
        match self {
            Self::Fanstel => "fanstel",
            Self::Laird => "laird",
        }
    }

    const fn key_file(self) -> &'static str {
        match self {
            Self::Fanstel => "fanstel-debug.pem",
            Self::Laird => "laird-dongle.pem",
        }
    }
}

/// Mass-storage bootloaders (the Adafruit nRF52 one and its relatives) check
/// this nRF52840 family id before accepting a block.
const UF2_FAMILY: u32 = 0xADA5_2840;

#[derive(clap::Subcommand)]
pub(crate) enum Cmd {
    /// Build and convert the firmware to Intel HEX (the toolchain's llvm-objcopy).
    Hex,
    /// Build the signed DFU packages, one per vendor bootloader (nrfutil).
    DfuPkg,
    /// Build the UF2 that mass-storage bootloaders (the Adafruit nRF52 one and
    /// its relatives) take dropped onto the drive they expose.
    Uf2,
    /// Everything a release ships, named for the version in Cargo.toml, in
    /// firmware/dongle-fw/dist: .hex for programmers and bootloaders that take
    /// Intel HEX (probes), .uf2 for mass-storage bootloaders, and a signed .zip
    /// per vendor bootloader (Fanstel, Laird) for serial DFU, which is what the
    /// app updates over.
    Dist,
    /// Build the DFU package and flash it over the bootloader's serial port.
    /// The vendor must match the dongle's bootloader or it rejects the package.
    Flash {
        #[arg(long, value_enum)]
        vendor: Vendor,
        #[arg(long, default_value = "COM78")]
        port: String,
        #[arg(long, default_value = "115200")]
        baud: String,
    },
    /// Print where the ELF, HEX and DFU packages are built.
    Paths,
    /// Print the defmt log the firmware writes to its serial port.
    Attach {
        #[arg(long, default_value = "COM79")]
        port: String,
    },
    /// Build and run on an nRF52840 DK with RTT logging (probe-rs).
    FlashDk,
    /// Build and program the app over SWD (probe-rs), erasing only the sectors it
    /// covers. Needs the stock bootloader already installed.
    FlashSwd,
    /// Add the published release to the alpha manifest so desktops prompt for it.
    /// Run after the release workflow has published the assets.
    GenRelease {
        /// The oldest Odyssey Desktop this firmware is required and tested with.
        min_desktop: String,
    },
    /// Attach probe-rs to a running DK.
    AttachDk,
}

pub(crate) fn run(command: Cmd) -> eros::Result<()> {
    match command {
        Cmd::Hex => hex(),
        Cmd::DfuPkg => {
            hex()?;
            dfu_pkg(&Vendor::ALL)
        }
        Cmd::GenRelease { min_desktop } => {
            let version = cmd::package_version(DIR)?;
            let assets = Vendor::ALL.map(|vendor| {
                let name = vendor.name();
                (
                    format!("dfu-{name}"),
                    format!("{BIN}-{version}-dfu-{name}.zip"),
                )
            });
            crate::firmware::gen_asset_release(
                "dongle",
                &format!("dongle-v{version}"),
                &version,
                &min_desktop,
                &assets,
            )
        }
        Cmd::Uf2 => {
            hex()?;
            uf2()
        }
        Cmd::FlashSwd => {
            hex()?;
            cmd::run(
                cmd::at(DIR, "probe-rs")
                    .args([
                        "download",
                        "--chip",
                        CHIP,
                        "--binary-format",
                        "hex",
                        "--verify",
                        "--reset",
                    ])
                    .arg(hex_file()),
            )
        }
        Cmd::Dist => dist(),
        Cmd::Flash { vendor, port, baud } => {
            hex()?;
            dfu_pkg(&[vendor])?;
            println!("==> DFU over serial port {port} (baud {baud})");
            cmd::run(
                cmd::at(DIR, "nrfutil")
                    .args(["nrf5sdk-tools", "dfu", "serial", "-pkg"])
                    .arg(dfu_zip(vendor))
                    .args(["-p", &port, "-b", &baud, "--flow-control", "true"]),
            )?;
            println!("Done.");
            Ok(())
        }
        Cmd::Paths => {
            println!("ELF: ./{BUILT}/{BIN}");
            println!("HEX: {}", hex_file());
            for vendor in Vendor::ALL {
                println!("ZIP ({}): {}", vendor.name(), dfu_zip(vendor));
            }
            Ok(())
        }
        Cmd::Attach { port } => cmd::run(
            cmd::at(DIR, "defmt-print")
                .args(["--log-format", "oneline", "-e"])
                .arg(elf())
                .args(["serial", "--dtr", "--path", &port]),
        ),
        Cmd::FlashDk => cmd::run(cmd::at(DIR, "cargo").args([
            "run",
            "--release",
            "--config",
            ".cargo/config-dk.toml",
            "--features",
            "rtt",
        ])),
        Cmd::AttachDk => cmd::run(
            cmd::at(DIR, "probe-rs")
                .args(["attach", "--chip", CHIP])
                .arg(elf()),
        ),
    }
}

fn elf() -> String {
    format!("./{BUILT}/{BIN}")
}

fn hex_file() -> String {
    format!("./{BUILT}/{BIN}.hex")
}

fn uf2_file() -> String {
    format!("./{BUILT}/{BIN}.uf2")
}

fn dfu_zip(vendor: Vendor) -> String {
    format!("./{BUILT}/{BIN}-dfu-{}.zip", vendor.name())
}

fn hex() -> eros::Result<()> {
    println!("==> Building and converting to HEX (llvm-objcopy)");
    cmd::run(cmd::at(DIR, "cargo").args(["build", "--release", "--bin", BIN, "--target", TARGET]))?;
    let hex = hex_file();
    cmd::run(
        cmd::at(DIR, cmd::llvm_objcopy()?)
            .args(["-O", "ihex"])
            .arg(elf())
            .arg(&hex),
    )?;
    println!("HEX: {hex}");
    // A peek at the first records only; failing to read them is not an error.
    if let Ok(text) = fs::read_to_string(cmd::root().join(DIR).join(&hex)) {
        for line in text.lines().take(2) {
            println!("{line}");
        }
    }
    Ok(())
}

fn dfu_pkg(vendors: &[Vendor]) -> eros::Result<()> {
    println!("==> Creating DFU packages (nrfutil)");
    let version = cmd::package_version(DIR)?;
    let application_version = application_version(&version)?.to_string();
    for &vendor in vendors {
        let pkg = dfu_zip(vendor);
        cmd::run(
            cmd::at(DIR, "nrfutil")
                .args([
                    "nrf5sdk-tools",
                    "pkg",
                    "generate",
                    "--hw-version",
                    HW_VERSION,
                ])
                .args(["--application-version", &application_version])
                .args(["--sd-req", SD_REQ, "--application"])
                .arg(hex_file())
                .args(["--key-file", vendor.key_file(), &pkg]),
        )?;
        println!("PKG ({}): {pkg}", vendor.name());
    }
    Ok(())
}

fn uf2() -> eros::Result<()> {
    println!("==> Converting to UF2");
    let dir = cmd::root().join(DIR);
    let hex_path = dir.join(hex_file());
    let hex = fs::read_to_string(&hex_path)
        .with_context(|| format!("could not read {}", hex_path.display()))?;
    let uf2 = uf2_file();
    let uf2_path = dir.join(&uf2);
    fs::write(&uf2_path, uf2::from_ihex(&hex, UF2_FAMILY)?)
        .with_context(|| format!("could not write {}", uf2_path.display()))?;
    println!("UF2: {uf2}");
    Ok(())
}

fn dist() -> eros::Result<()> {
    cmd::require_clean_tree(DIR)?;
    hex()?;
    uf2()?;
    dfu_pkg(&Vendor::ALL)?;
    let version = cmd::package_version(DIR)?;
    let crate_dir = cmd::root().join(DIR);
    let out = crate_dir.join("dist");
    if out.exists() {
        fs::remove_dir_all(&out).with_context(|| format!("removing {}", out.display()))?;
    }
    fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    let built = crate_dir.join(BUILT);
    let mut files = vec![
        (format!("{BIN}.hex"), format!("{BIN}-{version}.hex")),
        (format!("{BIN}.uf2"), format!("{BIN}-{version}.uf2")),
    ];
    for vendor in Vendor::ALL {
        let name = vendor.name();
        files.push((
            format!("{BIN}-dfu-{name}.zip"),
            format!("{BIN}-{version}-dfu-{name}.zip"),
        ));
    }
    for (from, to) in files {
        copy(&built.join(from), &out.join(to))?;
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> eros::Result<()> {
    let bytes = fs::copy(from, to)
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    println!("{bytes:>10} {}", to.display());
    Ok(())
}

/// The bootloader refuses a package whose application-version is below the one
/// already flashed, so this has to rise with every release. major*10000 +
/// minor*100 + patch keeps it monotonic and readable: 0.1.10 -> 110.
fn application_version(version: &str) -> eros::Result<u32> {
    let parts: Vec<&str> = version.split('.').collect();
    let [major, minor, patch] = parts.as_slice() else {
        eros::bail!("version {version} is not major.minor.patch");
    };
    let mut number = 0;
    for (part, scale) in [(major, 10000), (minor, 100), (patch, 1)] {
        let value = part
            .parse::<u32>()
            .with_context(|| format!("version {version}: {part} is not a number"))?;
        number += value * scale;
    }
    Ok(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_version_rises_with_every_component() {
        assert_eq!(application_version("0.1.10").ok(), Some(110));
        assert_eq!(application_version("0.1.9").ok(), Some(109));
        assert_eq!(application_version("1.2.3").ok(), Some(10203));
        assert_eq!(application_version("0.0.0").ok(), Some(0));
    }

    #[test]
    fn application_version_rejects_what_is_not_major_minor_patch() {
        assert!(application_version("0.1").is_err());
        assert!(application_version("0.1.2.3").is_err());
        assert!(application_version("0.1.10-rc1").is_err());
    }
}
