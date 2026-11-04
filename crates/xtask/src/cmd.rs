use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use eros::Context as _;

/// The repository root, independent of the directory `cargo xtask` ran from.
pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask sits two levels below the repository root")
        .to_path_buf()
}

/// A command whose working directory is `dir`, relative to the repository root.
pub(crate) fn at(dir: impl AsRef<Path>, program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root().join(dir));
    command
}

/// llvm-objcopy from the pinned toolchain's `llvm-tools` component (listed in
/// rust-toolchain.toml), so rustup, the nix shells and CI need no binutils
/// package. The host's own objcopy cannot read the ARM ELFs.
pub(crate) fn llvm_objcopy() -> eros::Result<PathBuf> {
    let sysroot = output(at(".", "rustc").args(["--print", "sysroot"]))?;
    let version = output(at(".", "rustc").arg("-vV"))?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("`rustc -vV` printed no host line")?;
    let tool = Path::new(&sysroot)
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(format!("llvm-objcopy{}", std::env::consts::EXE_SUFFIX));
    if !tool.is_file() {
        eros::bail!(
            "{} is missing: install the toolchain's llvm-tools component",
            tool.display()
        );
    }
    Ok(tool)
}

/// Echo the command, run it with inherited stdio, and fail on a nonzero exit.
pub(crate) fn run(command: &mut Command) -> eros::Result<()> {
    eprintln!("$ {}", display(command));
    let status = command
        .status()
        .with_context(|| format!("could not start `{}`", display(command)))?;
    if !status.success() {
        eros::bail!("`{}` failed: {status}", display(command));
    }
    Ok(())
}

/// Run the command and return its trimmed stdout; stderr passes through.
pub(crate) fn output(command: &mut Command) -> eros::Result<String> {
    let output = command
        .stderr(std::process::Stdio::inherit())
        .output()
        .with_context(|| format!("could not start `{}`", display(command)))?;
    if !output.status.success() {
        eros::bail!("`{}` failed: {}", display(command), output.status);
    }
    let stdout = String::from_utf8(output.stdout)
        .with_context(|| format!("`{}` printed non-UTF-8", display(command)))?;
    Ok(stdout.trim().to_owned())
}

/// The `[package]` version of the manifest in `dir` (relative to the repository
/// root): the first `version = "..."` line, which every manifest here puts in
/// its `[package]` table.
pub(crate) fn package_version(dir: impl AsRef<Path>) -> eros::Result<String> {
    let manifest = root().join(dir).join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("could not read {}", manifest.display()))?;
    let version = first_version(&text)
        .with_context(|| format!("no version line in {}", manifest.display()))?;
    Ok(version.to_owned())
}

fn first_version(manifest: &str) -> Option<&str> {
    manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = \""))
        .and_then(|rest| rest.split_once('"'))
        .map(|(version, _)| version)
}

fn display(command: &Command) -> String {
    let mut line = command.get_program().to_string_lossy().into_owned();
    for arg in command.get_args() {
        line.push(' ');
        line.push_str(&arg.to_string_lossy());
    }
    line
}

#[cfg(test)]
mod tests {
    use super::first_version;

    #[test]
    fn first_version_reads_the_package_line_not_a_dependency() {
        let manifest = "[package]\nname = \"x\"\nversion = \"0.6.7\"\n\n[dependencies]\nfoo = { version = \"1\" }\n";
        assert_eq!(first_version(manifest), Some("0.6.7"));
    }

    #[test]
    fn first_version_is_none_without_a_version_line() {
        assert_eq!(first_version("[package]\nname = \"x\"\n"), None);
        assert_eq!(first_version("version = \"unterminated\n"), None);
    }
}
