use std::collections::BTreeSet;
use std::fs;
use std::process::{Command, Stdio};

use eros::Context as _;
use serde_json::{Value, json};

use super::board::{Board, Crate};
use super::manifest::{self, ALPHA};
use super::{FileDigest, OUTPUT_DIR, download, listing};
use crate::cmd;

/// A bootloader reports its release counter `N` as USB bcdDevice `0xF000 | N`.
const BCD_DEVICE_RELEASE_MARK: u32 = 0xF000;
const RELEASES_BASE_URL: &str = "https://github.com/odysseyarm/odyssey/releases/download";
const RELEASE_MACRO: &str = "macro_rules!release{()=>{";
const RELEASE_MACRO_END: &str = "};}";
const KEEP_NOTE: &str =
    "Installed by the desktop updater when a firmware release names this release. Do not delete.";

/// A component released on its own, by the counter in its source.
#[derive(Clone, Copy)]
pub(super) enum Part {
    Bootloader,
    NetCore,
}

impl Part {
    /// Only dual-core boards release components on their own.
    pub(super) fn krate(self, board: &Board) -> eros::Result<&Crate> {
        let dual = board.dual_core()?;
        Ok(match self {
            Self::Bootloader => &board.bootloader,
            Self::NetCore => &dual.net_core,
        })
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

    fn tag(self, device: &str, release: u32) -> String {
        let kind = match self {
            Self::Bootloader => "bootloader",
            Self::NetCore => "netcore",
        };
        format!("{device}-{kind}-v{release}")
    }

    /// The `release-*` task that publishes this component.
    fn task(self) -> &'static str {
        match self {
            Self::Bootloader => "release-bootloader",
            Self::NetCore => "release-netcore",
        }
    }

    fn title_and_notes(self, product: &str, release: u32) -> (String, String) {
        match self {
            Self::Bootloader => (
                format!("{product} bootloader release {release}"),
                format!(
                    "{product} stage2 bootloader, release {release} (bcdDevice {}). {KEEP_NOTE}",
                    bcd_device(release)
                ),
            ),
            Self::NetCore => (
                format!("{product} net-core release {release}"),
                format!(
                    "{product} net core (BLE controller) firmware, release {release}. {KEEP_NOTE}"
                ),
            ),
        }
    }
}

fn bcd_device(release: u32) -> String {
    format!("0x{:04X}", BCD_DEVICE_RELEASE_MARK | release)
}

/// The digits of the last `macro_rules! release { () => { N }; }` in `source`,
/// whatever its layout. Spaces, tabs, CR and LF are dropped before matching.
pub(super) fn counter(source: &str) -> Option<String> {
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

pub(super) fn publish(board: &Board, part: Part) -> eros::Result<()> {
    let dual = board.dual_core()?;
    super::dist(board, part)?;
    let main_rs = part.krate(board)?.main_rs();
    let release = read_counter(&main_rs)?;
    let tag = part.tag(dual.device, release);
    let changes = cmd::output(cmd::at(board.dir, "git").args([
        "status",
        "--porcelain",
        "--untracked-files=no",
    ]))?;
    if !changes.is_empty() {
        eros::bail!("uncommitted changes; the release must be built from a commit.");
    }
    let commit = cmd::output(cmd::at(board.dir, "git").args(["rev-parse", "HEAD"]))?;
    let branches =
        cmd::output(cmd::at(board.dir, "git").args(["branch", "-r", "--contains", &commit]))?;
    if !branches.contains("origin/master") {
        eros::bail!("{commit} is not on origin/master. Push it first.");
    }
    if release_exists(board, &tag)? {
        eros::bail!("release {tag} already exists. Raise the counter in {main_rs}.");
    }
    let (title, notes) = part.title_and_notes(dual.product, release);
    let dist = part.dist_dir();
    if std::env::var("DRY_RUN").as_deref() == Ok("1") {
        println!("Dry run: would publish {tag} from {commit} with:");
        for (name, _) in listing(&board.path(dist))? {
            println!("{name}");
        }
        return Ok(());
    }
    let stem = part.stem();
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

/// A component as a manifest entry pins it: its published asset and counter.
struct Published {
    digest: FileDigest,
    release: u32,
}

/// Hash the published asset rather than a local build: a release built
/// elsewhere is not bit-identical, and the manifest has to describe the bytes
/// devices will actually download.
fn published(board: &Board, part: Part, device: &str) -> eros::Result<Published> {
    let release = read_counter(&part.krate(board)?.main_rs())?;
    let tag = part.tag(device, release);
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
        release,
    })
}

pub(super) fn gen_release(
    board: &Board,
    requires_bootloader: u32,
    requires_netcore: u32,
    min_desktop: &str,
) -> eros::Result<()> {
    let dual = board.dual_core()?;
    if !is_plain_version(min_desktop) {
        eros::bail!("min_desktop must be a version like 0.6.0, got '{min_desktop}'.");
    }
    let version = cmd::package_version(board.dir)?;
    let manifest_name = format!("{}/{ALPHA}", manifest::MANIFESTS);
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
    if requires_bootloader > bootloader.release {
        eros::bail!(
            "requires_bootloader {requires_bootloader} is above the bootloader being published ({}).",
            bootloader.release
        );
    }

    let mut manifest = manifest::read(&manifest_path)?;
    let firmwares = manifest["devices"][dual.device]["firmwares"]
        .as_array()
        .with_context(|| format!("{manifest_name} lists no firmwares for {}", dual.device))?;
    check_counter(
        firmwares,
        Part::Bootloader,
        &bootloader,
        &Part::Bootloader.krate(board)?.main_rs(),
    )?;
    check_counter(
        firmwares,
        Part::NetCore,
        &net_core,
        &Part::NetCore.krate(board)?.main_rs(),
    )?;
    check_netcore_floor(firmwares, requires_netcore)?;

    let today = cmd::output(Command::new("date").arg("+%Y-%m-%d"))?;
    let entry = json!({
        "version": version,
        "requires_bootloader": requires_bootloader,
        "requires_netcore": requires_netcore,
        "min_desktop": min_desktop,
        "files": {
            "app-a": { "sha256": app_a.sha256, "size": app_a.size },
            "app-b": { "sha256": app_b.sha256, "size": app_b.size },
            "net-core": {
                "sha256": net_core.digest.sha256,
                "size": net_core.digest.size,
                "release": net_core.release,
            },
            "stage2-bootloader": {
                "sha256": bootloader.digest.sha256,
                "size": bootloader.digest.size,
                "release": bootloader.release,
            },
        },
        "released": today,
    });
    // The manifest lists newest first; check_netcore_floor reads firmwares[0].
    manifest::prepend(&mut manifest, dual.device, entry)?;
    manifest::write(&manifest_path, &manifest)?;

    println!("Added release {version} to {manifest_name}");
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

/// `MAJOR.MINOR.PATCH`, digits only.
fn is_plain_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// The counter is baked into the image, so a hash and a counter identify each
/// other. A changed image under an unraised counter would never reach devices
/// already reporting that counter; one counter on two images, or one image under
/// two counters, means the manifest and the build disagree. Entries that predate
/// the counter have no release and count as 0.
fn check_counter(
    firmwares: &[Value],
    part: Part,
    image: &Published,
    main_rs: &str,
) -> eros::Result<()> {
    let release = u64::from(image.release);
    let mut sha_releases = BTreeSet::new();
    let mut counter_on_other_image = false;
    let mut newest = 0;
    for firmware in firmwares {
        let Some(file) = firmware["files"].get(part.stem()) else {
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
    let label = match part {
        Part::Bootloader => "bootloader",
        Part::NetCore => "net-core",
    };
    if !sha_releases.is_empty() && sha_releases != BTreeSet::from([release]) {
        let listed: Vec<String> = sha_releases.iter().map(u64::to_string).collect();
        eros::bail!(
            "this {label} image is already published as release {} but its counter says {release}.",
            listed.join(",")
        );
    }
    if counter_on_other_image {
        eros::bail!(
            "release {release} is already published for a different {label} image. Raise the counter in {main_rs}."
        );
    }
    if sha_releases.is_empty() && release <= newest {
        eros::bail!(
            "the {label} image changed but its release counter ({release}) is not above the newest published ({newest})."
        );
    }
    Ok(())
}

/// requires_netcore is checked against the net core a unit is already running,
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
    fn bcd_device_marks_the_release() {
        assert_eq!(bcd_device(4), "0xF004");
        assert_eq!(bcd_device(0xFFF), "0xFFFF");
    }

    #[test]
    fn plain_versions_only() {
        assert!(is_plain_version("0.6.0"));
        assert!(is_plain_version("10.20.30"));
        for bad in ["0.6", "0.6.0.1", "v0.6.0", "0.6.0-rc1", "0..1", ""] {
            assert!(!is_plain_version(bad), "{bad}");
        }
    }

    fn bootloader_entry(sha: &str, release: Option<u64>) -> Value {
        let mut file = json!({ "sha256": sha, "size": 1 });
        if let Some(release) = release {
            file["release"] = json!(release);
        }
        json!({ "files": { "stage2-bootloader": file } })
    }

    fn image(sha: &str, release: u32) -> Published {
        Published {
            digest: FileDigest {
                sha256: sha.to_owned(),
                size: 1,
            },
            release,
        }
    }

    fn check(firmwares: &[Value], sha: &str, release: u32) -> eros::Result<()> {
        check_counter(
            firmwares,
            Part::Bootloader,
            &image(sha, release),
            "x/src/main.rs",
        )
    }

    #[test]
    fn republishing_the_same_image_under_its_counter_passes() {
        let firmwares = [
            bootloader_entry("aa", Some(4)),
            bootloader_entry("bb", Some(3)),
        ];
        assert!(check(&firmwares, "aa", 4).is_ok());
    }

    #[test]
    fn a_raised_counter_on_a_new_image_passes() {
        let firmwares = [bootloader_entry("aa", Some(4))];
        assert!(check(&firmwares, "cc", 5).is_ok());
        assert!(check(&[], "cc", 1).is_ok());
    }

    #[test]
    fn an_image_under_another_counter_is_refused() {
        let firmwares = [bootloader_entry("aa", Some(4))];
        let error = check(&firmwares, "aa", 5)
            .expect_err("image is published as 4")
            .to_string();
        assert!(
            error.contains("already published as release 4 but its counter says 5"),
            "{error}"
        );
    }

    #[test]
    fn a_counter_reused_for_a_new_image_is_refused() {
        let firmwares = [bootloader_entry("aa", Some(4))];
        let error = check(&firmwares, "cc", 4)
            .expect_err("4 belongs to aa")
            .to_string();
        assert!(
            error.contains("different bootloader image. Raise the counter in x/src/main.rs"),
            "{error}"
        );
    }

    #[test]
    fn a_changed_image_below_the_newest_counter_is_refused() {
        let firmwares = [bootloader_entry("aa", Some(4))];
        let error = check(&firmwares, "cc", 3)
            .expect_err("3 is not above 4")
            .to_string();
        assert!(
            error.contains("release counter (3) is not above the newest published (4)"),
            "{error}"
        );
    }

    #[test]
    fn entries_without_a_counter_count_as_zero() {
        let firmwares = [bootloader_entry("aa", None), json!({ "files": {} })];
        let error = check(&firmwares, "aa", 1)
            .expect_err("aa is release 0")
            .to_string();
        assert!(
            error.contains("already published as release 0 but its counter says 1"),
            "{error}"
        );
        assert!(check(&firmwares, "cc", 1).is_ok());
    }

    #[test]
    fn net_core_floor_is_bounded_by_the_newest_entry() {
        let carrying = |release: u64| json!({ "files": { "net-core": { "sha256": "aa", "size": 1, "release": release } } });
        let firmwares = [carrying(1), carrying(2)];
        assert!(check_netcore_floor(&firmwares, 1).is_ok());
        let error = check_netcore_floor(&firmwares, 2)
            .expect_err("head carries 1")
            .to_string();
        assert!(
            error.contains("above the net core carried by the newest entry (1)"),
            "{error}"
        );
        assert!(check_netcore_floor(&[], 0).is_ok());
        assert!(check_netcore_floor(&[json!({ "files": {} })], 1).is_err());
    }
}
