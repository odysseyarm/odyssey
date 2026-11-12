//! `cargo xtask release`: which components changed since their last release
//! tag, the bumps that follow, and publishing what was bumped.
//!
//! A component changed when a file in its crate, in a path dependency of it, in
//! its lock file or in its memory map differs between its newest tag and HEAD.
//! That errs toward bumping: a comment-only edit in a shared crate counts.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use eros::Context as _;
use pure_release::{COMPONENT_COUNT, Kind, Link, Refusal, State};
use serde_json::Value;

use crate::firmware::{self, Task};
use crate::{cmd, dongle};

const DESKTOP: &str = "odyssey-desktop";
const RELEASE_POLL: Duration = Duration::from_secs(20);
/// The slowest workflow a later step waits on is a firmware Nix build from a cold cache.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(45 * 60);

#[derive(clap::Subcommand)]
pub(crate) enum Cmd {
    /// List every released component: its newest tag, the version in the source,
    /// what changed since, and the Desktop floor its manifest entry would get.
    Plan,
    /// Raise the version or release counter of each component that changed since
    /// its tag, and of each application that delivers a raised net core.
    ///
    /// Without names, every changed component except bootloaders. A bootloader is
    /// raised only by name: releasing one reflashes every unit's bootloader.
    /// Cargo versions are raised by cargo-release; review minor or major changes separately.
    Bump { components: Vec<String> },
    /// Publish each component whose source is ahead of its tag: net cores and
    /// bootloaders first, then the applications that deliver them, then the rest.
    ///
    /// Needs a clean tree on a commit merged into master. Firmware entries
    /// are added to the nightly manifest in the working tree; merge them through a PR.
    Publish {
        components: Vec<String>,
        /// Print what would be published and stop.
        #[arg(long)]
        dry_run: bool,
    },
}

pub(crate) fn run(command: Cmd) -> eros::Result<()> {
    match command {
        Cmd::Plan => print(&plan()?),
        Cmd::Bump { components } => bump(&components),
        Cmd::Publish {
            components,
            dry_run,
        } => publish(&components, dry_run),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AtsLite {
    Lite1,
    Legacy,
}

impl AtsLite {
    fn cmd(self, task: Task) -> firmware::Cmd {
        match self {
            Self::Lite1 => firmware::Cmd::Lite1(task),
            Self::Legacy => firmware::Cmd::LegacyAtslite(task),
        }
    }

    /// The product name in release notes.
    fn product(self) -> &'static str {
        match self {
            Self::Lite1 => "Lite 1",
            Self::Legacy => "Legacy ATS Lite",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    /// Pushing the tag is the release: a workflow builds and publishes.
    Workflow,
    /// A workflow publishes the packages; the nightly manifest entry is added here.
    Dongle,
    /// Built and published here, with its nightly manifest entry.
    App(AtsLite),
    /// A net core or bootloader: released by counter and delivered as `file` of
    /// `app`'s manifest entries.
    Part {
        app: &'static str,
        file: &'static str,
    },
}

/// A wire-format version constant in odyssey-protocol that a Desktop has to reach.
#[derive(Clone, Copy)]
struct Wire {
    file: &'static str,
    constant: &'static str,
}

const DEVICE_WIRE: Wire = Wire {
    file: "crates/odyssey-protocol/src/version.rs",
    constant: "DEVICE_PROTOCOL",
};
const MUX_WIRE: Wire = Wire {
    file: "crates/odyssey-protocol/src/mux.rs",
    constant: "MUX_PROTOCOL",
};

struct Component {
    /// Releases are tagged `<name>-v<version>`; firmware uses it as its manifest device key.
    name: &'static str,
    dir: &'static str,
    /// Files outside the crate and its path dependencies that shape the image.
    extra: &'static [&'static str],
    role: Role,
    wire: Option<Wire>,
    /// Bumped only by name.
    manual: bool,
}

const fn component(name: &'static str, dir: &'static str, role: Role) -> Component {
    Component {
        name,
        dir,
        extra: &[],
        role,
        wire: None,
        manual: false,
    }
}

const fn part(
    name: &'static str,
    dir: &'static str,
    app: &'static str,
    file: &'static str,
    memory: &'static [&'static str],
) -> Component {
    Component {
        extra: memory,
        ..component(name, dir, Role::Part { app, file })
    }
}

const LITE1_MEMORY: &[&str] = &["firmware/memory/memory-lite1.kdl"];
const LEGACY_MEMORY: &[&str] = &["firmware/memory/memory-atslite.kdl"];

const COMPONENTS: [Component; COMPONENT_COUNT] = [
    component(DESKTOP, "crates/odyssey-desktop", Role::Workflow),
    component(
        "odyssey-hub-client-ffi",
        "sdk/odyssey-hub-client-ffi",
        Role::Workflow,
    ),
    component(
        "odyssey-hub-client-uniffi",
        "sdk/odyssey-hub-client-uniffi",
        Role::Workflow,
    ),
    Component {
        wire: Some(MUX_WIRE),
        ..component("dongle", "firmware/dongle-fw", Role::Dongle)
    },
    Component {
        extra: &["firmware/memory/memory-marker-control-board.kdl"],
        ..component(
            "marker-control-board",
            "firmware/marker-control-board",
            Role::Workflow,
        )
    },
    part(
        "atslite1-netcore",
        "firmware/lite1-net-core",
        "atslite1",
        "net-core",
        LITE1_MEMORY,
    ),
    Component {
        manual: true,
        ..part(
            "atslite1-bootloader",
            "firmware/lite1-bootloader",
            "atslite1",
            "stage2-bootloader",
            LITE1_MEMORY,
        )
    },
    Component {
        extra: LITE1_MEMORY,
        wire: Some(DEVICE_WIRE),
        ..component("atslite1", "firmware/lite1", Role::App(AtsLite::Lite1))
    },
    part(
        "legacy-atslite1-netcore",
        "firmware/legacy-atslite-net-core",
        "legacy-atslite1",
        "net-core",
        LEGACY_MEMORY,
    ),
    Component {
        manual: true,
        ..part(
            "legacy-atslite1-bootloader",
            "firmware/legacy-atslite-bootloader",
            "legacy-atslite1",
            "stage2-bootloader",
            LEGACY_MEMORY,
        )
    },
    Component {
        extra: LEGACY_MEMORY,
        wire: Some(DEVICE_WIRE),
        ..component(
            "legacy-atslite1",
            "firmware/legacy-atslite",
            Role::App(AtsLite::Legacy),
        )
    },
];

/// Dot-separated numbers: a semver triple, or a release counter as one number.
fn parse_version(text: &str) -> Option<Vec<u64>> {
    text.split('.').map(|part| part.parse().ok()).collect()
}

fn dotted(version: &[u64]) -> String {
    let parts: Vec<String> = version.iter().map(u64::to_string).collect();
    parts.join(".")
}

struct Tag {
    version: Vec<u64>,
    commit: String,
}

/// Stdout of `program`, or `None` when it fails; stderr is dropped.
fn quiet(program: &str, args: &[&str]) -> Option<String> {
    let output = cmd::at(".", program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Every tag on GitHub as `(name, commit)`; a local tag list can be stale.
fn remote_tags() -> eros::Result<Vec<(String, String)>> {
    let listing = cmd::output(cmd::at(".", "gh").args([
        "api",
        "repos/{owner}/{repo}/tags",
        "--paginate",
        "--jq",
        r#".[] | "\(.name) \(.commit.sha)""#,
    ]))?;
    let mut tags = Vec::new();
    for line in listing.lines() {
        let (name, commit) = line
            .split_once(' ')
            .with_context(|| format!("unreadable tag line from gh: {line}"))?;
        tags.push((name.to_owned(), commit.to_owned()));
    }
    Ok(tags)
}

fn newest_tag(tags: &[(String, String)], name: &str) -> Option<Tag> {
    let prefix = format!("{name}-v");
    let mut newest: Option<Tag> = None;
    for (tag, commit) in tags {
        let Some(version) = tag.strip_prefix(&prefix).and_then(parse_version) else {
            continue;
        };
        if newest.as_ref().is_none_or(|tag| version > tag.version) {
            newest = Some(Tag {
                version,
                commit: commit.clone(),
            });
        }
    }
    newest
}

struct Closure {
    /// The Cargo package selected for version preparation.
    package: String,
    /// Everything whose change alters the build, relative to the repository root.
    paths: Vec<String>,
}

fn closure(component: &Component) -> eros::Result<Closure> {
    // --locked: a release must not be planned from a lock file cargo would rewrite.
    let text = cmd::output(cmd::at(component.dir, "cargo").args([
        "metadata",
        "--locked",
        "--format-version",
        "1",
    ]))?;
    let metadata: Value =
        serde_json::from_str(&text).context("cargo metadata printed invalid JSON")?;
    let mut closure = local_closure(&metadata, &cmd::root(), component.dir)?;
    closure
        .paths
        .extend(component.extra.iter().map(|path| (*path).to_owned()));
    Ok(closure)
}

/// The directories of `dir`'s package and of every path dependency it builds
/// with, plus its lock file. Registry and git dependencies are pinned by the lock.
fn local_closure(metadata: &Value, root: &Path, dir: &str) -> eros::Result<Closure> {
    let relative = |path: &Path| -> eros::Result<String> {
        let path = path
            .strip_prefix(root)
            .with_context(|| format!("{} is outside the repository", path.display()))?;
        Ok(path.to_string_lossy().into_owned())
    };
    let manifest = root.join(dir).join("Cargo.toml");
    let packages = metadata["packages"]
        .as_array()
        .context("cargo metadata lists no packages")?;
    let start = packages
        .iter()
        .find(|package| package["manifest_path"].as_str().map(Path::new) == Some(&manifest))
        .with_context(|| format!("cargo metadata does not list {}", manifest.display()))?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("cargo metadata has no dependency graph")?;

    let mut built = BTreeSet::new();
    let mut todo: Vec<&str> = start["id"].as_str().into_iter().collect();
    while let Some(id) = todo.pop() {
        if !built.insert(id) {
            continue;
        }
        let Some(node) = nodes.iter().find(|node| node["id"] == id) else {
            continue;
        };
        for dependency in node["deps"].as_array().map_or(&[][..], Vec::as_slice) {
            let in_image = dependency["dep_kinds"]
                .as_array()
                .is_none_or(|kinds| kinds.iter().any(|kind| kind["kind"] != "dev"));
            if in_image && let Some(package) = dependency["pkg"].as_str() {
                todo.push(package);
            }
        }
    }

    let mut paths = Vec::new();
    for package in packages {
        let local = package["source"].is_null();
        let Some(id) = package["id"].as_str() else {
            continue;
        };
        if !local || !built.contains(id) {
            continue;
        }
        let manifest = package["manifest_path"]
            .as_str()
            .context("a package has no manifest path")?;
        let dir = Path::new(manifest)
            .parent()
            .context("a manifest path has no directory")?;
        paths.push(relative(dir)?);
    }
    paths.sort();
    let workspace = metadata["workspace_root"]
        .as_str()
        .context("cargo metadata has no workspace root")?;
    paths.push(relative(&Path::new(workspace).join("Cargo.lock"))?);
    Ok(Closure {
        package: start["name"].as_str().unwrap_or_default().to_owned(),
        paths,
    })
}

fn changed_since(commit: &str, paths: &[String]) -> eros::Result<Vec<String>> {
    if quiet("git", &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_none() {
        eros::bail!("release commit {commit} is not in this checkout: fetch first");
    }
    let listing = cmd::output(
        cmd::at(".", "git")
            .args(["diff", "--name-only", commit, "HEAD", "--"])
            .args(paths),
    )?;
    Ok(listing.lines().map(str::to_owned).collect())
}

fn source_version(component: &Component) -> eros::Result<Vec<u64>> {
    let text = if matches!(component.role, Role::Part { .. }) {
        let main_rs = cmd::root().join(component.dir).join("src/main.rs");
        let source = fs::read_to_string(&main_rs)
            .with_context(|| format!("could not read {}", main_rs.display()))?;
        firmware::counter(&source)
            .with_context(|| format!("no release counter in {}", main_rs.display()))?
    } else {
        cmd::package_version(component.dir)?
    };
    let version = parse_version(&text)
        .with_context(|| format!("{} has an unreadable version '{text}'", component.name))?;
    Ok(version)
}

struct Row {
    component: &'static Component,
    closure: Closure,
    source: Vec<u64>,
    tag: Option<Tag>,
    state: State,
    why: String,
}

struct Plan {
    rows: Vec<Row>,
    tags: Vec<(String, String)>,
    links: [Link; COMPONENT_COUNT],
}

impl Plan {
    fn index(name: &str) -> eros::Result<usize> {
        let index = COMPONENTS
            .iter()
            .position(|component| component.name == name)
            .with_context(|| {
                let known: Vec<&str> = COMPONENTS.iter().map(|component| component.name).collect();
                format!("no component '{name}'; known: {}", known.join(", "))
            })?;
        Ok(index)
    }

    fn row(&self, name: &str) -> eros::Result<&Row> {
        Ok(&self.rows[Self::index(name)?])
    }

    fn states(&self) -> [State; COMPONENT_COUNT] {
        std::array::from_fn(|index| self.rows[index].state)
    }

    /// The components `names` selects, or `None` when it names none.
    fn named(names: &[String]) -> eros::Result<Option<[bool; COMPONENT_COUNT]>> {
        if names.is_empty() {
            return Ok(None);
        }
        let mut named = [false; COMPONENT_COUNT];
        for name in names {
            named[Self::index(name)?] = true;
        }
        Ok(Some(named))
    }
}

/// The component table as the rules see it. A part naming no component maps
/// past the table, which `pure_release::valid` rejects.
fn links() -> [Link; COMPONENT_COUNT] {
    COMPONENTS.each_ref().map(|component| Link {
        kind: match component.role {
            Role::Part { app, .. } => Kind::Part {
                app: COMPONENTS
                    .iter()
                    .position(|other| other.name == app)
                    .unwrap_or(usize::MAX),
            },
            Role::App(_) => Kind::App,
            Role::Workflow | Role::Dongle => Kind::Other,
        },
        manual: component.manual,
    })
}

fn plan() -> eros::Result<Plan> {
    let links = links();
    if !pure_release::valid(&links) {
        eros::bail!("the component table names a part without an application to deliver it");
    }
    let tags = remote_tags()?;
    let mut rows = Vec::new();
    let mut behind = [false; COMPONENT_COUNT];
    for component in &COMPONENTS {
        let closure = closure(component)?;
        let source = source_version(component)?;
        let tag = newest_tag(&tags, component.name);
        let changed = match &tag {
            Some(tag) => changed_since(&tag.commit, &closure.paths)?,
            None => Vec::new(),
        };
        let ahead = tag.as_ref().map(|tag| source.cmp(&tag.version));
        let Ok(state) = pure_release::state(ahead, !changed.is_empty()) else {
            eros::bail!(
                "{} is at {} in the source, below its newest release",
                component.name,
                dotted(&source)
            );
        };
        if let Role::Part { app, file } = component.role {
            let carried = firmware::nightly_head(app)?
                .and_then(|entry| entry["files"][file]["release"].as_u64())
                .unwrap_or(0);
            behind[rows.len()] = carried < source.first().copied().unwrap_or(0);
        }
        let why = match (&tag, changed.as_slice()) {
            (None, _) => "never released".to_owned(),
            (Some(_), []) => String::new(),
            (Some(_), [first, ..]) => format!("{} files changed, e.g. {first}", changed.len()),
        };
        rows.push(Row {
            component,
            closure,
            source,
            tag,
            state,
            why,
        });
    }
    let mut states: [State; COMPONENT_COUNT] = std::array::from_fn(|index| rows[index].state);
    let forced = pure_release::deliver(&links, &mut states, &behind);
    for (index, row) in rows.iter_mut().enumerate() {
        row.state = states[index];
        if let Some(part) = forced[index] {
            row.why = format!("delivers {}", COMPONENTS[part].name);
        }
    }
    Ok(Plan { rows, tags, links })
}

fn wire_constant(source: &str, name: &str) -> Option<[u64; 3]> {
    let (_, rest) = source.split_once(&format!("const {name}: [u16; 3] = ["))?;
    let (list, _) = rest.split_once(']')?;
    let parts: Vec<u64> = list
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    parts.try_into().ok()
}

/// The `min_desktop` of the row's next manifest entry: the oldest Desktop release
/// that speaks its wire version, and never below its previous entry's floor,
/// which records what the wire version cannot (an updater capability).
fn min_desktop(row: &Row, plan: &Plan) -> eros::Result<Option<String>> {
    let Some(wire) = row.component.wire else {
        return Ok(None);
    };
    let path = cmd::root().join(wire.file);
    let source =
        fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
    let want = wire_constant(&source, wire.constant)
        .with_context(|| format!("no {} in {}", wire.constant, wire.file))?;
    let prefix = format!("{DESKTOP}-v");
    let mut desktops = Vec::new();
    for (tag, commit) in &plan.tags {
        let Some(version) = tag.strip_prefix(&prefix).and_then(parse_version) else {
            continue;
        };
        let speaks = quiet("git", &["show", &format!("{commit}:{}", wire.file)])
            .and_then(|text| wire_constant(&text, wire.constant));
        desktops.push((version, speaks));
    }
    desktops.sort();
    let speaks: Vec<Option<[u64; 3]>> = desktops.iter().map(|(_, speaks)| *speaks).collect();
    let mut floor = if let Some(oldest) = pure_release::oldest_covering(want, &speaks) {
        desktops[oldest].0.clone()
    } else {
        let desktop = plan.row(DESKTOP)?;
        if desktop.state != State::Ready {
            eros::bail!(
                "no released Desktop speaks {} {}: bump {DESKTOP} first",
                wire.constant,
                dotted(&want)
            );
        }
        desktop.source.clone()
    };
    let previous = firmware::nightly_head(row.component.name)?
        .and_then(|entry| parse_version(entry["min_desktop"].as_str()?));
    if let Some(previous) = previous
        && previous > floor
    {
        floor = previous;
    }
    Ok(Some(dotted(&floor)))
}

fn print(plan: &Plan) -> eros::Result<()> {
    for row in &plan.rows {
        let released = row
            .tag
            .as_ref()
            .map_or_else(|| "-".to_owned(), |tag| dotted(&tag.version));
        let state = match row.state {
            State::Current => "current",
            State::NeedsBump if row.component.manual => "changed, bump by name",
            State::NeedsBump => "needs bump",
            State::Ready => "ready to publish",
        };
        let floor = min_desktop(row, plan)?
            .map_or_else(String::new, |floor| format!("  [desktop >= {floor}]"));
        println!(
            "{:<28} {:>7} -> {:<7} {state:<22} {}{floor}",
            row.component.name,
            released,
            dotted(&row.source),
            row.why,
        );
    }
    Ok(())
}

fn bump(names: &[String]) -> eros::Result<()> {
    let plan = plan()?;
    let named = Plan::named(names)?;
    let raise_these = pure_release::bump_set(&plan.links, &plan.states(), named.as_ref());
    let targets: Vec<&Row> = plan
        .rows
        .iter()
        .zip(raise_these)
        .filter_map(|(row, raise)| raise.then_some(row))
        .collect();
    if targets.is_empty() {
        println!("Nothing to bump");
        return Ok(());
    }
    if targets
        .iter()
        .any(|row| !matches!(row.component.role, Role::Part { .. }))
    {
        cmd::output(cmd::at(".", "cargo").args(["release", "--version"]))
            .context("cargo-release is required; enter the default Nix development shell")?;
    }
    for row in targets {
        let next = raise(row)?;
        println!(
            "{}: {} -> {}",
            row.component.name,
            dotted(&row.source),
            dotted(&next)
        );
    }
    println!(
        "Review the patch in a pull request and squash merge it, then publish from the merged master commit."
    );
    Ok(())
}

fn raise(row: &Row) -> eros::Result<Vec<u64>> {
    let dir = cmd::root().join(row.component.dir);
    if !matches!(row.component.role, Role::Part { .. }) {
        cmd::run(cmd::at(".", "cargo").args([
            "release",
            "version",
            "patch",
            "--manifest-path",
            &dir.join("Cargo.toml").to_string_lossy(),
            "--package",
            &row.closure.package,
            "--config",
            &cmd::root().join("release.toml").to_string_lossy(),
            "--isolated",
            "--execute",
            "--no-confirm",
        ]))?;
        return Ok(parse_version(&cmd::package_version(row.component.dir)?)
            .context("cargo-release produced an unsupported package version")?);
    }
    let mut next = row.source.clone();
    let maximum = match row.component.role {
        Role::Part {
            file: "net-core", ..
        } => u64::from(u16::MAX),
        Role::Part {
            file: "stage2-bootloader",
            ..
        } => 0x0FFF,
        _ => u64::MAX,
    };
    let last = next
        .last_mut()
        .with_context(|| format!("{} has no version components", row.component.name))?;
    *last = pure_release::successor(*last, maximum).with_context(|| {
        format!(
            "{} release counter exhausted at {}",
            row.component.name,
            dotted(&row.source)
        )
    })?;
    let new = dotted(&next);
    edit(&dir.join("src/main.rs"), |text| raise_counter(text, &new))?;
    Ok(next)
}

fn edit(path: &Path, change: impl FnOnce(&str) -> Option<String>) -> eros::Result<()> {
    let text =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    let changed = change(&text)
        .with_context(|| format!("{} does not hold the version to raise", path.display()))?;
    fs::write(path, changed).with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

/// `source` with its release counter set to `release`. Only the layout rustfmt
/// gives the macro is rewritten; any other is refused rather than guessed at.
fn raise_counter(source: &str, release: &str) -> Option<String> {
    let old = firmware::counter(source)?;
    let declared = source.rfind("macro_rules! release")?;
    let digits = declared + source[declared..].find(|c: char| c.is_ascii_digit())?;
    if !source[digits..].starts_with(&old) {
        return None;
    }
    let raised = format!(
        "{}{release}{}",
        &source[..digits],
        &source[digits + old.len()..]
    );
    (firmware::counter(&raised).as_deref() == Some(release)).then_some(raised)
}

fn publish(names: &[String], dry_run: bool) -> eros::Result<()> {
    cmd::require_clean_tree(".")?;
    let head = cmd::output(cmd::at(".", "git").args(["rev-parse", "HEAD"]))?;
    cmd::require_merged_master(&head)?;
    let plan = plan()?;
    let named = Plan::named(names)?;
    let order = match pure_release::publish_order(&plan.links, &plan.states(), named.as_ref()) {
        Ok(order) => order,
        Err(Refusal::NotReady(index)) => {
            let name = COMPONENTS[index].name;
            eros::bail!(
                "{name} is not ahead of its tag: run `cargo xtask release bump {name}` first"
            );
        }
        Err(Refusal::NoCarrier { part, app }) => {
            let (part, app) = (COMPONENTS[part].name, COMPONENTS[app].name);
            eros::bail!(
                "{part} has no {app} release to deliver it: run `cargo xtask release bump {app}` first"
            );
        }
    };
    let targets: Vec<&Row> = order
        .as_slice()
        .iter()
        .map(|&index| &plan.rows[index])
        .collect();
    for row in &plan.rows {
        if row.state == State::NeedsBump && !row.component.manual {
            eprintln!(
                "warning: {} changed since its tag and is not bumped ({})",
                row.component.name, row.why
            );
        }
    }
    // Every part is tagged before any is waited on, so their workflows overlap.
    for row in &targets {
        if matches!(row.component.role, Role::Part { .. }) && !dry_run {
            push_tag(&tag_of(row), &head)?;
        }
    }
    for row in targets {
        release(row, &plan, &head, dry_run)?;
    }
    Ok(())
}

fn tag_of(row: &Row) -> String {
    format!("{}-v{}", row.component.name, dotted(&row.source))
}

fn release(row: &Row, plan: &Plan, head: &str, dry_run: bool) -> eros::Result<()> {
    let version = dotted(&row.source);
    let tag = tag_of(row);
    let floor = min_desktop(row, plan)?;
    if dry_run {
        let floor = floor.map_or_else(String::new, |floor| format!(", min_desktop {floor}"));
        println!("would publish {tag} from {head}{floor}");
        return Ok(());
    }
    match row.component.role {
        Role::Workflow => {
            push_tag(&tag, head)?;
            println!("Pushed {tag}; its workflow builds and publishes it.");
        }
        Role::Part { .. } => wait_for_release(&tag)?,
        Role::Dongle => {
            push_tag(&tag, head)?;
            wait_for_release(&tag)?;
            let min_desktop = floor.context("the dongle has no wire version")?;
            dongle::run(dongle::Cmd::GenRelease { min_desktop })?;
        }
        Role::App(board) => {
            let min_desktop = floor.context("the application has no wire version")?;
            publish_app(row, board, &tag, &version, min_desktop, head)?;
        }
    }
    Ok(())
}

fn publish_app(
    row: &Row,
    board: AtsLite,
    tag: &str,
    version: &str,
    min_desktop: String,
    head: &str,
) -> eros::Result<()> {
    let device = row.component.name;
    let previous = firmware::nightly_head(device)?;
    if previous
        .as_ref()
        .is_some_and(|entry| entry["version"] == version)
    {
        eros::bail!(
            "the nightly manifest already lists {device} {version}: restore it, or finish this release by hand"
        );
    }
    // A floor is a compatibility claim nothing here can derive, so it carries
    // over from the previous entry; raise one with `gen-release` by hand.
    let floor = |key: &str| {
        previous
            .as_ref()
            .and_then(|entry| entry[key].as_u64())
            .and_then(|floor| u32::try_from(floor).ok())
            .unwrap_or(0)
    };
    firmware::run(board.cmd(Task::Build))?;
    firmware::run(board.cmd(Task::GenRelease {
        requires_bootloader: floor("requires_bootloader"),
        requires_netcore: floor("requires_netcore"),
        min_desktop,
    }))?;
    let title = format!("{device} v{version}");
    let notes = format!("{} application firmware {version}", board.product());
    cmd::run(cmd::at(row.component.dir, "gh").args([
        "release",
        "create",
        tag,
        "--draft",
        "--target",
        head,
        "--title",
        &title,
        "--notes",
        &notes,
        "build/app-a.bin",
        "build/app-b.bin",
    ]))?;
    cmd::run(cmd::at(".", "gh").args(["release", "edit", tag, "--draft=false"]))?;
    println!("Published {tag}");
    Ok(())
}

/// Creates the tag on GitHub, which starts the workflow watching for it.
fn push_tag(tag: &str, commit: &str) -> eros::Result<()> {
    cmd::run(cmd::at(".", "gh").args([
        "api",
        "--silent",
        "--method",
        "POST",
        "repos/{owner}/{repo}/git/refs",
        "-f",
        &format!("ref=refs/tags/{tag}"),
        "-f",
        &format!("sha={commit}"),
    ]))
}

/// Blocks until the tag's workflow has published its release, or has failed.
fn wait_for_release(tag: &str) -> eros::Result<()> {
    const FAILED_RUNS: &str = r#".[] | select(.status == "completed" and .conclusion != "success" and .conclusion != "skipped") | .url"#;
    eprintln!("Waiting for the {tag} workflow to publish");
    let started = Instant::now();
    while started.elapsed() < RELEASE_TIMEOUT {
        std::thread::sleep(RELEASE_POLL);
        let draft = quiet(
            "gh",
            &[
                "release", "view", tag, "--json", "isDraft", "--jq", ".isDraft",
            ],
        );
        if draft.as_deref() == Some("false") {
            println!("Published {tag}");
            return Ok(());
        }
        let failed = quiet(
            "gh",
            &[
                "run",
                "list",
                "--branch",
                tag,
                "--json",
                "status,conclusion,url",
                "--jq",
                FAILED_RUNS,
            ],
        );
        if let Some(run) = failed.filter(|run| !run.is_empty()) {
            eros::bail!("the workflow for {tag} failed: {run}");
        }
    }
    eros::bail!(
        "no {tag} release after {} minutes",
        RELEASE_TIMEOUT.as_secs() / 60
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_newest_tag_is_by_version_and_ignores_other_components() {
        let tags: Vec<(String, String)> = [
            "atslite1-v0.1.9",
            "atslite1-v0.1.22",
            "atslite1-netcore-v2",
            "atslite1-vnext",
            "legacy-atslite1-v0.1.23",
        ]
        .iter()
        .map(|tag| ((*tag).to_owned(), format!("commit-of-{tag}")))
        .collect();
        let newest = newest_tag(&tags, "atslite1").unwrap();
        assert_eq!(newest.version, [0, 1, 22]);
        assert_eq!(newest.commit, "commit-of-atslite1-v0.1.22");
        assert_eq!(newest_tag(&tags, "atslite1-netcore").unwrap().version, [2]);
        assert!(newest_tag(&tags, "dongle").is_none());
    }

    #[test]
    fn a_wire_constant_is_three_numbers() {
        let source = "/// doc\npub const MUX_PROTOCOL: [u16; 3] = [0, 4, 12];\n";
        assert_eq!(wire_constant(source, "MUX_PROTOCOL"), Some([0, 4, 12]));
        assert_eq!(wire_constant(source, "DEVICE_PROTOCOL"), None);
        for broken in ["[0, 4];", "[0, 4, 1, 2];", "[0, x, 1];", "[0, 4, 1"] {
            let source = format!("const MUX_PROTOCOL: [u16; 3] = {broken}");
            assert_eq!(wire_constant(&source, "MUX_PROTOCOL"), None, "{broken}");
        }
    }

    #[test]
    fn raising_a_counter_rewrites_only_the_last_declaration() {
        let source = "const PORT: u16 = 9;\nmacro_rules! release {\n    () => {\n        9\n    };\n}\nconst X: u8 = 9;\n";
        let raised = raise_counter(source, "10").unwrap();
        assert_eq!(
            raised,
            "const PORT: u16 = 9;\nmacro_rules! release {\n    () => {\n        10\n    };\n}\nconst X: u8 = 9;\n"
        );
        assert_eq!(raise_counter("fn main() {}", "2"), None);
        assert_eq!(
            raise_counter("macro_rules!release{()=>{4};}", "5"),
            None,
            "a layout rustfmt does not produce is refused"
        );
    }

    #[test]
    fn the_closure_follows_path_dependencies_but_not_dev_or_registry_ones() {
        let package = |name: &str, dir: &str, source: Option<&str>| {
            json!({
                "id": name, "name": name, "source": source,
                "manifest_path": format!("/repo/{dir}/Cargo.toml"),
            })
        };
        let dep = |name: &str, kind: Option<&str>| json!({ "pkg": name, "dep_kinds": [{ "kind": kind }] });
        let metadata = json!({
            "workspace_root": "/repo/firmware/app",
            "packages": [
                package("app", "firmware/app", None),
                package("shared", "crates/shared", None),
                package("deeper", "crates/deeper", None),
                package("fixture", "crates/fixture", None),
                package("unrelated", "crates/unrelated", None),
                package("serde", "vendor/serde", Some("registry+https://github.com/rust-lang/crates.io-index")),
            ],
            "resolve": { "nodes": [
                { "id": "app", "deps": [dep("shared", None), dep("fixture", Some("dev")), dep("serde", None)] },
                { "id": "shared", "deps": [dep("deeper", Some("build")), dep("app", None)] },
                { "id": "deeper", "deps": [] },
                { "id": "fixture", "deps": [dep("unrelated", None)] },
                { "id": "serde", "deps": [] },
            ] },
        });
        let closure = local_closure(&metadata, Path::new("/repo"), "firmware/app").unwrap();
        assert_eq!(closure.package, "app");
        assert_eq!(
            closure.paths,
            [
                "crates/deeper",
                "crates/shared",
                "firmware/app",
                "firmware/app/Cargo.lock"
            ]
        );
        assert!(local_closure(&metadata, Path::new("/repo"), "firmware/missing").is_err());
    }

    #[test]
    fn the_component_table_is_one_the_proofs_cover() {
        assert!(pure_release::valid(&links()));
    }
}
