mod orbbec;
mod prebuilt;

use std::fs;
use std::path::{Path, PathBuf};

use eros::Context as _;
use serde::Deserialize;

use crate::cmd;

const DIR: &str = "crates/odyssey-desktop";
const REPO: &str = "odysseyarm/odyssey";
const TAG_PREFIX: &str = "odyssey-desktop-v";

/// Windows release recipes. cargo-packager bundles for the host platform only, so these
/// run on the Windows box.
#[derive(clap::Subcommand)]
pub(crate) enum Cmd {
    /// Stage what the MSI needs for one variant: writes the `WiX` fragment the
    /// `wix` section of crates/odyssey-desktop/Cargo.toml points at and, for full,
    /// the SDK files and EULA it installs. The version, URLs, hashes and file list
    /// come only from crates/odyssey-desktop/orbbec-sdk.json, which the app reads too.
    PrepareOrbbec {
        variant: Variant,
        /// Stage directory [default: crates/odyssey-desktop/target/orbbec-stage].
        #[arg(long)]
        stage: Option<PathBuf>,
        /// Use this SDK zip instead of downloading it, for testing.
        #[arg(long)]
        archive: Option<PathBuf>,
        /// Use this EULA text instead of downloading it, for testing.
        #[arg(long)]
        eula: Option<PathBuf>,
    },
    /// Build the app and wrap it in an MSI in crates/odyssey-desktop/dist/<variant>.
    /// The full variant needs network access to fetch the pinned SDK.
    Bundle {
        #[arg(default_value = "online")]
        variant: Variant,
        /// Wrap the binaries of the Nix `desktop-windows` package (see desktop/prebuilt.rs)
        /// instead of compiling the app and cargo-packager here.
        #[arg(long)]
        prebuilt: Option<PathBuf>,
    },
    /// Build the app. Defaults: official manifests, release profile.
    Build(#[command(flatten)] BuildOpts),
    /// Build and run the app for HITL work. Defaults: dev manifests, hitl profile.
    Run(#[command(flatten)] BuildOpts),
    /// Publish the MSIs in crates/odyssey-desktop/dist/ to a GitHub release
    /// tagged odyssey-desktop-v<version>.
    Release {
        /// Defaults to the one in crates/odyssey-desktop/Cargo.toml.
        #[arg(long)]
        version: Option<String>,
        /// Falls back to `$GITHUB_TOKEN`.
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum Variant {
    /// The app downloads the Orbbec SDK.
    Online,
    /// Carries the SDK, so neither installing nor calibrating needs the
    /// internet. The app is built without the downloader. Whether offline
    /// mode is on is separate: `OFFLINE_MODE=1` at install time, or policy.
    Full,
}

/// Deployment selects only the manifest source: never optimization or trusted keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Deployment {
    Official,
    Dev,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Profile {
    Dev,
    Release,
    Hitl,
}

impl Profile {
    fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Release => "release",
            Self::Hitl => "hitl",
        }
    }
}

/// HITL hosts set the `ODYSSEY_*` variables in their own config, so the tailnet
/// address stays out of this public repository.
#[derive(clap::Args)]
pub(crate) struct BuildOpts {
    #[arg(long, value_enum, env = "ODYSSEY_DEPLOYMENT")]
    deployment: Option<Deployment>,
    /// HTTPS base URL holding `releases.<channel>.json`; required for dev.
    #[arg(long, env = "ODYSSEY_DEV_MANIFEST_URL")]
    manifest_url: Option<String>,
    #[arg(long, value_enum)]
    profile: Option<Profile>,
}

/// The manifest URL baked into the build, or none for the official default.
fn manifest_override(deployment: Deployment, url: Option<String>) -> eros::Result<Option<String>> {
    match (deployment, url) {
        (Deployment::Official, _) => Ok(None),
        (Deployment::Dev, None) => {
            eros::bail!("dev deployment requires --manifest-url or ODYSSEY_DEV_MANIFEST_URL")
        }
        (Deployment::Dev, Some(url)) => {
            let url = url.trim_end_matches('/');
            if !url.starts_with("https://") || url.len() == "https://".len() {
                eros::bail!("dev manifest URL must be https://<host>[/path]");
            }
            Ok(Some(url.to_owned()))
        }
    }
}

fn build(opts: BuildOpts, deployment: Deployment, profile: Profile, run: bool) -> eros::Result<()> {
    let deployment = opts.deployment.unwrap_or(deployment);
    let manifest = manifest_override(deployment, opts.manifest_url)?;
    let mut cargo = cmd::at(DIR, "cargo");
    cargo
        .arg(if run { "run" } else { "build" })
        .args(["--package", "odyssey-desktop", "--profile"])
        .arg(opts.profile.unwrap_or(profile).name());
    // An inherited value would silently retarget an official build.
    cargo.env_remove("ODYSSEY_FIRMWARE_MANIFEST_URL");
    if let Some(url) = &manifest {
        cargo.env("ODYSSEY_FIRMWARE_MANIFEST_URL", url);
    }
    cmd::run(&mut cargo)
}

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Full => "full",
        }
    }
}

pub(crate) fn run(command: Cmd) -> eros::Result<()> {
    match command {
        Cmd::PrepareOrbbec {
            variant,
            stage,
            archive,
            eula,
        } => {
            let stage = stage.unwrap_or_else(orbbec::default_stage);
            orbbec::prepare(variant, &stage, archive.as_deref(), eula.as_deref())?;
            Ok(())
        }
        Cmd::Bundle { variant, prebuilt } => bundle(variant, prebuilt),
        Cmd::Build(opts) => build(opts, Deployment::Official, Profile::Release, false),
        Cmd::Run(opts) => build(opts, Deployment::Dev, Profile::Hitl, true),
        Cmd::Release { version, token } => release(version, token),
    }
}

fn bundle(variant: Variant, prebuilt: Option<PathBuf>) -> eros::Result<()> {
    let out_dir = format!("dist/{}", variant.name());
    // Check the prebuilt directory before the (large) SDK download.
    let prebuilt = prebuilt
        .map(|dir| prebuilt::packager(variant, &dir, &out_dir))
        .transpose()?;
    let stage = orbbec::prepare(variant, &orbbec::default_stage(), None, None)?;

    let mut packager = if let Some(packager) = prebuilt {
        packager
    } else {
        // cargo-packager packages a finished binary, it does not build one.
        let mut build = cmd::at(DIR, "cargo");
        build.args(["build", "--release", "--package", "odyssey-desktop"]);
        // Customer installers always read the official manifests.
        build.env_remove("ODYSSEY_FIRMWARE_MANIFEST_URL");
        // The full app is the same code without the SDK downloader.
        if let Variant::Full = variant {
            build.arg("--no-default-features");
        }
        cmd::run(&mut build)?;

        let mut packager = cmd::at(DIR, "cargo");
        packager
            .args(["bin", "cargo-packager", "--release", "--formats", "wix"])
            .args(["--packages", "odyssey-desktop", "--out-dir", &out_dir]);
        packager
    };
    // The installer template reads these (see wix/main.wxs). cargo-packager
    // only forwards variables with this prefix to WiX.
    packager
        .env("CARGO_PACKAGER_ODYSSEY_MSI_VARIANT", variant.name())
        .env("CARGO_PACKAGER_ODYSSEY_ORBBEC_STAGE", &stage);
    cmd::run(&mut packager)?;

    let Some(msi) = first_msi(&cmd::root().join(DIR).join(out_dir))? else {
        eros::bail!("cargo-packager produced no msi");
    };
    println!("{}", msi.display());
    Ok(())
}

/// The alphabetically first `*.msi` in `dir`, or none when there is none or no `dir`.
fn first_msi(dir: &Path) -> eros::Result<Option<PathBuf>> {
    if !dir.exists() {
        return Ok(None);
    }
    let entries = fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))?;
    let mut msis = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("listing {}", dir.display()))?
            .path();
        if path.extension().is_some_and(|extension| extension == "msi") {
            msis.push(path);
        }
    }
    msis.sort();
    Ok(msis.into_iter().next())
}

#[derive(Deserialize)]
struct Release {
    id: u64,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
}

fn release(version: Option<String>, token: Option<String>) -> eros::Result<()> {
    let version = match version {
        Some(version) => version,
        None => cmd::package_version(DIR)?,
    };
    let Some(token) = token.or_else(|| std::env::var("GITHUB_TOKEN").ok()) else {
        eros::bail!(
            "No token: pass one (cargo xtask desktop release --token <token>) or set GITHUB_TOKEN"
        );
    };

    // What `bundle online` and `bundle full` left in dist/.
    let dist = cmd::root().join(DIR).join("dist");
    let mut built = Vec::new();
    for variant in [Variant::Online, Variant::Full] {
        if let Some(msi) = first_msi(&dist.join(variant.name()))? {
            built.push((variant, msi));
        }
    }
    if built.is_empty() {
        eros::bail!(
            "No msi found (run: cargo xtask desktop bundle online, and/or cargo xtask desktop bundle full)"
        );
    }

    let tag = format!("{TAG_PREFIX}{version}");
    let api = format!("https://api.github.com/repos/{REPO}");

    // Reuse the release if the tag was already pushed, so a re-run only replaces the asset.
    let release: Release = if let Ok(mut response) =
        github(ureq::get(format!("{api}/releases/tags/{tag}")), &token).call()
    {
        json(response.body_mut())?
    } else {
        let body = serde_json::json!({
            "tag_name": tag,
            "name": format!("Odyssey Desktop v{version}"),
        });
        let mut response = github(ureq::post(format!("{api}/releases")), &token)
            .content_type("application/json")
            .send(body.to_string())
            .with_context(|| format!("creating the release {tag}"))?;
        json(response.body_mut())?
    };

    for (variant, msi) in built {
        let name = asset_name(variant, &version);
        if let Some(existing) = release.assets.iter().find(|asset| asset.name == name) {
            github(
                ureq::delete(format!("{api}/releases/assets/{}", existing.id)),
                &token,
            )
            .call()
            .with_context(|| format!("deleting the old {name}"))?;
        }

        let upload = format!(
            "https://uploads.github.com/repos/{REPO}/releases/{}/assets?name={name}",
            release.id
        );
        let file = fs::File::open(&msi).with_context(|| format!("opening {}", msi.display()))?;
        github(ureq::post(upload), &token)
            .content_type("application/octet-stream")
            .send(file)
            .with_context(|| format!("uploading {name}"))?;

        println!("Published {name} to {tag}");
    }
    Ok(())
}

fn github<B>(request: ureq::RequestBuilder<B>, token: &str) -> ureq::RequestBuilder<B> {
    request
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "odyssey-desktop-release")
}

fn json<T: serde::de::DeserializeOwned>(body: &mut ureq::Body) -> eros::Result<T> {
    let text = body
        .read_to_string()
        .context("reading the GitHub response")?;
    Ok(serde_json::from_str(&text).context("parsing the GitHub response")?)
}

fn asset_name(variant: Variant, version: &str) -> String {
    match variant {
        Variant::Online => format!("Odyssey-{version}-win-x64.msi"),
        Variant::Full => format!("Odyssey-{version}-win-x64-full.msi"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_controls_manifest_override() {
        let url = |s: &str| Some(s.to_owned());
        assert_eq!(manifest_override(Deployment::Official, None).unwrap(), None);
        assert_eq!(
            manifest_override(Deployment::Official, url("https://hitl")).unwrap(),
            None
        );
        assert!(manifest_override(Deployment::Dev, None).is_err());
        assert!(manifest_override(Deployment::Dev, url("http://hitl.ts.net")).is_err());
        assert!(manifest_override(Deployment::Dev, url("https://")).is_err());
        assert_eq!(
            manifest_override(Deployment::Dev, url("https://hitl.ts.net/manifests/")).unwrap(),
            url("https://hitl.ts.net/manifests")
        );
    }
}
