//! Compile-time build identity.

use core::fmt;

/// Immutable identity embedded into a Phoenix kernel image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildInfo {
    /// Phoenix project version.
    pub version: &'static str,
    /// Source commit used for the build.
    pub git_commit: &'static str,
    /// Whether the source tree contained uncommitted changes.
    pub git_state: &'static str,
    /// Rust compilation target.
    pub target: &'static str,
    /// Cargo build profile.
    pub profile: &'static str,
}

impl BuildInfo {
    /// Build identity for the currently compiled image.
    pub const CURRENT: Self = Self {
        version: env!("CARGO_PKG_VERSION"),
        git_commit: env!("PHOENIX_GIT_COMMIT"),
        git_state: env!("PHOENIX_GIT_DIRTY"),
        target: env!("PHOENIX_BUILD_TARGET"),
        profile: env!("PHOENIX_BUILD_PROFILE"),
    };
}

impl fmt::Display for BuildInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Phoenix {} (commit {}, {}, target {}, profile {})",
            self.version, self.git_commit, self.git_state, self.target, self.profile
        )
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::format;

    use super::BuildInfo;

    #[test]
    fn display_is_stable_and_complete() {
        let identity = BuildInfo {
            version: "0.0.0",
            git_commit: "0123456789ab",
            git_state: "clean",
            target: "aarch64-unknown-none-softfloat",
            profile: "debug",
        };

        assert_eq!(
            format!("{identity}"),
            "Phoenix 0.0.0 (commit 0123456789ab, clean, target \
             aarch64-unknown-none-softfloat, profile debug)"
        );
    }
}
