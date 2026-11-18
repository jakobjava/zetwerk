//! Says which commit zetwerk is built from, for what a report says of
//! the program (see `zetwerk_app::viewer::report`). Outside a git
//! checkout, or without git, it is "unknown".

use std::process::Command;

fn main() {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|said| !said.is_empty())
    };
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=ZETWERK_COMMIT={commit}");
    // Built again when another commit is checked out or made.
    if let Some(directory) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={directory}/HEAD");
        println!("cargo:rerun-if-changed={directory}/logs/HEAD");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
