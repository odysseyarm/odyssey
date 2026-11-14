mod artwork;
mod orbbec_ffi;

use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub(crate) enum Cmd {
    /// Writes the print artwork for the scale board described by board.json.
    ///
    /// Each layout is a vector PDF and SVG at exactly its physical size (print at
    /// 100%, "actual size", never "fit to page"). All of them carry the same dots:
    /// the grid in board.json, the same pitch and dot size, with the dots listed
    /// under "missing" left out (that breaks the grid's 180 degree symmetry), and a
    /// scale bar to check the print against a rule. Only the space around the dots
    /// differs, so one detector reads all of them. No text and no embedded fonts,
    /// so nothing can be substituted at the print shop.
    ///
    /// With neither --layout, --all nor --custom it writes the quick paper board,
    /// the first thing to print: scale-board-paper-letter.
    Artwork(artwork::Args),
    /// Regenerates crates/odyssey-autocalib/src/orbbec/ffi.rs, the checked-in
    /// function table for `OrbbecSDK` v2.
    ///
    /// Needs, on the machine that runs it and nowhere in the normal build:
    /// bindgen-cli 0.72.x (`cargo install bindgen-cli --version 0.72.1 --locked`),
    /// libclang (set `LIBCLANG_PATH` if bindgen cannot find it) and rustfmt.
    ///
    /// What gets emitted follows the backend source, so adding an SDK call to
    /// src/orbbec.rs or src/orbbec/runtime.rs and rerunning this is the whole
    /// procedure: functions are every `ob_*` identifier those files mention,
    /// constants every `ffi::OB*` path, types every `ffi::ob_*` path plus whatever
    /// the emitted functions' signatures reach. bindgen cannot select single enum
    /// variants, so it emits each reached enum in full and the constants nothing
    /// references are then dropped.
    OrbbecFfi {
        /// The `include/` directory of the official `OrbbecSDK` v2.10.6 release
        /// archive (or of a `CMake` install of that tag). The raw source tree is not
        /// enough: `libobsensor/h/Export.h` only exists after the SDK's `CMake` step.
        include: PathBuf,
    },
}

pub(crate) fn run(command: Cmd) -> eros::Result<()> {
    match command {
        Cmd::Artwork(args) => artwork::run(&args),
        Cmd::OrbbecFfi { include } => orbbec_ffi::regenerate(&include),
    }
}
