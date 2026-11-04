#![expect(
    clippy::print_stderr,
    reason = "command-line tool: stderr is where it reports failure"
)]

use std::process;

/// ANSI escape sequences wrapping the failure message in red.
const ANSI_RED: &str = "\x1b[31m";
const ANSI_RESET: &str = "\x1b[0m";

fn main() {
    let res = cargo_run_bin::cli::run();

    // Only reached if run-bin code fails, otherwise process exits early from within
    // binary::run.
    if let Err(res) = res {
        eprintln!("{ANSI_RED}run-bin failed: {res}{ANSI_RESET}");
        process::exit(1);
    }
}
