fn main() {
    println!("cargo:rerun-if-changed=input.css");

    // Install required packages
    let toolchain = install_packages();

    // Compile TailwindCSS .css file
    let output = std::process::Command::new(toolchain)
        .args([
            "@tailwindcss/cli",
            "-i",
            "input.css",
            "-o",
            "assets/tailwind.css",
            "--minify",
        ])
        .env("NODE_ENV", "production")
        .output()
        .expect("failed to run @tailwindcss/cli");

    if !output.status.success() {
        panic!(
            "@tailwindcss/cli failed with status {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// Installs required packages and selects toolchain to use.
///
/// It will prioritize `yarn` over `npm`.
///
/// # Panic
/// Will panic if none of the toolchains is installed.
fn install_packages() -> &'static str {
    let yarn = if_windows("yarn.cmd", "yarn");
    let npm = if_windows("npm.cmd", "npm");
    let npx = if_windows("npx.cmd", "npx");

    if let Ok(status) = std::process::Command::new(yarn).arg("install").status() {
        if !status.success() {
            panic!("ERROR: `yarn install` failed with status {status}");
        }
        return yarn;
    }

    match std::process::Command::new(npm).arg("install").status() {
        Ok(status) if status.success() => npx,
        Ok(status) => panic!("ERROR: `npm install` failed with status {status}"),
        Err(e) => panic!("ERROR: Npm or Yarn installation is needed.\n{e}"),
    }
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
