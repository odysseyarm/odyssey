/// `version` as `[major, minor, patch]`. A pre-release or build suffix after
/// `-` or `+` is ignored; anything else that is not `N.N.N` fails to compile
/// when called in a `const`.
///
/// Moved from `firmware/dongle-fw/src/main.rs`.
pub const fn parse_cargo_version(version: &str) -> [u16; 3] {
    let bytes = version.as_bytes();
    let mut out = [0u16; 3];
    let mut i = 0;
    let mut part = 0;
    let mut saw_digit = false;

    while i < bytes.len() {
        let b = bytes[i];
        if b >= b'0' && b <= b'9' {
            saw_digit = true;
            out[part] = out[part] * 10 + (b - b'0') as u16;
        } else if b == b'.' {
            if !saw_digit || part >= 2 {
                panic!("invalid CARGO_PKG_VERSION");
            }
            part += 1;
            saw_digit = false;
        } else if b == b'-' || b == b'+' {
            break;
        } else {
            panic!("invalid CARGO_PKG_VERSION");
        }
        i += 1;
    }

    if part != 2 || !saw_digit {
        panic!("invalid CARGO_PKG_VERSION");
    }

    out
}

/// The `version` of the crate that calls this, as `[major, minor, patch]`.
///
/// Firmware publishes its version in `bcdDevice` and over the control
/// protocol; this is the one place either reads it from `Cargo.toml`.
#[macro_export]
macro_rules! cargo_version {
    () => {
        $crate::parse_cargo_version(::core::env!("CARGO_PKG_VERSION"))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_three_components() {
        assert_eq!(parse_cargo_version("0.1.23"), [0, 1, 23]);
        assert_eq!(parse_cargo_version("14.63.63"), [14, 63, 63]);
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
            "", "1", "1.2", "1..3", "1.2.", ".1.2", "1.2.3.4", "a.b.c", "1.2.x",
        ] {
            assert!(
                std::panic::catch_unwind(|| parse_cargo_version(bad)).is_err(),
                "{bad:?} was accepted"
            );
        }
    }
}
