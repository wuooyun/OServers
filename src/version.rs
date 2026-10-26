//! Version and build information shown in the GUI.
//!
//! The values are injected by `build.rs` at compile time:
//! - `VERSION` comes from `Cargo.toml`.
//! - `GIT_COMMIT` / `GIT_TAG` identify the checked-out commit, so a binary
//!   built by the release workflow reports the tag it was released from.
//! - `BUILD_TARGET` is the Rust target triple, shortened for display.

/// Version declared in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Short commit hash the binary was built from (`unknown` without git).
pub const GIT_COMMIT: &str = env!("OSERVERS_GIT_COMMIT");

/// Git tag if `HEAD` points exactly at a tag, otherwise empty.
pub const GIT_TAG: &str = env!("OSERVERS_GIT_TAG");

/// Rust target triple the binary was built for (e.g. `x86_64-pc-windows-msvc`).
pub const BUILD_TARGET: &str = env!("OSERVERS_BUILD_TARGET");

/// True when the binary was built from a tagged commit, i.e. by the release
/// workflow.
pub fn is_release() -> bool {
    !GIT_TAG.is_empty()
}

/// Short version label, e.g. `v0.1.0` for a tagged build or
/// `v0.1.0-dev.a1b2c3d` for a development build.
pub fn short() -> String {
    if is_release() {
        normalize_tag(GIT_TAG)
    } else {
        format!("v{VERSION}-dev.{GIT_COMMIT}")
    }
}

/// Detailed label with commit and platform, e.g.
/// `v0.1.0 · a1b2c3d · x86_64-pc-windows-msvc`. Used as the tooltip of the
/// version label in the GUI.
pub fn full() -> String {
    format!("{} · {} · {}", short(), GIT_COMMIT, BUILD_TARGET)
}

/// Tags are accepted both as `v0.2.0` and `0.2.0`; display them uniformly.
fn normalize_tag(tag: &str) -> String {
    if tag.starts_with('v') {
        tag.to_string()
    } else {
        format!("v{tag}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_is_never_empty() {
        assert!(!short().is_empty());
        assert!(full().contains(GIT_COMMIT));
    }

    #[test]
    fn tags_are_normalized() {
        assert_eq!(normalize_tag("v0.2.0"), "v0.2.0");
        assert_eq!(normalize_tag("0.2.0"), "v0.2.0");
    }
}
