mod board;
mod manifest;
mod release;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use board::{Board, Cores, Crate, Flash, LEGACY_ATSLITE, LEGACY_VM, LITE1};
use eros::Context as _;
use release::Part;
use sha2::{Digest as _, Sha256};

use crate::cmd;

/// Where `build`, `build-dual-bank` and `flash-app` put their images, inside the board directory.
const OUTPUT_DIR: &str = "build";
/// stage1_state and stage2_state in memory-*.kdl. Erased so both bootloaders
/// start from 0xFFFFFFFF.
const BOOT_STATE_PAGES: [&str; 2] = ["0xF9000", "0xFA000"];
/// Guard pages between boot state and settings in memory-*.kdl, erased after a
/// chip erase or recover, which have already cleared the boot state pages.
const GUARD_PAGES: [&str; 2] = ["0xFC000", "0xFD000"];
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const USER_AGENT: &str = "odyssey-firmware-pack";

#[derive(clap::Subcommand)]
pub(crate) enum Cmd {
    /// ATS Lite 1 (nRF5340): firmware/lite1 with its bootloaders and net core.
    #[command(subcommand)]
    Lite1(Task),
    /// Legacy ATS Lite (nRF5340): firmware/legacy-atslite with its bootloaders and net core.
    #[command(subcommand)]
    LegacyAtslite(Task),
    /// Legacy VM (nRF52840): firmware/legacy-vm and its bootloader.
    #[command(subcommand)]
    LegacyVm(Task),
    /// Print the release counter a bootloader or net-core main.rs declares with
    /// `macro_rules! release { () => { N }; }`, or nothing when there is none.
    ///
    /// The source is read as tokens, with all whitespace dropped, so rustfmt may
    /// lay the macro out however it likes. The board commands and the bootloader
    /// and net-core release workflows all read the counter through here, so there
    /// is one parser to keep right.
    ReleaseCounter {
        /// The main.rs to read.
        source: PathBuf,
    },
    /// Build an offline firmware pack: everything an Odyssey in offline mode needs
    /// to update devices, in one folder, laid out like GitHub so nothing has to be
    /// named by hand.
    ///
    /// For each channel it writes OUT/releases.<channel>.json, holding the newest
    /// N releases of each device from the manifests, and downloads every file
    /// those releases list into OUT/<tag>/<name>.bin, checking each against the
    /// size and sha256 in the manifest. Both application slots (app-a and app-b)
    /// are always downloaded, because an update writes whichever slot the unit is
    /// not running.
    ///
    /// The result goes wherever the administrator points Odyssey's
    /// `firmware_pack_dir` policy, by default
    /// %ProgramData%\OdysseyArm\Odyssey\firmware-pack. Odyssey itself leaves out
    /// any release whose files are not all there (see
    /// crates/odyssey-hub-server/src/firmware/pack.rs), and this runs the same
    /// completeness check before it finishes.
    BuildPack(manifest::PackArgs),
    /// Copy one release entry from the alpha manifest to the stable one.
    ///
    /// Stable only ever contains entries someone promoted here, so this is the one
    /// place customers' firmware changes. Entries are prepended, keeping the
    /// newest-first order hosts rely on.
    Promote {
        /// The manifest device key, e.g. atslite1.
        device: String,
        version: String,
    },
}

/// The recipes of one board. A board without the parts a recipe needs refuses it.
#[derive(clap::Subcommand)]
pub(crate) enum Task {
    /// Clean build artifacts.
    Clean,
    /// Build the application (bank A on a dual-bank board).
    BuildApp,
    /// Build the application for bank A.
    BuildAppA,
    /// Build the application for bank B.
    BuildAppB,
    /// Build the stage1 bootloader.
    BuildStage1,
    /// Build the bootloader (the DFU stage2 bootloader on nRF5340 boards).
    BuildBootloader,
    /// Build the stage2 bootloader artifacts a bootloader release publishes, into dist-bootloader/.
    DistBootloader,
    /// Publish the stage2 bootloader as its own GitHub release, <device>-bootloader-v<release>.
    ///
    /// Set DRY_RUN=1 to run every check and stop before publishing.
    ///
    /// The release is created as a draft and published last, so a tag never exists
    /// without its assets; the bootloader-release workflow skips a tag that already
    /// has a release, so the two ways of publishing cannot overwrite each other.
    ReleaseBootloader,
    /// Build the net core.
    BuildNet,
    /// Build the net-core artifacts a net-core release publishes, into dist-netcore/.
    DistNetcore,
    /// Publish the net core as its own GitHub release, <device>-netcore-v<release>.
    ///
    /// Set DRY_RUN=1 to run every check and stop before publishing.
    ///
    /// The release is created as a draft and published last, so a tag never exists
    /// without its assets; the netcore-release workflow skips a tag that already
    /// has a release, so the two ways of publishing cannot overwrite each other.
    ReleaseNetcore,
    /// Build the net-core bootloader.
    BuildNetcoreBootloader,
    /// Build the complete firmware into build/, with both app banks on dual-bank boards.
    Build,
    /// Build complete dual-bank firmware with both app banks and DFU binaries.
    BuildDualBank,
    /// Flash the complete firmware.
    Flash,
    /// Flash with chip erase.
    FlashChiperase,
    /// Flash with recover.
    FlashRecover,
    /// Flash dual-bank with chip erase.
    FlashDualChiperase,
    /// Flash dual-bank with recover.
    FlashDualRecover,
    /// Flash the application only.
    FlashApp,
    /// Attach the probe-rs debugger to the application (single-core boards).
    Attach,
    /// Attach probe-rs to the app core running bank A.
    AttachAppA,
    /// Attach probe-rs to the app core running bank B.
    AttachAppB,
    /// Attach probe-rs to the network core.
    AttachNet,
    /// Attach to the app core bootloader (net core unlocked for debugging).
    AttachBootloader,
    /// Attach probe-rs to the network core bootloader.
    AttachNetBootloader,
    /// Reset the device.
    Reset,
    /// Check all components.
    Check,
    /// Generate a release entry in firmware/manifests/releases.alpha.json from the current build.
    ///
    /// None of the arguments has a default: leaving one out would silently mean "any".
    GenRelease {
        /// The lowest bootloader release this application can run on, 0 for none.
        requires_bootloader: u32,
        /// The net-core release the unit must already be running before this entry
        /// installs, 0 for none. This entry's own net core never counts, so it may not
        /// exceed what an existing entry carries.
        requires_netcore: u32,
        /// The oldest Odyssey Desktop this firmware is required and tested with, which
        /// is not automatically the desktop shipping alongside it.
        min_desktop: String,
    },
}

pub(crate) fn run(command: Cmd) -> eros::Result<()> {
    match command {
        Cmd::Lite1(task) => run_task(&LITE1, task),
        Cmd::LegacyAtslite(task) => run_task(&LEGACY_ATSLITE, task),
        Cmd::LegacyVm(task) => run_task(&LEGACY_VM, task),
        Cmd::ReleaseCounter { source } => {
            let text = fs::read_to_string(&source)
                .with_context(|| format!("could not read {}", source.display()))?;
            if let Some(counter) = release::counter(&text) {
                println!("{counter}");
            }
            Ok(())
        }
        Cmd::BuildPack(args) => manifest::build_pack(&args),
        Cmd::Promote { device, version } => manifest::promote(&device, &version),
    }
}

fn run_task(board: &Board, task: Task) -> eros::Result<()> {
    match task {
        Task::Clean => clean(board),
        Task::BuildApp => build(&board.app),
        Task::BuildAppA => build_bank(board, Bank::A),
        Task::BuildAppB => build_bank(board, Bank::B),
        Task::BuildStage1 => build(&board.dual_core()?.stage1),
        Task::BuildBootloader => build(&board.bootloader),
        Task::DistBootloader => dist(board, Part::Bootloader),
        Task::ReleaseBootloader => release::publish(board, Part::Bootloader),
        Task::BuildNet => build(&board.dual_core()?.net_core),
        Task::DistNetcore => dist(board, Part::NetCore),
        Task::ReleaseNetcore => release::publish(board, Part::NetCore),
        Task::BuildNetcoreBootloader => build(&board.dual_core()?.netcore_bootloader),
        Task::Build => build_full(board),
        Task::BuildDualBank => build_dual_bank(board),
        Task::Flash => flash(board, Images::Full, Erase::Sector),
        Task::FlashChiperase => flash(board, Images::Full, Erase::Chip),
        Task::FlashRecover => flash(board, Images::Full, Erase::Recover),
        Task::FlashDualChiperase => flash(board, Images::DualBank, Erase::Chip),
        Task::FlashDualRecover => flash(board, Images::DualBank, Erase::Recover),
        Task::FlashApp => flash_app(board),
        Task::Attach => {
            if let Cores::Dual(_) = board.cores {
                eros::bail!(
                    "{} has two cores: use attach-app-a, attach-app-b, attach-net, attach-bootloader or attach-net-bootloader",
                    board.name
                );
            }
            attach(
                board,
                &["--log-format", "oneline", "--rtt-scan-memory"],
                Path::new(&output("app.elf")),
            )
        }
        Task::AttachAppA => attach_output(board, &[], "app-a.elf"),
        Task::AttachAppB => attach_output(board, &[], "app-b.elf"),
        Task::AttachNet => attach_output(board, &["--rtt-scan-memory"], "net-core.elf"),
        Task::AttachBootloader => attach_output(board, &[], "stage2-bootloader.elf"),
        Task::AttachNetBootloader => {
            let elf = board.dual_core()?.netcore_bootloader.elf();
            attach(board, &["--rtt-scan-memory"], &elf)
        }
        Task::Reset => {
            cmd::run(cmd::at(board.dir, "probe-rs").args(["reset", "--chip", board.chip]))
        }
        Task::Check => check(board),
        Task::GenRelease {
            requires_bootloader,
            requires_netcore,
            min_desktop,
        } => release::gen_release(board, requires_bootloader, requires_netcore, &min_desktop),
    }
}

fn clean(board: &Board) -> eros::Result<()> {
    remove_dir(&board.path(OUTPUT_DIR))?;
    for krate in board.clean {
        remove_dir(&cmd::root().join(krate.dir).join("target"))?;
        cmd::run(cmd::at(krate.dir, "cargo").arg("clean"))?;
    }
    Ok(())
}

fn check(board: &Board) -> eros::Result<()> {
    if board.check.is_empty() {
        eros::bail!("{} has no check", board.name);
    }
    for krate in board.check {
        cmd::run(cmd::at(krate.dir, "cargo").arg("check"))?;
    }
    Ok(())
}

fn build(krate: &Crate) -> eros::Result<()> {
    cmd::run(cmd::at(krate.dir, "cargo").arg("build").args(krate.build))
}

#[derive(Clone, Copy)]
enum Bank {
    A,
    B,
}

impl Bank {
    /// Read by the application's build script to pick the bank's memory layout.
    fn app_bank(self) -> &'static str {
        match self {
            Self::A => "bank_a",
            Self::B => "bank_b",
        }
    }

    fn target_dir(self) -> &'static str {
        match self {
            Self::A => "target-bank-a",
            Self::B => "target-bank-b",
        }
    }
}

fn build_bank(board: &Board, bank: Bank) -> eros::Result<()> {
    board.dual_core()?;
    cmd::run(
        cmd::at(board.app.dir, "cargo")
            .env("APP_BANK", bank.app_bank())
            .arg("build")
            .args(board.app.build)
            .args(["--target-dir", bank.target_dir()]),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Formats {
    Hex,
    HexAndBin,
}

/// One ELF copied into build/ as `<name>.elf` and converted next to it.
struct Image {
    name: &'static str,
    elf: PathBuf,
    formats: Formats,
}

/// The firmware set `build` (full) or `build-dual-bank` produces and flashes.
#[derive(Clone, Copy)]
enum Images {
    Full,
    /// Without the net-core bootloader.
    DualBank,
}

impl Images {
    fn merged(self) -> &'static str {
        match self {
            Self::Full => "merged.hex",
            Self::DualBank => "merged-dual.hex",
        }
    }
}

fn build_full(board: &Board) -> eros::Result<()> {
    let Cores::Dual(dual) = &board.cores else {
        build(&board.app)?;
        build(&board.bootloader)?;
        let images = [
            Image {
                name: "bootloader",
                elf: board.bootloader.elf(),
                formats: Formats::Hex,
            },
            Image {
                name: "app",
                elf: board.app.elf(),
                formats: Formats::Hex,
            },
        ];
        return package(board, &images, Images::Full.merged());
    };
    build(&dual.stage1)?;
    build_bank(board, Bank::A)?;
    build_bank(board, Bank::B)?;
    build(&board.bootloader)?;
    build(&dual.netcore_bootloader)?;
    build(&dual.net_core)?;
    let images = [
        Image {
            name: "stage1-bootloader",
            elf: dual.stage1.elf(),
            formats: Formats::Hex,
        },
        Image {
            name: "stage2-bootloader",
            elf: board.bootloader.elf(),
            formats: Formats::HexAndBin,
        },
        Image {
            name: "app-a",
            elf: board.app.elf_in(Bank::A.target_dir()),
            formats: Formats::HexAndBin,
        },
        Image {
            name: "app-b",
            elf: board.app.elf_in(Bank::B.target_dir()),
            formats: Formats::HexAndBin,
        },
        Image {
            name: "netcore-bootloader",
            elf: dual.netcore_bootloader.elf(),
            formats: Formats::Hex,
        },
        Image {
            name: "net-core",
            elf: dual.net_core.elf(),
            formats: Formats::HexAndBin,
        },
    ];
    package(board, &images, Images::Full.merged())
}

fn build_dual_bank(board: &Board) -> eros::Result<()> {
    let dual = board.dual_core()?;
    build(&dual.stage1)?;
    build_bank(board, Bank::A)?;
    build_bank(board, Bank::B)?;
    build(&board.bootloader)?;
    build(&dual.net_core)?;
    let images = [
        Image {
            name: "stage1-bootloader",
            elf: dual.stage1.elf(),
            formats: Formats::Hex,
        },
        Image {
            name: "stage2-bootloader",
            elf: board.bootloader.elf(),
            formats: Formats::Hex,
        },
        Image {
            name: "app-a",
            elf: board.app.elf_in(Bank::A.target_dir()),
            formats: Formats::HexAndBin,
        },
        Image {
            name: "app-b",
            elf: board.app.elf_in(Bank::B.target_dir()),
            formats: Formats::HexAndBin,
        },
        Image {
            name: "net-core",
            elf: dual.net_core.elf(),
            formats: Formats::HexAndBin,
        },
    ];
    package(board, &images, Images::DualBank.merged())
}

/// Copy each image into build/, convert it, and merge the hex files in `images` order.
fn package(board: &Board, images: &[Image], merged: &str) -> eros::Result<()> {
    create_dir(&board.path(OUTPUT_DIR))?;
    let tool = cmd::llvm_objcopy()?;
    let mut merge = cmd::at(board.dir, "mergehex");
    merge.arg("-o").arg(output(merged)).arg("-m");
    for image in images {
        let elf = output(&format!("{}.elf", image.name));
        let hex = output(&format!("{}.hex", image.name));
        copy(&image.elf, &board.path(&elf))?;
        objcopy(board, &tool, "ihex", Path::new(&elf), Path::new(&hex))?;
        if image.formats == Formats::HexAndBin {
            let bin = output(&format!("{}.bin", image.name));
            objcopy(board, &tool, "binary", Path::new(&elf), Path::new(&bin))?;
        }
        merge.arg(hex);
    }
    cmd::run(&mut merge)
}

/// `name` inside the output directory, relative to the board directory.
fn output(name: &str) -> String {
    format!("{OUTPUT_DIR}/{name}")
}

fn objcopy(board: &Board, tool: &Path, format: &str, elf: &Path, out: &Path) -> eros::Result<()> {
    cmd::run(
        cmd::at(board.dir, tool)
            .args(["-O", format])
            .arg(elf)
            .arg(out),
    )
}

/// Build one component and put its ELF, bin and hex into its dist directory.
fn dist(board: &Board, part: Part) -> eros::Result<()> {
    let krate = part.krate(board)?;
    build(krate)?;
    let tool = cmd::llvm_objcopy()?;
    let dist = board.path(part.dist_dir());
    remove_dir(&dist)?;
    create_dir(&dist)?;
    let elf = krate.elf();
    let stem = part.stem();
    copy(&elf, &dist.join(format!("{stem}.elf")))?;
    objcopy(
        board,
        &tool,
        "binary",
        &elf,
        &dist.join(format!("{stem}.bin")),
    )?;
    objcopy(
        board,
        &tool,
        "ihex",
        &elf,
        &dist.join(format!("{stem}.hex")),
    )?;
    for (name, size) in listing(&dist)? {
        println!("{size:>10} {name}");
    }
    Ok(())
}

/// File names and sizes in `dir`, sorted by name.
fn listing(dir: &Path) -> eros::Result<Vec<(String, u64)>> {
    let mut files = Vec::new();
    let entries = fs::read_dir(dir).with_context(|| format!("could not list {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("could not list {}", dir.display()))?;
        let size = entry
            .metadata()
            .with_context(|| format!("could not stat {}", entry.path().display()))?
            .len();
        files.push((entry.file_name().to_string_lossy().into_owned(), size));
    }
    files.sort();
    Ok(files)
}

#[derive(Clone, Copy)]
enum Erase {
    Sector,
    Chip,
    Recover,
}

impl Erase {
    fn pages(self) -> [&'static str; 2] {
        match self {
            Self::Sector => BOOT_STATE_PAGES,
            Self::Chip | Self::Recover => GUARD_PAGES,
        }
    }
}

fn flash(board: &Board, images: Images, erase: Erase) -> eros::Result<()> {
    match images {
        Images::Full => build_full(board)?,
        Images::DualBank => build_dual_bank(board)?,
    }
    if matches!(erase, Erase::Recover) {
        cmd::run(cmd::at(board.dir, "probe-rs").args([
            "erase",
            "--chip",
            board.chip,
            "--allow-erase-all",
        ]))?;
    }
    if !matches!(board.flash, Flash::Merged) {
        erase_pages(board, erase)?;
    }
    let mut command = cmd::at(board.dir, "probe-rs");
    command.args([
        "download",
        "--chip",
        board.chip,
        "--binary-format",
        "hex",
        "--verify",
        "--reset",
    ]);
    if matches!(erase, Erase::Chip) {
        command.arg("--chip-erase");
    }
    command.arg(output(images.merged()));
    cmd::run(&mut command)
}

fn erase_pages(board: &Board, erase: Erase) -> eros::Result<()> {
    let blank = output("erased-page.bin");
    fs::write(board.path(&blank), [0xff; 4096]).context("could not write erased flash page")?;
    for page in erase.pages() {
        cmd::run(
            cmd::at(board.dir, "probe-rs")
                .args([
                    "download",
                    "--chip",
                    board.chip,
                    "--binary-format",
                    "bin",
                    "--base-address",
                    page,
                    "--verify",
                ])
                .arg(&blank),
        )?;
    }
    Ok(())
}

fn flash_app(board: &Board) -> eros::Result<()> {
    build(&board.app)?;
    create_dir(&board.path(OUTPUT_DIR))?;
    let elf = output("app.elf");
    let hex = output("app.hex");
    copy(&board.app.elf(), &board.path(&elf))?;
    let tool = cmd::llvm_objcopy()?;
    objcopy(board, &tool, "ihex", Path::new(&elf), Path::new(&hex))?;
    cmd::run(
        cmd::at(board.dir, "probe-rs")
            .args([
                "download",
                "--chip",
                board.chip,
                "--binary-format",
                "hex",
                "--verify",
                "--reset",
            ])
            .arg(hex),
    )
}

/// Attach to an image in build/ of a dual-core board.
fn attach_output(board: &Board, options: &[&str], name: &str) -> eros::Result<()> {
    board.dual_core()?;
    attach(board, options, Path::new(&output(name)))
}

fn attach(board: &Board, options: &[&str], elf: &Path) -> eros::Result<()> {
    cmd::run(
        cmd::at(board.dir, "probe-rs")
            .args(["attach", "--chip", board.chip])
            .args(options)
            .arg(elf),
    )
}

fn copy(from: &Path, to: &Path) -> eros::Result<()> {
    fs::copy(from, to)
        .with_context(|| format!("could not copy {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn create_dir(dir: &Path) -> eros::Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    Ok(())
}

/// `rm -rf`: a directory that is already gone is fine.
fn remove_dir(dir: &Path) -> eros::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        result => Ok(result.with_context(|| format!("could not remove {}", dir.display()))?),
    }
}

/// Fetch `url`; an HTTP error status fails like `curl -f`.
fn download(url: &str) -> eros::Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("could not download {url}"))?;
    let data = response
        .body_mut()
        .read_to_vec()
        .with_context(|| format!("could not read {url}"))?;
    Ok(data)
}

/// What a manifest records for a file.
struct FileDigest {
    sha256: String,
    size: u64,
}

impl FileDigest {
    fn of(data: &[u8]) -> Self {
        Self {
            sha256: hex::encode(Sha256::digest(data)),
            size: data.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(board: &Board, task: Task) -> String {
        match run_task(board, task) {
            Ok(()) => panic!("{} accepted a recipe it lacks", board.name),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn single_core_board_refuses_dual_core_recipes_before_running_anything() {
        for task in [
            Task::BuildAppA,
            Task::BuildAppB,
            Task::BuildStage1,
            Task::DistBootloader,
            Task::ReleaseBootloader,
            Task::BuildNet,
            Task::DistNetcore,
            Task::ReleaseNetcore,
            Task::BuildNetcoreBootloader,
            Task::BuildDualBank,
            Task::FlashDualChiperase,
            Task::FlashDualRecover,
            Task::AttachAppA,
            Task::AttachAppB,
            Task::AttachNet,
            Task::AttachBootloader,
            Task::AttachNetBootloader,
            Task::GenRelease {
                requires_bootloader: 0,
                requires_netcore: 0,
                min_desktop: "0.6.0".to_owned(),
            },
        ] {
            assert!(refusal(&LEGACY_VM, task).contains("legacy-vm is single-core"));
        }
    }

    #[test]
    fn dual_core_boards_refuse_the_single_core_attach() {
        for board in [&LITE1, &LEGACY_ATSLITE] {
            assert!(refusal(board, Task::Attach).contains("has two cores"));
        }
    }

    #[test]
    fn only_lite1_has_check() {
        assert!(refusal(&LEGACY_ATSLITE, Task::Check).contains("legacy-atslite has no check"));
        assert!(refusal(&LEGACY_VM, Task::Check).contains("legacy-vm has no check"));
        assert!(!LITE1.check.is_empty());
    }

    #[test]
    fn file_digest_is_lowercase_hex_sha256_and_length() {
        let digest = FileDigest::of(b"abc");
        assert_eq!(
            digest.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(digest.size, 3);
    }
}
