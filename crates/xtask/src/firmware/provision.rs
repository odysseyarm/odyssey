//! `provision-lite1`: back up, erase and program a modern ATS Lite over SWD.
//!
//! Order matters on the nRF5340: recover the network core before the
//! application core (probe-rs cannot unlock a held network core; nrfjprog can),
//! then program each core with its own image. Debug lock is deliberately out of
//! scope: it is irreversible and gets its own confirmed step after this image has
//! booted.

use std::collections::BTreeMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

use eros::Context as _;

use super::modern::{address, region};
use super::{hex_bytes, write_hex};

/// nRF5340 FICR INFO.PART.
const FICR_PART: u32 = 0x00FF_020C;
/// UICR words that keep each access port open: APPROTECT, SECUREAPPROTECT and
/// the network core's APPROTECT. Production locking is a separate later step.
const APPROTECT_WORDS: [u32; 3] = [0x00FF_8000, 0x00FF_801C, 0x01FF_8000];
const APPROTECT_DISABLED: u32 = 0x50FA_50FA;

#[derive(clap::Args)]
pub(crate) struct ProvisionArgs {
    /// Assembled image from `assemble-lite1`.
    #[arg(long)]
    image: PathBuf,
    /// Directory for the pre-erase backup of both cores. Must not exist yet.
    #[arg(long)]
    backup_dir: PathBuf,
    /// Carry the unit's settings page (calibration, bonds) over from the backup.
    #[arg(long)]
    keep_settings: bool,
}

pub(crate) fn provision_lite1(args: &ProvisionArgs) -> eros::Result<()> {
    let (settings, net_flash) = layout()?;
    let image = fs::read_to_string(&args.image)
        .with_context(|| format!("could not read {}", args.image.display()))?;
    let image = hex_bytes(&image)?;

    let ids = nrfjprog_output(&["--ids"])?;
    if ids.lines().filter(|line| !line.trim().is_empty()).count() != 1 {
        eros::bail!("connect exactly one J-Link; found:\n{ids}");
    }
    let part = read_word(FICR_PART)?;
    if part != 0x5340 {
        eros::bail!("target reports part {part:#x}, not an nRF5340");
    }

    // Everything below erases the unit, so the backup must be complete first.
    // The backup holds bond keys, so only the owner may read it.
    fs::create_dir_all(args.backup_dir.parent().unwrap_or(Path::new(".")))?;
    let mut dir = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut dir, 0o700);
    dir.create(&args.backup_dir).with_context(|| {
        format!(
            "backup directory {} must not exist",
            args.backup_dir.display()
        )
    })?;
    let app_backup = args.backup_dir.join("app.hex");
    let net_backup = args.backup_dir.join("net.hex");
    nrfjprog(&["--readcode", path(&app_backup)?, "--readuicr"])
        .context("app-core backup failed; a protected unit needs a manual recover")?;
    nrfjprog(&[
        "--coprocessor",
        "CP_NETWORK",
        "--readcode",
        path(&net_backup)?,
        "--readuicr",
    ])
    .context("network-core backup failed; a protected unit needs a manual recover")?;

    let backup = if args.keep_settings {
        Some(hex_bytes(&fs::read_to_string(&app_backup)?)?)
    } else {
        None
    };
    let (app, net) = split_image(&image, backup.as_deref(), settings, net_flash)?;
    let app_hex = args.backup_dir.join("provision-app.hex");
    let net_hex = args.backup_dir.join("provision-net.hex");
    fs::write(&app_hex, write_hex(&app))?;
    fs::write(&net_hex, write_hex(&net))?;

    nrfjprog(&["--coprocessor", "CP_NETWORK", "--recover"])?;
    nrfjprog(&["--recover"])?;
    nrfjprog(&[
        "--coprocessor",
        "CP_NETWORK",
        "--program",
        path(&net_hex)?,
        "--verify",
        "--chiperase",
    ])?;
    nrfjprog(&[
        "--coprocessor",
        "CP_APPLICATION",
        "--program",
        path(&app_hex)?,
        "--verify",
        "--chiperase",
    ])?;
    // Programming the app core must not have disturbed the network core.
    nrfjprog(&["--coprocessor", "CP_NETWORK", "--verify", path(&net_hex)?])?;
    nrfjprog(&["--pinreset"])?;
    println!(
        "Provisioned and verified both cores; backup in {}",
        args.backup_dir.display()
    );
    Ok(())
}

#[derive(clap::Args)]
pub(crate) struct LockArgs {
    /// Confirm that this unit should become undebuggable.
    #[arg(long)]
    yes: bool,
}

/// Programming a UICR word can only clear bits, so zero is a valid locked value.
const APPROTECT_LOCKED: &str = "0x0";

pub(crate) fn lock_debug_lite1(args: &LockArgs) -> eros::Result<()> {
    if !args.yes {
        eros::bail!("locking debug is hard to undo on a unit; pass --yes to continue");
    }
    let ids = nrfjprog_output(&["--ids"])?;
    if ids.lines().filter(|line| !line.trim().is_empty()).count() != 1 {
        eros::bail!("connect exactly one J-Link; found:\n{ids}");
    }
    if read_word(FICR_PART)? != 0x5340 {
        eros::bail!("target is not an nRF5340");
    }
    for word in APPROTECT_WORDS {
        if read_word_on(word)? != APPROTECT_DISABLED {
            eros::bail!("{word:#010x} is not in the provisioned open state; refusing to lock");
        }
    }
    for word in APPROTECT_WORDS {
        let mut command = Command::new("nrfjprog");
        command.args(["-f", "NRF53"]);
        if word >= 0x0100_0000 {
            command.args(["--coprocessor", "CP_NETWORK"]);
        }
        crate::cmd::run(command.args([
            "--memwr",
            &format!("{word:#010x}"),
            "--val",
            APPROTECT_LOCKED,
        ]))?;
    }
    nrfjprog(&["--pinreset"])?;
    if read_word(FICR_PART).is_ok() {
        eros::bail!("debug access is still open after reset");
    }
    println!(
        "Debug locked on both cores. Recover with `nrfjprog -f NRF53 --recover`, which erases settings and bonds."
    );
    Ok(())
}

fn read_word_on(address: u32) -> eros::Result<u32> {
    let mut command = Command::new("nrfjprog");
    command.args(["-f", "NRF53"]);
    if address >= 0x0100_0000 {
        command.args(["--coprocessor", "CP_NETWORK"]);
    }
    let out = crate::cmd::output(command.args(["--memrd", &format!("{address:#010x}")]))?;
    let word = out
        .split_whitespace()
        .nth(1)
        .with_context(|| format!("unexpected nrfjprog output {out:?}"))?;
    Ok(u32::from_str_radix(word, 16).with_context(|| format!("bad word {word:?}"))?)
}

/// The settings page and the start of network-core flash, from the memory map.
fn layout() -> eros::Result<(Range<u32>, u32)> {
    let path = crate::cmd::root().join("firmware/memory/memory-lite1.kdl");
    let text =
        fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
    let spec = text
        .parse::<memory_spec::MemorySpec>()
        .map_err(|error| eros::error!("invalid lite1 memory layout: {error}"))?;
    let regions = spec.regions();
    let flash = regions
        .get("flash")
        .context("memory map lacks flash regions")?;
    let settings = region(flash, "settings")?;
    let start = address(settings)?;
    let len = u32::try_from(settings.length()).context("settings length exceeds u32")?;
    let net = regions
        .get("netcore_flash")
        .context("memory map lacks netcore_flash")?;
    Ok((start..start + len, address(net)?))
}

/// Splits the image by core, adds the settings page from `backup` and keeps
/// both access ports open. Refuses an image that already writes the settings page.
fn split_image(
    image: &[(u32, u8)],
    backup: Option<&[(u32, u8)]>,
    settings: Range<u32>,
    net_flash: u32,
) -> eros::Result<(BTreeMap<u32, u8>, BTreeMap<u32, u8>)> {
    let mut bytes: BTreeMap<u32, u8> = image.iter().copied().collect();
    if bytes.keys().any(|address| settings.contains(address)) {
        eros::bail!("image writes the settings page; refusing to overwrite unit settings");
    }
    if let Some(backup) = backup {
        bytes.extend(
            backup
                .iter()
                .filter(|(address, byte)| settings.contains(address) && *byte != 0xFF)
                .copied(),
        );
    }
    for word in APPROTECT_WORDS {
        for (offset, byte) in APPROTECT_DISABLED.to_le_bytes().into_iter().enumerate() {
            bytes.insert(word + offset as u32, byte);
        }
    }
    // The network core's UICR sits above its flash, so one split covers both.
    let net = bytes.split_off(&net_flash);
    Ok((bytes, net))
}

fn path(path: &Path) -> eros::Result<&str> {
    Ok(path
        .to_str()
        .with_context(|| format!("{} is not UTF-8", path.display()))?)
}

fn nrfjprog(args: &[&str]) -> eros::Result<()> {
    crate::cmd::run(Command::new("nrfjprog").args(["-f", "NRF53"]).args(args))
}

fn nrfjprog_output(args: &[&str]) -> eros::Result<String> {
    crate::cmd::output(Command::new("nrfjprog").args(args))
}

fn read_word(address: u32) -> eros::Result<u32> {
    let out = nrfjprog_output(&["-f", "NRF53", "--memrd", &format!("{address:#010x}")])?;
    let word = out
        .split_whitespace()
        .nth(1)
        .with_context(|| format!("unexpected nrfjprog output {out:?}"))?;
    Ok(u32::from_str_radix(word, 16).with_context(|| format!("bad word {word:?}"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: Range<u32> = 0xFE000..0x0010_0000;
    const NET: u32 = 0x0100_0000;

    #[test]
    fn splits_cores_keeps_settings_and_opens_access_ports() {
        let image = [(0x0000, 1), (0xF000, 2), (0x0100_0000, 3), (0x0100_F000, 4)];
        let backup = [(0xFE000, 9), (0xFE001, 0xFF), (0x1000, 7)];
        let (app, net) = split_image(&image, Some(&backup), SETTINGS, NET).unwrap();
        assert_eq!(app[&0xFE000], 9);
        assert!(
            !app.contains_key(&0xFE001),
            "erased settings bytes are not programmed"
        );
        assert!(
            !app.contains_key(&0x1000),
            "only the settings page is carried over"
        );
        assert_eq!(app[&0x0000], 1);
        assert_eq!(net[&0x0100_F000], 4);
        assert!(app.keys().all(|address| *address < NET));
        assert!(net.keys().all(|address| *address >= NET));
        assert_eq!(app[&0x00FF_8000], 0xFA);
        assert_eq!(app[&0x00FF_801C], 0xFA);
        assert_eq!(net[&0x01FF_8003], 0x50);
    }

    #[test]
    fn refuses_image_that_overwrites_settings() {
        split_image(&[(0xFE010, 0)], None, SETTINGS, NET).unwrap_err();
    }
}
