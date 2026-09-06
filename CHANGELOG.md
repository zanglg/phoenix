# Changelog

All notable changes to Phoenix are documented here. The project uses SemVer-compatible release
numbers and remains explicitly unstable before 1.0.0. During initial construction, changes remain
under `Unreleased` and the Cargo version remains 0.0.0 until the 0.1.0 release gate is satisfied.

## [Unreleased]

### Added

- Initial Rust 2024 workspace with a single Phoenix version source.
- Stable Rust toolchain policy, Rust 1.95 MSRV, and AArch64 bare-metal target.
- Minimal, non-bootable `no_std` kernel crate.
- Initial AArch64 bootstrap implementation in `boot.S`; Cargo integration remains pending.
- Config-file-driven rust-analyzer target selection.
- Standalone host-side xtask commands for environment checks and validation.
- Project, Agent, Roadmap, and kernel capability documentation.
