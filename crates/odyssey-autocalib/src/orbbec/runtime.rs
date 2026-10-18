//! Finding and loading the OrbbecSDK shared library at run time.
//!
//! The application links nothing from the SDK, so it starts and works without
//! it. [`runtime_status`] tells the UI which of four situations holds, and
//! [`load`] hands the backend the function table once a usable library is in
//! memory.
//!
//! Candidates are tried in this order:
//!
//! 1. The directory in [`SDK_DIR_ENV`]. An explicit override is final: a
//!    library that is present there but fails to load or has an unsupported
//!    version is reported as such, and nothing else is tried.
//! 2. The library's plain file name, so the operating system's loader applies
//!    `LD_LIBRARY_PATH`, rpaths, the loader cache or the Windows DLL search
//!    order. This is how a packager's wrapper or a global install is found.
//! 3. A directory the application supplies, for a copy it manages itself.
//!
//! No install location is guessed. A candidate that is simply absent is
//! recorded in [`RuntimeStatus::NotFound`]; one that is present but broken is
//! remembered and reported unless a later candidate loads.

use std::ffi::{c_int, OsStr};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;
use parking_lot::Mutex;

use super::ffi::OrbbecSdk;

/// Environment variable naming a directory that holds the SDK library, with
/// its `extensions/` directory beside it.
pub const SDK_DIR_ENV: &str = "ODYSSEY_ORBBEC_SDK_DIR";

/// File name the SDK library is loaded by.
#[cfg(target_os = "windows")]
pub const LIBRARY_NAME: &str = "OrbbecSDK.dll";
#[cfg(target_os = "macos")]
pub const LIBRARY_NAME: &str = "libOrbbecSDK.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const LIBRARY_NAME: &str = "libOrbbecSDK.so.2";

/// Versions the backend accepts, worded for people.
///
/// Policy: SDK major version 2 with minor version 8 or newer. The bindings come
/// from 2.10.6 and the backend is tested only with that release; accepting the
/// 2.x series from 2.8 on is a product decision that assumes minor releases
/// keep the C functions the backend calls (a library that lacks one fails to
/// load with the missing symbol named). Older 2.x releases and every other
/// major version are refused rather than risked.
pub const SUPPORTED_VERSIONS: &str = "2.8 or newer in the 2.x series (tested with 2.10.6)";

/// A loaded SDK. Every handle holds a clone, so the library stays mapped for as
/// long as anything created from it exists.
pub(super) type Sdk = Arc<OrbbecSdk>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SdkVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SdkVersion {
    /// Whether the backend accepts this version; see [`SUPPORTED_VERSIONS`].
    pub fn is_supported(self) -> bool {
        self.major == 2 && self.minor >= 8
    }
}

impl fmt::Display for SdkVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Whether camera support is usable on this machine, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    /// The library loaded and its version is supported. `library_path` is
    /// `None` when the operating system's loader found it by name.
    Available { version: SdkVersion, library_path: Option<PathBuf> },
    /// No candidate holds the library. `tried` lists, in order, where it was
    /// looked for and what each lookup said.
    NotFound { tried: Vec<String> },
    /// A library is there but the loader refused it, or it lacks the SDK's
    /// functions. `library` is the path, or the name for a by-name load.
    LoadFailed { library: String, error: String },
    /// The library loaded and reports a version the backend does not accept.
    UnsupportedVersion { found: SdkVersion, supported: &'static str },
}

impl fmt::Display for RuntimeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Available { version, library_path: Some(path) } => {
                write!(f, "Orbbec SDK {version} loaded from {}", path.display())
            }
            Self::Available { version, library_path: None } => {
                write!(f, "Orbbec SDK {version} loaded from the system library path")
            }
            Self::NotFound { tried } => {
                write!(f, "the Orbbec SDK was not found (tried: {})", tried.join("; "))
            }
            Self::LoadFailed { library, error } => {
                write!(f, "the Orbbec SDK at {library} could not be loaded: {error}")
            }
            Self::UnsupportedVersion { found, supported } => {
                write!(f, "Orbbec SDK {found} is not supported; use {supported}")
            }
        }
    }
}

/// A usable SDK and where it came from.
#[derive(Clone)]
pub(super) struct Loaded {
    pub sdk: Sdk,
    pub version: SdkVersion,
    pub library_path: Option<PathBuf>,
}

/// The library loaded by the first successful [`load`]. It stays mapped for
/// the life of the process: the SDK starts threads and registers state that
/// make unloading it unsafe, and loading it again would only repeat the work.
static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

/// Looks for the SDK as described in the module docs and reports the outcome.
///
/// A successful result is cached, so later calls are cheap. A failed lookup is
/// repeated on the next call, which is how an SDK installed while the
/// application runs gets noticed.
pub fn runtime_status(extra_dir: Option<&Path>) -> RuntimeStatus {
    match load(extra_dir) {
        Ok(loaded) => RuntimeStatus::Available { version: loaded.version, library_path: loaded.library_path },
        Err(status) => status,
    }
}

/// The cached SDK, or the result of a fresh lookup. `extra_dir` only matters
/// while nothing is loaded yet.
pub(super) fn load(extra_dir: Option<&Path>) -> Result<Loaded, RuntimeStatus> {
    // The lock spans the lookup so two threads never load the library twice.
    let mut cache = LOADED.lock();
    if let Some(loaded) = &*cache {
        return Ok(loaded.clone());
    }
    let env_dir = std::env::var_os(SDK_DIR_ENV).filter(|v| !v.is_empty()).map(PathBuf::from);
    let loaded = resolve(env_dir.as_deref(), extra_dir, OsStr::new(LIBRARY_NAME))?;
    tracing::info!("{}", RuntimeStatus::Available { version: loaded.version, library_path: loaded.library_path.clone() });
    *cache = Some(loaded.clone());
    Ok(loaded)
}

/// What one candidate came to.
enum Attempt {
    Loaded(Loaded),
    /// Nothing there; the text says where and what the lookup answered.
    Absent(String),
    /// Something is there but unusable.
    Failed(RuntimeStatus),
}

/// Walks the candidates in order. Separate from [`load`] so tests can pass
/// directories and a library name instead of touching the environment.
fn resolve(env_dir: Option<&Path>, extra_dir: Option<&Path>, name: &OsStr) -> Result<Loaded, RuntimeStatus> {
    let mut tried = Vec::new();
    let mut broken: Option<RuntimeStatus> = None;

    match env_dir {
        None => tried.push(format!("{SDK_DIR_ENV} is not set")),
        Some(dir) => match try_dir(dir, name) {
            Attempt::Loaded(loaded) => return Ok(loaded),
            Attempt::Failed(status) => return Err(status),
            Attempt::Absent(why) => tried.push(format!("{SDK_DIR_ENV}: {why}")),
        },
    }

    match try_name(name) {
        Attempt::Loaded(loaded) => return Ok(loaded),
        Attempt::Failed(status) => {
            tracing::warn!("{status}");
            broken = Some(status);
        }
        Attempt::Absent(why) => tried.push(why),
    }

    match extra_dir {
        None => tried.push("no application-supplied directory".to_string()),
        Some(dir) => match try_dir(dir, name) {
            Attempt::Loaded(loaded) => return Ok(loaded),
            Attempt::Failed(status) => {
                tracing::warn!("{status}");
                broken.get_or_insert(status);
            }
            Attempt::Absent(why) => tried.push(format!("application directory: {why}")),
        },
    }

    Err(broken.unwrap_or(RuntimeStatus::NotFound { tried }))
}

fn try_dir(dir: &Path, name: &OsStr) -> Attempt {
    let path = dir.join(name);
    if !path.is_file() {
        return Attempt::Absent(format!("{} does not exist", path.display()));
    }
    // SAFETY: loading a shared library runs its initializers. The file is the
    // SDK the user or the application pointed us at.
    match unsafe { open_path(&path) } {
        Ok(library) => bind(library, &path.display().to_string(), Some(path)),
        Err(error) => Attempt::Failed(RuntimeStatus::LoadFailed {
            library: path.display().to_string(),
            error: describe(&error),
        }),
    }
}

fn try_name(name: &OsStr) -> Attempt {
    let shown = name.to_string_lossy();
    // SAFETY: as in `try_dir`; the operating system's loader picks the file.
    match unsafe { Library::new(name) } {
        Ok(library) => bind(library, &shown, None),
        Err(error) if is_not_found(&error, &shown) => {
            Attempt::Absent(format!("{shown} through the system library search path: {}", describe(&error)))
        }
        Err(error) => Attempt::Failed(RuntimeStatus::LoadFailed { library: shown.into_owned(), error: describe(&error) }),
    }
}

/// Opens a library by path. Windows looks for the library's own dependencies
/// next to it instead of only beside the executable.
///
/// # Safety
/// Loading a library runs its initialization code.
unsafe fn open_path(path: &Path) -> Result<Library, libloading::Error> {
    #[cfg(windows)]
    {
        // SAFETY: forwarded to the caller.
        let library = unsafe {
            libloading::os::windows::Library::load_with_flags(path, libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH)
        };
        library.map(Library::from)
    }
    #[cfg(not(windows))]
    {
        // SAFETY: forwarded to the caller.
        unsafe { Library::new(path) }
    }
}

/// Whether a by-name load failed because no such library exists, as opposed to
/// the loader finding a file (or one of its dependencies) it cannot use.
///
/// Verified against glibc's wording; the Windows and macOS branches follow what
/// those loaders report for a missing module.
fn is_not_found(error: &libloading::Error, name: &str) -> bool {
    #[cfg(windows)]
    {
        // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_MOD_NOT_FOUND. The
        // last also covers a missing dependency, which the message then shows.
        let _ = name;
        let code = std::error::Error::source(error)
            .and_then(|cause| cause.downcast_ref::<std::io::Error>())
            .and_then(std::io::Error::raw_os_error);
        matches!(code, Some(2 | 3 | 126))
    }
    #[cfg(not(windows))]
    {
        let message = describe(error);
        // glibc words a missing dependency the same way but names the
        // dependency first, so the prefix tells the two apart.
        let glibc = message.starts_with(name) && message.contains("cannot open shared object file");
        let dyld = message.contains(&format!("dlopen({name},"))
            && message.contains("(no such file")
            && !message.contains("Library not loaded");
        glibc || dyld
    }
}

/// The loader's message. `libloading` keeps the operating system's reason in
/// the error's source on Windows, where its own text only names the call.
fn describe(error: &libloading::Error) -> String {
    match std::error::Error::source(error) {
        Some(cause) => format!("{error}: {cause}"),
        None => error.to_string(),
    }
}

type VersionFn = unsafe extern "C" fn() -> c_int;

/// Reads the version, checks the policy and builds the function table.
/// Dropping `library` on a refusal unloads it; nothing has been created from it.
fn bind(library: Library, origin: &str, library_path: Option<PathBuf>) -> Attempt {
    let failed = |error: libloading::Error| {
        Attempt::Failed(RuntimeStatus::LoadFailed { library: origin.to_string(), error: describe(&error) })
    };
    // Versions come first and through single lookups: the full table needs
    // every function this backend calls, which an unsupported release may lack.
    let part = |symbol: &[u8]| -> Result<u32, libloading::Error> {
        // SAFETY: the SDK documents these as `int f(void)`.
        let f: libloading::Symbol<'_, VersionFn> = unsafe { library.get(symbol) }?;
        Ok(u32::try_from(unsafe { f() }).unwrap_or(0))
    };
    let version = match (part(b"ob_get_major_version\0"), part(b"ob_get_minor_version\0"), part(b"ob_get_patch_version\0")) {
        (Ok(major), Ok(minor), Ok(patch)) => SdkVersion { major, minor, patch },
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return failed(e),
    };
    if !version.is_supported() {
        return Attempt::Failed(RuntimeStatus::UnsupportedVersion { found: version, supported: SUPPORTED_VERSIONS });
    }
    // SAFETY: `from_library` only looks up symbols.
    let sdk = match unsafe { OrbbecSdk::from_library(library) } {
        Ok(sdk) => Arc::new(sdk),
        Err(e) => return failed(e),
    };
    // The SDK finds `extensions/` beside the library it was loaded from, but
    // only we know that library when it came from a directory. A by-name load
    // leaves the lookup to the SDK.
    let extensions = library_path.as_deref().and_then(Path::parent).map(|dir| dir.join("extensions")).filter(|d| d.is_dir());
    super::initialize(&sdk, extensions.as_deref());
    Attempt::Loaded(Loaded { sdk, version, library_path })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name no system loader will resolve, so tests never meet a real SDK.
    const MISSING: &str = "libodyssey-no-such-library.so.0";

    fn garbage_library(dir: &Path) -> PathBuf {
        let path = dir.join(MISSING);
        std::fs::write(&path, b"this is not a shared object").unwrap();
        path
    }

    fn tried(status: RuntimeStatus) -> Vec<String> {
        match status {
            RuntimeStatus::NotFound { tried } => tried,
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn nothing_anywhere_is_not_found_and_lists_every_lookup_in_order() {
        let env = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let Err(status) = resolve(Some(env.path()), Some(extra.path()), OsStr::new(MISSING)) else {
            panic!("no library exists");
        };
        let tried = tried(status);
        assert_eq!(tried.len(), 3, "{tried:?}");
        assert!(tried[0].starts_with(SDK_DIR_ENV) && tried[0].contains(&env.path().display().to_string()), "{tried:?}");
        assert!(tried[1].starts_with(MISSING), "{tried:?}");
        assert!(tried[2].contains(&extra.path().display().to_string()), "{tried:?}");
    }

    #[test]
    fn unset_candidates_are_recorded_not_skipped() {
        let Err(status) = resolve(None, None, OsStr::new(MISSING)) else { panic!("no library exists") };
        let tried = tried(status);
        assert_eq!(tried.len(), 3, "{tried:?}");
        assert_eq!(tried[0], format!("{SDK_DIR_ENV} is not set"));
        assert_eq!(tried[2], "no application-supplied directory");
    }

    #[test]
    fn broken_library_in_the_env_dir_is_final() {
        let env = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let broken = garbage_library(env.path());
        // A second broken copy proves the extra directory is never consulted.
        garbage_library(extra.path());
        let Err(RuntimeStatus::LoadFailed { library, error }) =
            resolve(Some(env.path()), Some(extra.path()), OsStr::new(MISSING))
        else {
            panic!("expected LoadFailed");
        };
        assert_eq!(library, broken.display().to_string());
        assert!(!error.is_empty());
    }

    #[test]
    fn broken_library_in_the_extra_dir_beats_not_found() {
        let extra = tempfile::tempdir().unwrap();
        let broken = garbage_library(extra.path());
        let Err(RuntimeStatus::LoadFailed { library, .. }) = resolve(None, Some(extra.path()), OsStr::new(MISSING))
        else {
            panic!("expected LoadFailed");
        };
        assert_eq!(library, broken.display().to_string());
    }

    #[test]
    fn directory_without_the_file_is_absent_even_if_it_has_other_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("unrelated.so"), b"x").unwrap();
        // A directory named like the library is not a library either.
        std::fs::create_dir(dir.path().join(MISSING)).unwrap();
        assert!(matches!(try_dir(dir.path(), OsStr::new(MISSING)), Attempt::Absent(_)));
    }

    #[test]
    fn by_name_load_of_a_broken_file_is_a_load_failure_not_absence() {
        let dir = tempfile::tempdir().unwrap();
        let broken = garbage_library(dir.path());
        let Attempt::Failed(RuntimeStatus::LoadFailed { library, error }) = try_name(broken.as_os_str()) else {
            panic!("a file the loader rejects is not 'absent'");
        };
        assert_eq!(library, broken.display().to_string());
        assert!(!error.is_empty());
    }

    #[test]
    fn broken_by_name_library_is_reported_when_no_later_candidate_loads() {
        let dir = tempfile::tempdir().unwrap();
        let broken = garbage_library(dir.path());
        let Err(RuntimeStatus::LoadFailed { library, .. }) = resolve(None, None, broken.as_os_str()) else {
            panic!("the broken file must not be reported as missing");
        };
        assert_eq!(library, broken.display().to_string());
    }

    #[test]
    fn version_policy_accepts_2_8_and_newer_2_x_only() {
        let v = |major, minor| SdkVersion { major, minor, patch: 0 };
        assert!(v(2, 8).is_supported());
        assert!(v(2, 10).is_supported());
        assert!(!v(2, 7).is_supported());
        assert!(!v(1, 9).is_supported());
        assert!(!v(3, 0).is_supported());
    }

    #[test]
    fn version_prints_as_dotted_triple() {
        assert_eq!(SdkVersion { major: 2, minor: 10, patch: 6 }.to_string(), "2.10.6");
    }
}
