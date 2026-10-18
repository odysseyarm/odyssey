use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=input.css");
    // Tailwind generates only the classes it finds in the sources, so a new
    // class in a view has to rerun it too.
    println!("cargo:rerun-if-changed=src");

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

    if std::env::var_os("CARGO_FEATURE_ORBBEC").is_some() {
        make_orbbec_runtime_findable();
    }
}

/// Lets a development build load the Orbbec SDK at runtime.
///
/// `orbbec-sdk-sys` builds `libOrbbecSDK.so` (`OrbbecSDK.dll` on Windows) and
/// its `extensions/` directory into its own build directory and sets no rpath,
/// so a binary linked against it fails to start. The SDK finds `extensions/`
/// beside the library it loaded, so both must stay together.
///
/// - Unix: the executable gets an rpath to that build directory (works for
///   `cargo run`, `cargo test` and `dx serve` alike) and to `$ORIGIN`, which
///   is where a package that ships the library beside the executable loads it.
/// - Windows has no rpath; the DLL and `extensions/` are copied next to the
///   executable and into `deps/`, where test binaries run.
///
/// Packaging a release (MSI, AppImage, Nix) still has to place the library
/// and `extensions/` beside the executable; nothing here runs for those.
fn make_orbbec_runtime_findable() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    // OUT_DIR is <target>/<profile>/build/<crate>-<hash>/out. The SDK crate is a
    // direct dependency (see Cargo.toml), so cargo has run its build script
    // before this one and left the output in a sibling directory.
    let Some(lib_dir) = out_dir.ancestors().nth(2).and_then(find_orbbec_lib_dir) else {
        println!(
            "cargo:warning=OrbbecSDK build output not found; the binary starts only if the SDK library is on the loader path"
        );
        return;
    };
    println!("cargo:rerun-if-changed={}", lib_dir.display());

    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let Some(profile_dir) = out_dir.ancestors().nth(3) else { return };
        for dest in [profile_dir.to_path_buf(), profile_dir.join("deps")] {
            if let Err(e) = copy_runtime(&lib_dir, &dest) {
                println!("cargo:warning=could not copy the OrbbecSDK runtime to {}: {e}", dest.display());
            }
        }
    } else {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}

/// The newest `orbbec-sdk-sys-*/out/lib` (or `out/bin`, where Windows puts the
/// DLL) under `build_dir` that holds the SDK library.
fn find_orbbec_lib_dir(build_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(build_dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("orbbec-sdk-sys-"))
        .flat_map(|e| [e.path().join("out").join("lib"), e.path().join("out").join("bin")])
        .filter(|dir| {
            std::fs::read_dir(dir).is_ok_and(|mut files| {
                files.any(|f| {
                    f.is_ok_and(|f| {
                        let name = f.file_name().to_string_lossy().to_lowercase();
                        name.starts_with("liborbbecsdk") || name.starts_with("orbbecsdk.dll")
                    })
                })
            })
        })
        .max_by_key(|dir| std::fs::metadata(dir).and_then(|m| m.modified()).ok())
}

/// Copies the DLL and `extensions/` from `src` into `dest`.
fn copy_runtime(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let lower = name.to_string_lossy().to_lowercase();
        if lower == "extensions" || (lower.starts_with("orbbecsdk") && lower.ends_with(".dll")) {
            if entry.file_type()?.is_dir() {
                copy_tree(&entry.path(), &dest.join(&name))?;
            } else {
                copy_file(&entry.path(), &dest.join(&name))?;
            }
        }
    }
    Ok(())
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Links `src` to `dest` instead of copying where the filesystem allows (the
/// SDK is large) and leaves a file that is already current alone.
fn copy_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    let stamp = |p: &Path| std::fs::metadata(p).and_then(|m| Ok((m.len(), m.modified()?))).ok();
    if stamp(dest).zip(stamp(src)).is_some_and(|((dl, dm), (sl, sm))| dl == sl && dm >= sm) {
        return Ok(());
    }
    let _ = std::fs::remove_file(dest);
    if std::fs::hard_link(src, dest).is_err() {
        std::fs::copy(src, dest)?;
    }
    Ok(())
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
