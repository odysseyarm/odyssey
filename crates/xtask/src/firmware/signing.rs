//! Release signatures: `cargo xtask firmware sign` and the hook that attaches
//! them to manifest entries. The message and trusted keys live in the
//! `release-signing` crate so firmware and this tool cannot disagree.
//!
//! The signer is chosen by `--signer` or `ODYSSEY_SIGNER`. Release generation
//! requires signatures accepted by production keys.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use eros::Context as _;
use release_signing::{Component, MESSAGE_LEN, SIGNATURE_LEN, message};
use serde_json::Value;

use super::FileDigest;

/// Environment variable naming the signer for `gen-release`.
pub(super) const SIGNER_ENV: &str = "ODYSSEY_SIGNER";

/// Seed of the development key whose public half is `DEV_KEY` in
/// `firmware/release-signing/src/keys.rs`. It is public on purpose: it signs
/// only dev and test builds, which are the only ones that trust that key.
const DEV_SEED_HEX: &str = "da87ce81e4a702a27b21edf4e4594f6fd277189ee3958c648e50ec7335aa4b04";

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum SignedComponent {
    Lite1App,
    Lite1Netcore,
    LegacyAtsliteApp,
    LegacyAtsliteNetcore,
    MarkerApp,
    DongleApp,
}

impl SignedComponent {
    fn component(self) -> Component {
        match self {
            Self::Lite1App => Component::Lite1App,
            Self::Lite1Netcore => Component::Lite1NetCore,
            Self::LegacyAtsliteApp => Component::LegacyAtsliteApp,
            Self::LegacyAtsliteNetcore => Component::LegacyAtsliteNetCore,
            Self::MarkerApp => Component::MarkerApp,
            Self::DongleApp => Component::DongleApp,
        }
    }

    fn is_net_core(self) -> bool {
        matches!(self, Self::Lite1Netcore | Self::LegacyAtsliteNetcore)
    }
}

#[derive(clap::Args)]
pub(crate) struct SignArgs {
    component: SignedComponent,
    /// The image to sign.
    image: PathBuf,
    /// Application `SemVer`, or the release counter for a net core.
    #[arg(long)]
    version: String,
    /// `dev` signs with the public development seed. Anything else is a
    /// command that reads the message on stdin and writes the 64 raw signature
    /// bytes to stdout. Defaults to `ODYSSEY_SIGNER`.
    #[arg(long)]
    signer: Option<String>,
}

pub(crate) enum Signer {
    Dev,
    /// An external program, split on whitespace (no quoting), so a hardware
    /// key's tool can be driven without a shell on any platform.
    Command(String),
}

impl Signer {
    fn parse(spec: &str) -> Self {
        if spec == "dev" {
            Self::Dev
        } else {
            Self::Command(spec.to_owned())
        }
    }

    /// The signer `ODYSSEY_SIGNER` names, or `None` when unset or empty.
    pub(super) fn from_env() -> eros::Result<Option<Self>> {
        match std::env::var(SIGNER_ENV) {
            Ok(spec) if !spec.trim().is_empty() => Ok(Some(Self::parse(spec.trim()))),
            Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => eros::bail!("{SIGNER_ENV} is not valid UTF-8: {error}"),
        }
    }

    pub(super) fn required_from_env() -> eros::Result<Self> {
        Ok(Self::from_env()?.context(
            "ODYSSEY_SIGNER must name a production-key signer before generating this release",
        )?)
    }

    fn sign(&self, message: &[u8; MESSAGE_LEN]) -> eros::Result<[u8; SIGNATURE_LEN]> {
        match self {
            Self::Dev => {
                eprintln!("warning: dev signer; only dev-signing-key builds trust this signature");
                let mut seed = [0; 32];
                hex::decode_to_slice(DEV_SEED_HEX, &mut seed).context("dev seed is not hex")?;
                Ok(salty::Keypair::from(&seed).sign(message).to_bytes())
            }
            Self::Command(command) => run_signer(command, message),
        }
    }

    /// Signs an image given by the hash and size a manifest entry carries,
    /// and refuses a signature no trusted key accepts.
    pub(super) fn sign_digest(
        &self,
        component: Component,
        digest: &FileDigest,
        version: [u32; 3],
    ) -> eros::Result<String> {
        self.sign_digest_with(component, digest, version, false)
    }

    pub(super) fn sign_digest_production(
        &self,
        component: Component,
        digest: &FileDigest,
        version: [u32; 3],
    ) -> eros::Result<String> {
        self.sign_digest_with(component, digest, version, true)
    }

    fn sign_digest_with(
        &self,
        component: Component,
        digest: &FileDigest,
        version: [u32; 3],
        production_only: bool,
    ) -> eros::Result<String> {
        let mut sha256 = [0; 32];
        hex::decode_to_slice(&digest.sha256, &mut sha256)
            .with_context(|| format!("sha256 {:?} is not 32 bytes of hex", digest.sha256))?;
        let size = u32::try_from(digest.size).with_context(|| {
            format!("image of {} bytes exceeds the u32 size field", digest.size)
        })?;
        let message = message(component, size, version, &sha256);
        let signature = self.sign(&message)?;
        // A signature that fails here would be rejected by every device that
        // enforces signatures, so it must never reach a manifest.
        let valid = if production_only {
            release_signing::verify_production(&message, &signature)
        } else {
            release_signing::verify(&message, &signature)
        };
        if !valid {
            eros::bail!(
                "the signature does not verify against any trusted key; add the signer's public key to RELEASE_KEYS in firmware/release-signing/src/keys.rs"
            );
        }
        Ok(hex::encode(signature))
    }

    /// Adds `signature` to each listed file of a manifest entry, computed from
    /// the `sha256` and `size` already in it.
    pub(super) fn sign_files(
        &self,
        entry: &mut Value,
        files: &[(&str, Component, [u32; 3])],
    ) -> eros::Result<()> {
        for &(key, component, version) in files {
            let file = entry
                .get_mut("files")
                .and_then(|files| files.get_mut(key))
                .with_context(|| format!("entry has no file {key:?} to sign"))?;
            let digest = FileDigest {
                sha256: file["sha256"].as_str().unwrap_or_default().to_owned(),
                size: file["size"].as_u64().unwrap_or_default(),
            };
            file["signature"] = self
                .sign_digest_production(component, &digest, version)
                .with_context(|| format!("could not sign {key:?}"))?
                .into();
        }
        Ok(())
    }
}

fn run_signer(command: &str, message: &[u8; MESSAGE_LEN]) -> eros::Result<[u8; SIGNATURE_LEN]> {
    let mut words = command.split_whitespace();
    let program = words.next().context("the signer command is empty")?;
    let mut child = Command::new(program)
        .args(words)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("could not start signer `{command}`"))?;
    // Short enough to fit a pipe buffer, so writing before reading cannot deadlock.
    child
        .stdin
        .take()
        .context("signer stdin was not piped")?
        .write_all(message)
        .with_context(|| format!("could not send the message to `{command}`"))?;
    let output = child
        .wait_with_output()
        .with_context(|| format!("could not wait for `{command}`"))?;
    if !output.status.success() {
        eros::bail!("signer `{command}` failed: {}", output.status);
    }
    let written = output.stdout.len();
    let Ok(signature) = <[u8; SIGNATURE_LEN]>::try_from(output.stdout) else {
        eros::bail!(
            "signer `{command}` wrote {written} bytes; expected {SIGNATURE_LEN} raw signature bytes"
        );
    };
    Ok(signature)
}

/// `[major, minor, patch]` of an application version. Pre-release and build
/// parts are not part of the signed triple, matching what the device reports.
pub(super) fn app_version(text: &str) -> eros::Result<[u32; 3]> {
    let version = semver::Version::parse(text)
        .with_context(|| format!("{text:?} is not a SemVer version"))?;
    let part = |value: u64| {
        u32::try_from(value).with_context(|| format!("{text:?} has a part above u32::MAX"))
    };
    Ok([
        part(version.major)?,
        part(version.minor)?,
        part(version.patch)?,
    ])
}

pub(crate) fn sign(args: &SignArgs) -> eros::Result<()> {
    let signer = match &args.signer {
        Some(spec) => Signer::parse(spec),
        None => {
            Signer::from_env()?.with_context(|| format!("pass --signer or set {SIGNER_ENV}"))?
        }
    };
    let version = if args.component.is_net_core() {
        let release = args.version.parse::<u32>().with_context(|| {
            format!(
                "net-core version {:?} is not a release counter",
                args.version
            )
        })?;
        [release, 0, 0]
    } else {
        app_version(&args.version)?
    };
    let data = fs::read(&args.image)
        .with_context(|| format!("could not read {}", args.image.display()))?;
    let signature =
        signer.sign_digest(args.component.component(), &FileDigest::of(&data), version)?;
    println!("{signature}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use release_signing::Component;
    use serde_json::json;

    use super::{FileDigest, Signer, app_version};

    #[test]
    fn dev_signature_roundtrips_through_the_device_verifier() {
        let image = b"firmware image";
        let digest = FileDigest::of(image);
        let signature = Signer::Dev
            .sign_digest(Component::Lite1App, &digest, [1, 2, 3])
            .expect("dev signer signs");
        assert_eq!(signature.len(), 128);

        let mut sha256 = [0; 32];
        hex::decode_to_slice(&digest.sha256, &mut sha256).expect("hex");
        let mut raw = [0; 64];
        hex::decode_to_slice(&signature, &mut raw).expect("hex");
        let size = u32::try_from(image.len()).expect("small image");
        let message = release_signing::message(Component::Lite1App, size, [1, 2, 3], &sha256);
        assert!(release_signing::verify(&message, &raw));
        let other = release_signing::message(Component::Lite1NetCore, size, [1, 2, 3], &sha256);
        assert!(!release_signing::verify(&other, &raw));
    }

    #[test]
    fn release_manifest_refuses_development_signatures() {
        let app = FileDigest::of(b"app");
        let mut entry = json!({ "files": {
            "app": { "sha256": app.sha256, "size": app.size },
        } });
        Signer::Dev
            .sign_files(&mut entry, &[("app", Component::MarkerApp, [1, 0, 0])])
            .expect_err("production release must not trust the public development key");
        assert!(entry["files"]["app"].get("signature").is_none());
    }

    #[test]
    fn sign_files_refuses_a_missing_file() {
        let mut entry = json!({ "files": {} });
        Signer::Dev
            .sign_files(&mut entry, &[("app", Component::MarkerApp, [1, 0, 0])])
            .unwrap_err();
    }

    #[test]
    fn app_version_drops_prerelease_and_build() {
        assert_eq!(
            app_version("1.2.3-nightly.1+abc").expect("valid"),
            [1, 2, 3]
        );
        app_version("1.2").unwrap_err();
    }

    #[cfg(unix)]
    #[test]
    fn external_signer_output_must_be_a_signature() {
        let digest = FileDigest::of(b"x");
        for command in [
            "true",
            "head -c 64 /dev/zero",
            "false",
            "no-such-signer-binary",
        ] {
            Signer::Command(command.to_owned())
                .sign_digest(Component::MarkerApp, &digest, [1, 0, 0])
                .unwrap_err();
        }
    }
}
