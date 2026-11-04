/// The Odyssey Desktop version this build is, for matching against the oldest
/// desktop each firmware release declares it needs.
pub(super) fn desktop_version() -> [u16; 3] {
    [
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
    ]
}
