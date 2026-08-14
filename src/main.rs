//! Thin binary wrapper: collect arguments, run the CLI, exit with its code.
//!
//! Setting the process exit status is itself an effect, so it goes through
//! the capability layer like everything else.

#![cfg_attr(
    not(test),
    deny(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = legacy::cli::run(&argv);
    legacy::cap::process::exit(code);
}
