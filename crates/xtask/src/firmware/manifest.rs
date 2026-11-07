use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use eros::Context as _;
use serde_json::Value;

use super::{FileDigest, create_dir, download};
use crate::cmd;

/// Relative to the repository root.
pub(super) const MANIFESTS: &str = "firmware/manifests";
pub(super) const ALPHA: &str = "releases.alpha.json";
const STABLE: &str = "releases.stable.json";
const PACK_README: &str = "Odyssey offline firmware pack.\n\n\
Put this folder where the 'firmware_pack_dir' policy points (by default\n\
%ProgramData%\\OdysseyArm\\Odyssey\\firmware-pack) and restart Odyssey. Keep the layout\n\
as it is; Odyssey only offers a firmware release when every file it lists is here.\n\
SHA256SUMS lists the checksum of every firmware file.\n";

pub(super) fn read(path: &Path) -> eros::Result<Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    let manifest = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    Ok(manifest)
}

/// Two-space indented with a trailing newline, the layout jq wrote; replaced
/// whole so an interrupted write leaves the old manifest.
pub(super) fn write(path: &Path, manifest: &Value) -> eros::Result<()> {
    let mut text =
        serde_json::to_string_pretty(manifest).context("could not serialize the manifest")?;
    text.push('\n');
    let staging = path.with_extension("json.tmp");
    fs::write(&staging, text).with_context(|| format!("could not write {}", staging.display()))?;
    fs::rename(&staging, path).with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

/// Put `entry` first in the device's firmwares, creating the list if needed.
pub(super) fn prepend(manifest: &mut Value, device: &str, entry: Value) -> eros::Result<()> {
    if matches!(device, "marker-control-board" | "dongle")
        && let Some(head) = entries(manifest, device).first()
        && version_cmp(
            entry["version"].as_str().unwrap_or_default(),
            head["version"].as_str().unwrap_or_default(),
        ) != Ordering::Greater
    {
        eros::bail!("{device} release must be newer than current head");
    }
    match &mut manifest["devices"][device]["firmwares"] {
        Value::Array(firmwares) => firmwares.insert(0, entry),
        firmwares @ Value::Null => *firmwares = Value::Array(vec![entry]),
        _ => eros::bail!("firmwares of {device} is not a list"),
    }
    Ok(())
}

/// A manifest file's release counter; files that predate the counter count as 0.
pub(super) fn release_of(file: &Value) -> u64 {
    file.get("release").and_then(Value::as_u64).unwrap_or(0)
}

/// The device's entries, newest first; none when the device is not listed.
fn entries<'a>(manifest: &'a Value, device: &str) -> &'a [Value] {
    entries_of(&manifest["devices"][device])
}

pub(super) fn promote(device: &str, version: &str) -> eros::Result<()> {
    let dir = cmd::root().join(MANIFESTS);
    let alpha = read(&dir.join(ALPHA))?;
    let stable_path = dir.join(STABLE);
    let mut stable = read(&stable_path)?;
    promote_entry(&alpha, &mut stable, device, version)?;
    write(&stable_path, &stable)?;
    println!("Promoted {device} {version} to stable");
    Ok(())
}

/// Copy `device` `version` from `alpha` to the head of `stable`. Every refusal
/// comes before `stable` changes.
fn promote_entry(
    alpha: &Value,
    stable: &mut Value,
    device: &str,
    version: &str,
) -> eros::Result<()> {
    let Some(entry) = entries(alpha, device)
        .iter()
        .find(|entry| entry["version"] == version)
    else {
        eros::bail!("{device} {version} is not in {ALPHA}");
    };
    let promoted = entries(stable, device);
    if promoted.iter().any(|entry| entry["version"] == version) {
        eros::bail!("{device} {version} is already in {STABLE}");
    }
    // Hosts take the first installable entry, so stable has to stay newest-first.
    // Prepending anything that is not newer than the head would break that.
    let head = promoted.first();
    if let Some(head_version) = head.and_then(|head| head["version"].as_str())
        && version_cmp(version, head_version) == Ordering::Less
    {
        eros::bail!(
            "{version} is not newer than {head_version}, stable's current newest for {device}; promote in order."
        );
    }
    // A stable unit installs stable's head, so the candidate's net-core floor has
    // to be one that head delivers. Promoting the entry that requires a net core
    // before the bridge carrying it would publish a head no stable unit can ever
    // install.
    let required = entry["requires_netcore"].as_u64().unwrap_or(0);
    let carried = head.map_or(0, |head| release_of(&head["files"]["net-core"]));
    if required > carried {
        eros::bail!(
            "{version} requires net core release {required} but stable's newest entry for {device} carries {carried}; \
             promote the bridge entry that carries release {required} first."
        );
    }
    prepend(stable, device, entry.clone())
}

/// Orders versions like `sort -V`: digit runs compare as numbers, other runs as
/// text, and a version that is a prefix of another sorts first.
/// shortcut: none of sort -V's `~` or file-suffix rules; upgrade if versions grow them.
fn version_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a, b);
    loop {
        let (a_run, a_rest) = split_run(a);
        let (b_run, b_rest) = split_run(b);
        let order = match (a_run.is_empty(), b_run.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => run_cmp(a_run, b_run),
        };
        if order != Ordering::Equal {
            return order;
        }
        (a, b) = (a_rest, b_rest);
    }
}

/// The leading run of all-digit or all-non-digit characters, and the rest.
fn split_run(text: &str) -> (&str, &str) {
    let digits = text.starts_with(|c: char| c.is_ascii_digit());
    let end = text
        .find(|c: char| c.is_ascii_digit() != digits)
        .unwrap_or(text.len());
    text.split_at(end)
}

fn run_cmp(a: &str, b: &str) -> Ordering {
    let numeric = |run: &str| run.bytes().all(|b| b.is_ascii_digit());
    if !(numeric(a) && numeric(b)) {
        return a.cmp(b);
    }
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Channel {
    Stable,
    Alpha,
}

impl Channel {
    fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Alpha => "alpha",
        }
    }
}

#[derive(clap::Args)]
pub(crate) struct PackArgs {
    /// The folder to write the pack into.
    out: PathBuf,
    /// A channel to include; repeat for both [default: stable].
    #[arg(long = "channel", value_enum)]
    channels: Vec<Channel>,
    /// Only this device; repeat for more [default: all].
    #[arg(long = "device")]
    devices: Vec<String>,
    /// Newest releases per device.
    #[arg(long, default_value_t = 2)]
    latest: usize,
    /// The directory holding releases.*.json [default: firmware/manifests].
    #[arg(long)]
    manifests: Option<PathBuf>,
    /// The GitHub repository the releases live in.
    #[arg(long, default_value = "odysseyarm/odyssey")]
    repo: String,
}

pub(super) fn build_pack(args: &PackArgs) -> eros::Result<()> {
    let manifests = args
        .manifests
        .clone()
        .unwrap_or_else(|| cmd::root().join(MANIFESTS));
    let channels = if args.channels.is_empty() {
        &[Channel::Stable][..]
    } else {
        &args.channels[..]
    };
    let out = &args.out;
    create_dir(out)?;
    // Relative path -> sha256, so a file shared by releases is fetched once.
    let mut have = BTreeMap::new();
    for channel in channels {
        let mut source = read(&manifests.join(format!("releases.{}.json", channel.name())))?;
        select(&mut source, channel.name(), &args.devices, args.latest)?;
        for (device, entry, key, file) in files(&source) {
            let Some(tag) = tag_for(device, entry, key, file) else {
                continue;
            };
            let relative = format!("{tag}/{key}.bin");
            if have.contains_key(&relative) {
                continue;
            }
            let data = download(&format!(
                "https://github.com/{}/releases/download/{relative}",
                args.repo
            ))?;
            let digest = FileDigest::of(&data);
            let expected_sha = file["sha256"].as_str().unwrap_or_default();
            if digest.sha256 != expected_sha || Some(digest.size) != file["size"].as_u64() {
                eros::bail!(
                    "{relative}: expected {} bytes / {expected_sha}, got {} bytes / {}",
                    file["size"],
                    digest.size,
                    digest.sha256
                );
            }
            let target = out.join(&relative);
            if let Some(parent) = target.parent() {
                create_dir(parent)?;
            }
            fs::write(&target, &data)
                .with_context(|| format!("could not write {}", target.display()))?;
            println!("{relative}  ({} bytes)", digest.size);
            have.insert(relative, digest.sha256);
        }
        write(
            &out.join(format!("releases.{}.json", channel.name())),
            &source,
        )?;

        // The same check Odyssey does when it loads the pack.
        for (device, entry, key, file) in files(&source) {
            let Some(tag) = tag_for(device, entry, key, file) else {
                continue;
            };
            let path = out.join(&tag).join(format!("{key}.bin"));
            let size = fs::metadata(&path)
                .ok()
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len());
            if size.is_none() || size != file["size"].as_u64() {
                eros::bail!(
                    "incomplete pack: {} is missing or the wrong size",
                    path.display()
                );
            }
        }
    }
    let mut sums = String::new();
    for (relative, digest) in &have {
        sums.push_str(&format!("{digest}  {relative}\n"));
    }
    let sums_path = out.join("SHA256SUMS");
    fs::write(&sums_path, sums)
        .with_context(|| format!("could not write {}", sums_path.display()))?;
    let readme = out.join("README.txt");
    fs::write(&readme, PACK_README)
        .with_context(|| format!("could not write {}", readme.display()))?;
    println!(
        "pack written to {}: {} firmware files",
        out.display(),
        have.len()
    );
    Ok(())
}

/// Keep the listed devices (all when none is listed) and their newest `latest`
/// entries, and refuse an entry missing an application slot.
fn select(
    source: &mut Value,
    channel: &str,
    devices: &[String],
    latest: usize,
) -> eros::Result<()> {
    let Some(listed) = source["devices"].as_object_mut() else {
        eros::bail!("the {channel} manifest has no devices");
    };
    if !devices.is_empty() {
        listed.retain(|device, _| devices.contains(device));
    }
    if listed.is_empty() {
        eros::bail!("no device in the {channel} manifest matches {devices:?}");
    }
    for (device, info) in listed.iter_mut() {
        let Some(firmwares) = info["firmwares"].as_array_mut() else {
            eros::bail!("{device} in the {channel} manifest has no firmwares list");
        };
        firmwares.truncate(latest);
        for entry in firmwares.iter() {
            validate_layout(entry).with_context(|| {
                format!("{device} {}", entry["version"].as_str().unwrap_or_default())
            })?;
        }
    }
    Ok(())
}

fn validate_layout(entry: &Value) -> eros::Result<()> {
    let files = &entry["files"];
    let single = files.get("app").is_some();
    let a = files.get("app-a").is_some();
    let b = files.get("app-b").is_some();
    if single {
        if a || b
            || files.get("net-core").is_some()
            || files.get("stage2-bootloader").is_some()
            || entry["requires_netcore"].as_u64() != Some(0)
        {
            eros::bail!(
                "single app layout has bank/core/bootloader files or nonzero requires_netcore"
            );
        }
    } else if !a || !b {
        let missing = if !a { "app-a" } else { "app-b" };
        eros::bail!("lists no {missing}");
    }
    Ok(())
}

/// Every (device, entry, file key, file) in the manifest, files in key order.
fn files(source: &Value) -> Vec<(&str, &Value, &str, &Value)> {
    let mut listed = Vec::new();
    let Some(devices) = source["devices"].as_object() else {
        return listed;
    };
    for (device, info) in devices {
        for entry in entries_of(info) {
            let Some(files) = entry["files"].as_object() else {
                continue;
            };
            let mut sorted: Vec<(&String, &Value)> = files.iter().collect();
            sorted.sort_by_key(|(key, _)| *key);
            for (key, file) in sorted {
                listed.push((device.as_str(), entry, key.as_str(), file));
            }
        }
    }
    listed
}

/// A device's entries, newest first; none when it lists no firmwares.
fn entries_of(info: &Value) -> &[Value] {
    info["firmwares"].as_array().map_or(&[][..], Vec::as_slice)
}

/// Which GitHub release holds a manifest file. Mirrors `ReleaseSource::tag` in
/// odyssey-hub-server.
///
/// `None` for a net core or bootloader listed without a release number: older
/// entries list those for the record only, and the updater never downloads them.
fn tag_for(device: &str, entry: &Value, key: &str, file: &Value) -> Option<String> {
    let kind = match key {
        "net-core" => "netcore",
        "stage2-bootloader" => "bootloader",
        _ => {
            return Some(format!(
                "{device}-v{}",
                entry["version"].as_str().unwrap_or_default()
            ));
        }
    };
    let release = file.get("release")?;
    Some(format!("{device}-{kind}-v{release}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// An alpha entry: `carried` is the net-core release it ships, if any.
    fn entry(version: &str, requires_netcore: u64, carried: Option<u64>) -> Value {
        let mut files = json!({ "app-a": { "sha256": "00", "size": 1 } });
        if let Some(release) = carried {
            files["net-core"] = json!({ "sha256": "00", "size": 1, "release": release });
        }
        json!({
            "version": version,
            "requires_bootloader": 0,
            "requires_netcore": requires_netcore,
            "min_desktop": "0.6.0",
            "files": files,
        })
    }

    fn manifest(entries: Vec<Value>) -> Value {
        json!({ "version": 1, "devices": { "atslite1": { "firmwares": entries } } })
    }

    /// Alpha holds the bridge chain: 0.2.1 carries net core 2, 0.3.0 requires it.
    fn alpha() -> Value {
        manifest(vec![
            entry("0.3.0", 2, Some(2)),
            entry("0.2.1", 0, Some(2)),
            entry("0.2.0", 0, Some(1)),
            entry("0.1.0", 0, None),
        ])
    }

    fn refused(stable: Vec<Value>, version: &str) -> String {
        let mut stable = manifest(stable);
        let before = stable.clone();
        let error = promote_entry(&alpha(), &mut stable, "atslite1", version)
            .expect_err("expected a refusal");
        assert_eq!(stable, before, "stable changed despite the refusal");
        error.to_string()
    }

    fn versions(stable: &Value) -> Vec<&str> {
        entries(stable, "atslite1")
            .iter()
            .filter_map(|entry| entry["version"].as_str())
            .collect()
    }

    #[test]
    fn post_bridge_entry_before_its_bridge_is_refused() {
        let error = refused(vec![entry("0.1.0", 0, None)], "0.3.0");
        assert!(error.contains("promote the bridge entry"), "{error}");
    }

    #[test]
    fn the_bridge_then_the_entry_requiring_it_promote_in_order() {
        let mut stable = manifest(vec![entry("0.1.0", 0, None)]);
        promote_entry(&alpha(), &mut stable, "atslite1", "0.2.1").expect("the bridge promotes");
        promote_entry(&alpha(), &mut stable, "atslite1", "0.3.0")
            .expect("the entry requiring the bridge's release promotes");
        assert_eq!(versions(&stable), ["0.3.0", "0.2.1", "0.1.0"]);
        assert_eq!(
            entries(&stable, "atslite1")[0],
            entries(&alpha(), "atslite1")[0]
        );
    }

    #[test]
    fn a_floor_above_the_heads_net_core_is_refused() {
        // Judged against stable's head, not against anything in alpha.
        let error = refused(
            vec![entry("0.2.0", 0, Some(1)), entry("0.1.0", 0, None)],
            "0.3.0",
        );
        assert!(error.contains("carries 1"), "{error}");
    }

    #[test]
    fn an_older_version_than_the_head_is_refused_before_the_floor() {
        let error = refused(
            vec![entry("0.2.1", 0, Some(2)), entry("0.1.0", 0, None)],
            "0.2.0",
        );
        assert!(error.contains("not newer"), "{error}");
    }

    #[test]
    fn unknown_and_duplicate_versions_are_refused() {
        let error = refused(vec![], "9.9.9");
        assert!(
            error.contains("atslite1 9.9.9 is not in releases.alpha.json"),
            "{error}"
        );
        let error = refused(vec![entry("0.1.0", 0, None)], "0.1.0");
        assert!(error.contains("already in releases.stable.json"), "{error}");
    }

    #[test]
    fn a_device_new_to_stable_gets_its_first_entry() {
        let mut stable = json!({ "version": 1, "devices": {} });
        promote_entry(&alpha(), &mut stable, "atslite1", "0.1.0")
            .expect("empty stable takes any entry");
        assert_eq!(versions(&stable), ["0.1.0"]);
    }

    #[test]
    fn version_cmp_orders_like_sort_v() {
        assert_eq!(version_cmp("0.2.10", "0.2.9"), Ordering::Greater);
        assert_eq!(version_cmp("0.3.0", "0.2.1"), Ordering::Greater);
        assert_eq!(version_cmp("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(version_cmp("1.0.0", "1.0.0-rc1"), Ordering::Less);
        assert_eq!(version_cmp("0.10.0", "0.9.99"), Ordering::Greater);
        assert_eq!(version_cmp("01.0", "1.0"), Ordering::Equal);
    }

    #[test]
    fn tags_follow_release_source() {
        let entry = json!({ "version": "0.1.22" });
        assert_eq!(
            tag_for("atslite1", &entry, "app-a", &json!({})).as_deref(),
            Some("atslite1-v0.1.22")
        );
        assert_eq!(
            tag_for("atslite1", &entry, "net-core", &json!({ "release": 2 })).as_deref(),
            Some("atslite1-netcore-v2")
        );
        assert_eq!(
            tag_for(
                "legacy-atslite1",
                &entry,
                "stage2-bootloader",
                &json!({ "release": 4 })
            )
            .as_deref(),
            Some("legacy-atslite1-bootloader-v4")
        );
        assert_eq!(
            tag_for("atslite1", &entry, "stage2-bootloader", &json!({})),
            None
        );
    }

    fn pack_source() -> Value {
        let full =
            |version: &str| json!({ "version": version, "files": { "app-a": {}, "app-b": {} } });
        json!({ "version": 1, "devices": {
            "atslite1": { "firmwares": [full("0.3.0"), full("0.2.0"), full("0.1.0")] },
            "legacy-atslite1": { "firmwares": [full("0.1.22")] },
        }})
    }

    #[test]
    fn select_keeps_the_newest_entries_of_the_listed_devices() {
        let mut source = pack_source();
        select(&mut source, "stable", &["atslite1".to_owned()], 2).expect("atslite1 is listed");
        assert_eq!(versions(&source), ["0.3.0", "0.2.0"]);
        assert!(source["devices"].get("legacy-atslite1").is_none());

        let mut source = pack_source();
        select(&mut source, "stable", &[], 2).expect("all devices");
        assert_eq!(entries(&source, "legacy-atslite1").len(), 1);
    }

    #[test]
    fn select_refuses_no_matching_device_and_a_missing_app_slot() {
        assert!(select(&mut pack_source(), "alpha", &["vm".to_owned()], 2).is_err());

        let mut source = pack_source();
        source["devices"]["atslite1"]["firmwares"][1]["files"]
            .as_object_mut()
            .expect("files is an object")
            .remove("app-b");
        assert!(select(&mut source, "stable", &[], 2).is_err());
        assert!(
            select(&mut source, "stable", &[], 1).is_ok(),
            "0.2.0 is beyond --latest 1"
        );
    }

    #[test]
    fn pack_accepts_single_or_dual_but_not_mixed_layouts() {
        assert!(validate_layout(&json!({"files": {"app": {}}, "requires_netcore": 0})).is_ok());
        assert!(validate_layout(&json!({"files": {"app-a": {}, "app-b": {}}})).is_ok());
        for entry in [
            json!({"files": {"app": {}, "app-a": {}}, "requires_netcore": 0}),
            json!({"files": {"app": {}, "net-core": {}}, "requires_netcore": 0}),
            json!({"files": {"app": {}, "stage2-bootloader": {}}, "requires_netcore": 0}),
            json!({"files": {"app": {}}, "requires_netcore": 1}),
            json!({"files": {"app-a": {}}}),
            json!({"files": {}}),
        ] {
            assert!(validate_layout(&entry).is_err(), "{entry}");
        }
    }

    #[test]
    fn marker_alpha_prepend_preserves_newest_first_without_mutating_on_refusal() {
        let mut source = json!({"devices": {}});
        prepend(
            &mut source,
            "marker-control-board",
            json!({"version": "0.2.0"}),
        )
        .unwrap();
        let before = source.clone();
        for version in ["0.1.0", "0.2.0"] {
            assert!(
                prepend(
                    &mut source,
                    "marker-control-board",
                    json!({"version": version})
                )
                .is_err()
            );
            assert_eq!(source, before);
        }
        prepend(
            &mut source,
            "marker-control-board",
            json!({"version": "0.3.0"}),
        )
        .unwrap();
        assert_eq!(
            source["devices"]["marker-control-board"]["firmwares"][0]["version"],
            "0.3.0"
        );
        assert_eq!(
            source["devices"]["marker-control-board"]["firmwares"][1]["version"],
            "0.2.0"
        );
    }

    #[test]
    fn dongle_prepend_refuses_a_version_that_is_not_newer() {
        let mut source = json!({"devices": {}});
        prepend(&mut source, "dongle", json!({"version": "0.2.0"})).unwrap();
        let before = source.clone();
        for version in ["0.1.9", "0.2.0"] {
            assert!(prepend(&mut source, "dongle", json!({"version": version})).is_err());
            assert_eq!(source, before, "{version}");
        }
        prepend(&mut source, "dongle", json!({"version": "0.10.0"})).unwrap();
        assert_eq!(
            source["devices"]["dongle"]["firmwares"][0]["version"],
            "0.10.0"
        );
    }
}
