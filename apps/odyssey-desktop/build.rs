use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=input.css");

    install_packages();

    let output = std::process::Command::new(tailwind_bin())
        .args(["-i", "input.css", "-o", "assets/tailwind.css", "--minify"])
        .env("NODE_ENV", "production")
        .output()
        .expect("failed to run tailwindcss");

    if !output.status.success() {
        panic!(
            "tailwindcss failed with status {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// Installs the packages in package.json, preferring yarn over npm.
///
/// # Panic
/// Panics if neither is installed, or if the install fails.
fn install_packages() {
    let yarn = if_windows("yarn.cmd", "yarn");
    let npm = if_windows("npm.cmd", "npm");

    if let Ok(status) = std::process::Command::new(yarn).arg("install").status() {
        if !status.success() {
            panic!("ERROR: `yarn install` failed with status {status}");
        }
        return;
    }

    match std::process::Command::new(npm).arg("install").status() {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("ERROR: `npm install` failed with status {status}"),
        Err(e) => panic!("ERROR: Npm or Yarn installation is needed.\n{e}"),
    }
}

/// The CLI that @tailwindcss/cli installs, run straight out of node_modules.
///
/// Going through a package runner instead splits on the installer: `npx
/// @tailwindcss/cli` resolves the package name, while yarn 1 looks for a
/// binary of that name and fails. Windows runners ship yarn, so that path
/// only breaks in CI.
fn tailwind_bin() -> PathBuf {
    PathBuf::from("node_modules")
        .join(".bin")
        .join(if_windows("tailwindcss.cmd", "tailwindcss"))
}

const fn if_windows(_windows: &'static str, _unix: &'static str) -> &'static str {
    #[cfg(windows)]
    {
        _windows
    }
    #[cfg(unix)]
    {
        _unix
    }
}
