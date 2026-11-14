use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use eros::Context as _;
use serde_json::{Value, json};

use super::board::{Board, Crate};
use super::manifest::{self, NIGHTLY};
use super::{FileDigest, OUTPUT_DIR, download, listing};
use crate::cmd;

const RELEASES_BASE_URL: &str = "https://github.com/odysseyarm/odyssey/releases/download";
const RELEASE_MACRO: &str = "macro_rules!release{()=>{";
const RELEASE_MACRO_END: &str = "};}";
const KEEP_NOTE: &str =
    "Installed by the desktop updater when a firmware release names this release. Do not delete.";

/// A component released on its own.
#[derive(Clone, Copy)]
pub(super) enum Part {
    Bootloader,
    NetCore,
}

fn legacy_bootloader_counter(version: &semver::Version) -> Option<u16> {
    if !version.pre.is_empty() || !version.build.is_empty() {
        return None;
    }
    let triple = [
        u32::try_from(version.major).ok()?,
        u32::try_from(version.minor).ok()?,
        u32::try_from(version.patch).ok()?,
    ];
    odyssey_version::legacy_bootloader_counter(triple)
}

impl Part {
    /// Single-core marker bootloaders are factory-only. legacy-vm has no release recipe.
    pub(super) fn krate(self, board: &Board) -> eros::Result<&Crate> {
        match self {
            Self::Bootloader if board.name == "marker-control-board" => Ok(&board.bootloader),
            Self::Bootloader => {
                board.dual_core()?;
                Ok(&board.bootloader)
            }
            Self::NetCore => Ok(&board.dual_core()?.net_core),
        }
    }

    pub(super) fn dist_dir(self) -> &'static str {
        match self {
            Self::Bootloader => "dist-bootloader",
            Self::NetCore => "dist-netcore",
        }
    }

    /// The asset file name, without extension.
    pub(super) fn stem(self) -> &'static str {
        match self {
            Self::Bootloader => "stage2-bootloader",
            Self::NetCore => "net-core",
        }
    }

    pub(super) fn asset_stem(self, board: &Board) -> &'static str {
        if matches!(self, Self::Bootloader) && board.name == "marker-control-board" {
            "bootloader"
        } else {
            self.stem()
        }
    }

    fn tag(self, device: &str, version: &semver::Version) -> String {
        let (kind, label) = match self {
            Self::Bootloader => {
                let legacy = legacy_bootloader_counter(version);
                (
                    "bootloader",
                    legacy.map_or_else(|| version.to_string(), |n| n.to_string()),
                )
            }
            Self::NetCore => ("netcore", version.patch.to_string()),
        };
        format!("{device}-{kind}-v{label}")
    }

    /// The `release-*` task that publishes this component.
    fn task(self) -> &'static str {
        match self {
            Self::Bootloader => "release-bootloader",
            Self::NetCore => "release-netcore",
        }
    }

    fn title_and_notes(self, product: &str, version: &str) -> (String, String) {
        match self {
            Self::Bootloader => (
                format!("{product} bootloader {version}"),
                format!("{product} stage2 bootloader {version}. {KEEP_NOTE}"),
            ),
            Self::NetCore => (
                format!("{product} net-core release {version}"),
                format!(
                    "{product} net core (BLE controller) firmware, release {version}. {KEEP_NOTE}"
                ),
            ),
        }
    }
}

/// The digits of the last `macro_rules! release { () => { N }; }` in `source`,
/// whatever its layout. Spaces, tabs, CR and LF are dropped before matching.
pub(crate) fn counter(source: &str) -> Option<String> {
    let tokens: String = source
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
        .collect();
    let mut found = None;
    for (at, _) in tokens.match_indices(RELEASE_MACRO) {
        let rest = &tokens[at + RELEASE_MACRO.len()..];
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 && rest[digits..].starts_with(RELEASE_MACRO_END) {
            found = Some(rest[..digits].to_owned());
        }
    }
    found
}

/// The release counter of `main_rs`, relative to the repository root.
fn read_counter(main_rs: &str) -> eros::Result<u32> {
    let path = cmd::root().join(main_rs);
    let text =
        fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
    let Some(counter) = counter(&text) else {
        eros::bail!("no release counter found in {main_rs}");
    };
    let release = counter
        .parse::<u32>()
        .with_context(|| format!("release counter {counter} in {main_rs} does not fit 32 bits"))?;
    Ok(release)
}
fn version(part: Part, krate: &Crate) -> eros::Result<semver::Version> {
    match part {
        Part::Bootloader => {
            let text = cmd::package_version(krate.dir)?;
            Ok(semver::Version::parse(&text)
                .with_context(|| format!("invalid bootloader SemVer {text:?}"))?)
        }
        Part::NetCore => Ok(semver::Version::new(
            0,
            0,
            u64::from(read_counter(&krate.main_rs())?),
        )),
    }
}

pub(super) fn publish(board: &Board, part: Part) -> eros::Result<()> {
    let (device, product) = if board.name == "marker-control-board" {
        (board.name, "Marker Control Board")
    } else {
        let dual = board.dual_core()?;
        (dual.device, dual.product)
    };
    cmd::require_clean_tree(board.dir)?;
    super::dist(board, part)?;
    let krate = part.krate(board)?;
    let version = version(part, krate)?;
    if matches!(part, Part::Bootloader) && legacy_bootloader_counter(&version).is_some() {
        eros::bail!("bootloader version {version} is reserved for a historical counter release");
    }
    let tag = part.tag(device, &version);
    let commit = cmd::output(cmd::at(board.dir, "git").args(["rev-parse", "HEAD"]))?;
    cmd::require_merged_master(&commit)?;
    if release_exists(board, &tag)? {
        eros::bail!(
            "release {tag} already exists. Raise the version in {}.",
            krate.dir
        );
    }
    let label = if matches!(part, Part::NetCore) {
        version.patch.to_string()
    } else {
        version.to_string()
    };
    let (title, mut notes) = part.title_and_notes(product, &label);
    if board.name == "marker-control-board" {
        notes = format!(
            "Factory USB DFU bootloader release {version}. SWD installation only; application DFU cannot update this bootloader."
        );
    }
    let dist = part.dist_dir();
    if std::env::var("DRY_RUN").as_deref() == Ok("1") {
        println!("Dry run: would publish {tag} from {commit} with:");
        for (name, _) in listing(&board.path(dist))? {
            println!("{name}");
        }
        return Ok(());
    }
    let stem = part.asset_stem(board);
    cmd::run(
        cmd::at(board.dir, "gh")
            .args(["release", "create", &tag, "--draft", "--target", &commit])
            .args(["--title", &title, "--notes", &notes])
            .args(["bin", "hex", "elf"].map(|extension| format!("{dist}/{stem}.{extension}"))),
    )?;
    cmd::run(cmd::at(board.dir, "gh").args(["release", "edit", &tag, "--draft=false"]))?;
    println!("Published {tag}");
    Ok(())
}

fn release_exists(board: &Board, tag: &str) -> eros::Result<bool> {
    let status = cmd::at(board.dir, "gh")
        .args(["release", "view", tag])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not start `gh`")?;
    Ok(status.success())
}

struct Published {
    digest: FileDigest,
    version: semver::Version,
    release: u32,
}
/// Hash published asset: releases built elsewhere are not bit-identical.
fn published(board: &Board, part: Part, device: &str) -> eros::Result<Published> {
    let version = version(part, part.krate(board)?)?;
    let tag = part.tag(device, &version);
    let base = std::env::var("RELEASES_BASE_URL").unwrap_or_else(|_| RELEASES_BASE_URL.to_owned());
    let data = download(&format!("{base}/{tag}/{}.bin", part.stem())).with_context(|| {
        format!(
            "no published release {tag}. Run `cargo xtask firmware {} {}` first.",
            board.name,
            part.task()
        )
    })?;
    Ok(Published {
        digest: FileDigest::of(&data),
        release: if matches!(part, Part::NetCore) {
            version.patch as u32
        } else {
            0
        },
        version,
    })
}

pub(super) fn gen_release(
    board: &Board,
    requires_bootloader: &semver::Version,
    requires_netcore: u32,
    min_desktop: &str,
) -> eros::Result<()> {
    let requires_bootloader = super::with_ats_ota_bootloader_floor(
        matches!(board.name, "lite1" | "legacy-atslite"),
        requires_bootloader.clone(),
    );
    if board.name == "marker-control-board" {
        return gen_single_release(board, &requires_bootloader, requires_netcore, min_desktop);
    }
    let dual = board.dual_core()?;
    if !is_plain_version(min_desktop) {
        eros::bail!("min_desktop must be a version like 0.6.0, got '{min_desktop}'.");
    }
    let version = build_version(board.dir)?;
    let manifest_name = format!("{}/{NIGHTLY}", manifest::MANIFESTS);
    let manifest_path = cmd::root().join(&manifest_name);

    // The bootloader and net core are not among these: they are released on
    // their own and pinned from those releases below.
    for name in ["app-a.bin", "app-b.bin"] {
        if !board.path(OUTPUT_DIR).join(name).is_file() {
            eros::bail!(
                "{OUTPUT_DIR}/{name} not found. Run `cargo xtask firmware {} build` first.",
                board.name
            );
        }
    }
    let app_a = read_digest(board, "app-a.bin")?;
    let app_b = read_digest(board, "app-b.bin")?;
    let net_core = published(board, Part::NetCore, dual.device)?;
    let bootloader = published(board, Part::Bootloader, dual.device)?;
    if requires_bootloader
        .cmp_precedence(&bootloader.version)
        .is_gt()
    {
        eros::bail!(
            "requires_bootloader {requires_bootloader} is above the bootloader being published ({}).",
            bootloader.version
        );
    }

    let mut manifest = manifest::read(&manifest_path)?;
    let firmwares = manifest["devices"][dual.device]["firmwares"]
        .as_array()
        .with_context(|| format!("{manifest_name} lists no firmwares for {}", dual.device))?;
    check_bootloader_version(firmwares, &bootloader)?;
    check_counter(firmwares, &net_core, &Part::NetCore.krate(board)?.main_rs())?;
    check_netcore_floor(firmwares, requires_netcore)?;

    let today = cmd::output(Command::new("date").arg("+%Y-%m-%d"))?;
    let entry = json!({
        "version": version,
        "requires_bootloader": requires_bootloader.to_string(),
        "requires_netcore": requires_netcore,
        "min_desktop": min_desktop,
        "files": {
            "app-a": { "sha256": app_a.sha256, "size": app_a.size },
            "app-b": { "sha256": app_b.sha256, "size": app_b.size },
            "net-core": {
                "sha256": net_core.digest.sha256,
                "size": net_core.digest.size,
                "release": net_core.version.patch,
            },
            "stage2-bootloader": {
                "sha256": bootloader.digest.sha256,
                "size": bootloader.digest.size,
                "version": bootloader.version.to_string(),
            },
        },
        "released": today,
    });
    // The manifest lists newest first. check_netcore_floor reads firmwares[0].
    manifest::prepend(&mut manifest, dual.device, entry)?;
    manifest::write(&manifest_path, &manifest)?;

    println!("Added release {version} to {manifest_name}");
    println!("  requires_bootloader: {requires_bootloader}");
    println!("  app-a: {} ({} bytes)", app_a.sha256, app_a.size);
    println!("  app-b: {} ({} bytes)", app_b.sha256, app_b.size);
    println!(
        "  net-core: {} ({} bytes, release {})",
        net_core.digest.sha256, net_core.digest.size, net_core.release
    );
    println!(
        "  stage2-bootloader: {} ({} bytes)",
        bootloader.digest.sha256, bootloader.digest.size
    );
    Ok(())
}

fn read_digest(board: &Board, name: &str) -> eros::Result<FileDigest> {
    let path = board.path(OUTPUT_DIR).join(name);
    let data = fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
    Ok(FileDigest::of(&data))
}

fn gen_single_release(
    board: &Board,
    requires_bootloader: &semver::Version,
    requires_netcore: u32,
    min_desktop: &str,
) -> eros::Result<()> {
    if requires_netcore != 0 || requires_bootloader != &semver::Version::new(0, 0, 1) {
        eros::bail!(
            "marker-control-board requires bootloader 0.0.1 and requires_netcore 0; bootloader updates require SWD"
        );
    }
    if !is_plain_version(min_desktop) {
        eros::bail!("min_desktop must be a version like 0.6.0");
    }
    let version = build_version(board.dir)?;
    let base = std::env::var("RELEASES_BASE_URL").unwrap_or_else(|_| RELEASES_BASE_URL.to_owned());
    let data = download(&format!("{base}/{}-v{version}/app.bin", board.name))?;
    if data.is_empty() {
        eros::bail!("published app.bin is empty");
    }
    let app = FileDigest::of(&data);
    let today = cmd::output(Command::new("date").arg("+%Y-%m-%d"))?;
    let path = cmd::root().join(manifest::MANIFESTS).join(NIGHTLY);
    let mut manifest = manifest::read(&path)?;
    manifest::prepend(
        &mut manifest,
        board.name,
        json!({
            "version": version,
            "requires_bootloader": requires_bootloader.to_string(),
            "requires_netcore": 0,
            "min_desktop": min_desktop,
            "files": { "app": { "sha256": app.sha256, "size": app.size } },
            "released": today,
        }),
    )?;
    manifest::write(&path, &manifest)
}

/// Adds `device`'s nightly entry for a release whose images this repo only
/// announces, not installs: each file is a published asset, hashed as downloaded.
/// `assets` pairs a manifest file key with its asset name in release `tag`.
pub(crate) fn gen_asset_release(
    device: &str,
    tag: &str,
    version: &str,
    min_desktop: &str,
    assets: &[(String, String)],
) -> eros::Result<()> {
    if !is_plain_version(min_desktop) {
        eros::bail!("min_desktop must be a version like 0.6.0, got '{min_desktop}'.");
    }
    let base = std::env::var("RELEASES_BASE_URL").unwrap_or_else(|_| RELEASES_BASE_URL.to_owned());
    let mut files = serde_json::Map::new();
    for (key, asset) in assets {
        let data = download(&format!("{base}/{tag}/{asset}"))
            .with_context(|| format!("no published asset {asset} in {tag}"))?;
        if data.is_empty() {
            eros::bail!("published {asset} is empty");
        }
        let digest = FileDigest::of(&data);
        files.insert(
            key.clone(),
            json!({ "sha256": digest.sha256, "size": digest.size }),
        );
    }
    let today = cmd::output(Command::new("date").arg("+%Y-%m-%d"))?;
    let path = cmd::root().join(manifest::MANIFESTS).join(NIGHTLY);
    let mut manifest = manifest::read(&path)?;
    manifest::prepend(
        &mut manifest,
        device,
        json!({
            "version": version,
            "requires_bootloader": "0.0.0",
            "requires_netcore": 0,
            "min_desktop": min_desktop,
            "files": files,
            "released": today,
        }),
    )?;
    manifest::write(&path, &manifest)
}

fn build_version(dir: impl AsRef<Path>) -> eros::Result<String> {
    let version = match std::env::var("ODYSSEY_BUILD_VERSION") {
        Ok(version) => version,
        Err(std::env::VarError::NotPresent) => cmd::package_version(dir)?,
        Err(error) => return Err(error.into()),
    };
    validate_build_version(&version)?;
    Ok(version)
}

fn validate_build_version(version: &str) -> eros::Result<()> {
    if let Err(error) = semver::Version::parse(version) {
        eros::bail!("invalid firmware build version {version:?}: {error}");
    }
    Ok(())
}

/// `MAJOR.MINOR.PATCH`, digits only.
fn is_plain_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn check_bootloader_version(firmwares: &[Value], image: &Published) -> eros::Result<()> {
    let image_version = image.version.to_string();
    let mut newest: Option<semver::Version> = None;
    for file in firmwares
        .iter()
        .filter_map(|entry| entry["files"].get("stage2-bootloader"))
    {
        let Some(text) = file["version"].as_str() else {
            continue;
        };
        let old = semver::Version::parse(text)
            .with_context(|| format!("invalid stage2 bootloader version {text:?}"))?;
        let same_image = file["sha256"].as_str() == Some(image.digest.sha256.as_str());
        if same_image && old != image.version {
            eros::bail!(
                "this bootloader image is already published as version {old} but Cargo.toml says {}.",
                image.version
            );
        }
        if !same_image && old == image.version {
            eros::bail!(
                "bootloader version {} is already published for a different image. Raise Cargo.toml version.",
                image.version
            );
        }
        if newest
            .as_ref()
            .is_none_or(|known| old.cmp_precedence(known).is_gt())
        {
            newest = Some(old);
        }
    }
    if let Some(newest) = newest
        && image.version.cmp_precedence(&newest).is_le()
        && !firmwares.iter().any(|entry| {
            entry["files"]["stage2-bootloader"]["sha256"].as_str()
                == Some(image.digest.sha256.as_str())
                && entry["files"]["stage2-bootloader"]["version"].as_str()
                    == Some(image_version.as_str())
        })
    {
        eros::bail!(
            "bootloader image changed but Cargo.toml version ({}) is not above newest published ({newest}).",
            image.version
        );
    }
    Ok(())
}

/// Net-core counter/hash identity must remain unique.
fn check_counter(firmwares: &[Value], image: &Published, main_rs: &str) -> eros::Result<()> {
    let release = u64::from(image.release);
    let mut sha_releases = BTreeSet::new();
    let mut counter_on_other_image = false;
    let mut newest = 0;
    for firmware in firmwares {
        let Some(file) = firmware["files"].get("net-core") else {
            continue;
        };
        let file_release = manifest::release_of(file);
        newest = newest.max(file_release);
        if file["sha256"] == image.digest.sha256.as_str() {
            sha_releases.insert(file_release);
        } else if file["release"].as_u64() == Some(release) {
            counter_on_other_image = true;
        }
    }
    if !sha_releases.is_empty() && sha_releases != BTreeSet::from([release]) {
        let listed: Vec<String> = sha_releases.iter().map(u64::to_string).collect();
        eros::bail!(
            "this net-core image is already published as release {} but its counter says {release}.",
            listed.join(",")
        );
    }
    if counter_on_other_image {
        eros::bail!(
            "release {release} is already published for a different net-core image. Raise the counter in {main_rs}."
        );
    }
    if sha_releases.is_empty() && release <= newest {
        eros::bail!(
            "the net-core image changed but its release counter ({release}) is not above the newest published ({newest})."
        );
    }
    Ok(())
}

/// `requires_netcore` is checked against the net core a unit is already running,
/// and nothing reads the net core back until the new application runs, so the net
/// core this entry carries can never satisfy its own floor. Selection is greedy:
/// units take the newest entry they are allowed, so a floor may only name a
/// release the current newest entry delivers. A release carried only by an older
/// entry gets overtaken and never reaches units.
fn check_netcore_floor(firmwares: &[Value], requires_netcore: u32) -> eros::Result<()> {
    let head = firmwares
        .first()
        .map_or(0, |entry| manifest::release_of(&entry["files"]["net-core"]));
    if u64::from(requires_netcore) > head {
        eros::bail!(
            "requires_netcore {requires_netcore} is above the net core carried by the newest entry ({head}).\n\
             A net-core bump ships in a bridge entry that requires only what is deployed; \
             the bridge has to be the newest entry when a later one requires the new release."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_reads_the_rustfmt_layout() {
        let source = "/// doc\nmacro_rules! release {\n    () => {\n        4\n    };\n}\n";
        assert_eq!(counter(source).as_deref(), Some("4"));
    }

    #[test]
    fn counter_ignores_layout_tabs_and_crlf() {
        assert_eq!(
            counter("macro_rules!release{()=>{12};}").as_deref(),
            Some("12")
        );
        assert_eq!(
            counter("macro_rules ! release\t{ ( ) =>\r\n{ 7 } ; }").as_deref(),
            Some("7")
        );
    }

    #[test]
    fn counter_is_absent_without_a_numeric_literal() {
        assert_eq!(counter("fn main() {}"), None);
        assert_eq!(counter("macro_rules! release { () => { RELEASE }; }"), None);
        assert_eq!(counter("macro_rules! release { () => { }; }"), None);
        assert_eq!(counter("macro_rules! release { () => { 3 } }"), None);
        assert_eq!(counter("macro_rules! release { () => { 3u16 }; }"), None);
    }

    #[test]
    fn counter_takes_the_last_declaration_and_keeps_the_digits_verbatim() {
        let source =
            "macro_rules! release { () => { 1 }; }\nmacro_rules! release { () => { 007 }; }";
        assert_eq!(counter(source).as_deref(), Some("007"));
    }

    #[test]
    fn plain_versions_only() {
        assert!(is_plain_version("0.6.0"));
        assert!(is_plain_version("10.20.30"));
        for bad in ["0.6", "0.6.0.1", "v0.6.0", "0.6.0-rc1", "0..1", ""] {
            assert!(!is_plain_version(bad), "{bad}");
        }
    }

    #[test]
    fn firmware_build_versions_accept_semver_nightly_identity() {
        validate_build_version("1.2.3").expect("plain semver is accepted");
        validate_build_version("1.2.4-nightly.20261111.t201500.gabcdef012345")
            .expect("nightly identity is accepted");
        for bad in ["1.2", "v1.2.3", "1.2.3-nightly.20261111.020000.gabcdef"] {
            validate_build_version(bad).expect_err("invalid build version is rejected");
        }
    }

    fn bootloader_entry(sha: &str, version: Option<&str>) -> Value {
        let mut file = json!({ "sha256": sha, "size": 1 });
        if let Some(version) = version {
            file["version"] = json!(version);
        }
        json!({ "files": { "stage2-bootloader": file } })
    }

    fn image(sha: &str, version: &str) -> Published {
        Published {
            digest: FileDigest {
                sha256: sha.to_owned(),
                size: 1,
            },
            version: semver::Version::parse(version).unwrap(),
            release: 0,
        }
    }

    #[test]
    fn bootloader_identity_uses_semver_and_ignores_unversioned_history() {
        let firmwares = [
            bootloader_entry("old", None),
            bootloader_entry("aa", Some("0.2.1")),
        ];
        check_bootloader_version(&firmwares, &image("aa", "0.2.1"))
            .expect("same image/version accepted");
        check_bootloader_version(&firmwares, &image("cc", "0.2.2"))
            .expect("higher version accepted");
        assert!(
            check_bootloader_version(&firmwares, &image("aa", "0.2.2")).is_err(),
            "image cannot be relabeled"
        );
        assert!(
            check_bootloader_version(&firmwares, &image("cc", "0.2.1")).is_err(),
            "version cannot be reused for another image"
        );
    }

    #[test]
    fn bootloader_changed_image_must_advance_semver() {
        let firmwares = [bootloader_entry("aa", Some("1.2.3"))];
        assert!(check_bootloader_version(&firmwares, &image("bb", "1.2.2")).is_err());
        check_bootloader_version(&firmwares, &image("bb", "1.2.3+build.2"))
            .expect_err("build metadata does not advance release precedence");
        check_bootloader_version(&firmwares, &image("bb", "1.2.4"))
            .expect("new image with a higher version is publishable");
    }

    #[test]
    fn net_core_floor_is_bounded_by_the_newest_entry() {
        let carrying = |release: u64| json!({ "files": { "net-core": { "sha256": "aa", "size": 1, "release": release } } });
        let firmwares = [carrying(1), carrying(2)];
        check_netcore_floor(&firmwares, 1).expect("floor matches newest carried net core");
        let error = check_netcore_floor(&firmwares, 2)
            .expect_err("head carries 1")
            .to_string();
        assert!(
            error.contains("above the net core carried by the newest entry (1)"),
            "{error}"
        );
        check_netcore_floor(&[], 0).expect("empty history has no net-core floor");
        check_netcore_floor(&[json!({ "files": {} })], 1)
            .expect_err("missing net core is rejected");
    }
}
