//! Build script: embeds version/build metadata into the binary.
//!
//! The values are exposed through `cargo:rustc-env` and read by
//! `src/version.rs`, which shows them in the GUI so users can tell exactly
//! which build they are running.
//!
//! Everything here degrades gracefully: when git is unavailable (e.g. a
//! source archive without `.git`), the commit is reported as `unknown` and
//! the build is treated as a development build.

use std::process::Command;

fn main() {
    // Re-run when the checked-out commit or the tags change, so the embedded
    // version stays accurate (e.g. right after `git tag v0.2.0`).
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let tag = git(&["describe", "--tags", "--exact-match", "HEAD"]).unwrap_or_default();
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=OSERVERS_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=OSERVERS_GIT_TAG={tag}");
    println!("cargo:rustc-env=OSERVERS_BUILD_TARGET={target}");
}

/// Run a git command, returning its trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}
