/// `version` as `[major, minor, patch]`. A pre-release or build suffix after
/// `-` or `+` is ignored. Anything else that is not `N.N.N` fails to compile
/// when called in a `const`.
///
/// Moved from `firmware/dongle-fw/src/main.rs`.
///
/// Since: Odyssey Desktop (unreleased).
/// # Panics
/// Panics when `version` is not exactly three numeric components or a component exceeds `u32`.
///
pub const fn parse_cargo_version(version: &str) -> [u32; 3] {
    let bytes = version.as_bytes();
    let mut out = [0u32; 3];
    let mut i = 0;
    let mut part = 0;
    let mut saw_digit = false;

    while i < bytes.len() {
        let b = bytes[i];
        if b >= b'0' && b <= b'9' {
            saw_digit = true;
            let digit = (b - b'0') as u32;
            assert!(
                out[part] <= (u32::MAX - digit) / 10,
                "CARGO_PKG_VERSION component exceeds u32"
            );
            out[part] = out[part] * 10 + digit;
        } else if b == b'.' {
            assert!(saw_digit && part < 2, "invalid CARGO_PKG_VERSION");
            part += 1;
            saw_digit = false;
        } else if b == b'-' || b == b'+' {
            break;
        } else {
            panic!("invalid CARGO_PKG_VERSION");
        }
        i += 1;
    }

    assert!(part == 2 && saw_digit, "invalid CARGO_PKG_VERSION");

    out
}

/// Full `SemVer` assigned to calling firmware build.
///
/// Release builds can set `ODYSSEY_BUILD_VERSION` to a validated Nightly
/// version. Ordinary builds use the package's clean Cargo `SemVer`.
///
/// Since: Odyssey Desktop (unreleased).
#[macro_export]
macro_rules! cargo_semver {
    () => {
        match option_env!("ODYSSEY_BUILD_VERSION") {
            Some(version) => version,
            None => ::core::env!("CARGO_PKG_VERSION"),
        }
    };
}

/// Numeric core of the calling firmware's full build version.
///
/// Since: Odyssey Desktop (unreleased).
#[macro_export]
macro_rules! cargo_version {
    () => {
        $crate::parse_cargo_version($crate::cargo_semver!())
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_three_components() {
        assert_eq!(parse_cargo_version("0.1.23"), [0, 1, 23]);
        assert_eq!(parse_cargo_version("14.63.63"), [14, 63, 63]);
        assert_eq!(parse_cargo_version("4294967295.0.1"), [u32::MAX, 0, 1]);
    }

    #[test]
    fn ignores_a_pre_release_or_build_suffix() {
        assert_eq!(parse_cargo_version("1.2.3-alpha.1"), [1, 2, 3]);
        assert_eq!(parse_cargo_version("1.2.3+build.5"), [1, 2, 3]);
    }

    #[test]
    fn the_macro_reads_the_calling_crate() {
        assert_eq!(cargo_version!(), [0, 0, 0]);
    }

    #[test]
    fn rejects_anything_but_three_numbers() {
        for bad in [
            "",
            "1",
            "1.2",
            "1..3",
            "1.2.",
            ".1.2",
            "1.2.3.4",
            "a.b.c",
            "1.2.x",
            "4294967296.0.0",
        ] {
            assert!(
                std::panic::catch_unwind(|| parse_cargo_version(bad)).is_err(),
                "{bad:?} was accepted"
            );
        }
    }
}
