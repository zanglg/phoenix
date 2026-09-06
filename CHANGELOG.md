# Changelog

All notable changes to Phoenix are documented here. The project uses SemVer-compatible release
numbers and remains explicitly unstable before 1.0.0. During initial construction, changes remain
under `Unreleased` and the Cargo version remains 0.0.0 until the 0.1.0 release gate is satisfied.

## [Unreleased]

### Added

- Initial Rust 2024 workspace with a single Phoenix version source.
- Stable Rust toolchain policy, Rust 1.95 MSRV, and AArch64 bare-metal target.
- `no_std` AArch64 kernel binary with the initial bootstrap, higher-half linker layout, BSS
  initialization, 64 KiB boot stack, PL011 early console, panic path, and safe halt loop.
- Build identity plus deterministic boot-success and panic sentinels.
- ELF, raw kernel image, and linker-map generation with static layout and symbol inspection.
- Config-file-driven rust-analyzer target selection.
- Standalone host-side xtask commands for environment checks, tests, builds, and validation.
- Explicit Host Tested, Cross Compiled, ELF Inspected, Runtime Pending, and Runtime Verified states.
- Checked architecture-neutral physical/virtual addresses, pages, frames, and half-open ranges.
- Strict allocation-free DTB parsing with boot CPU, bootargs, memory-region, and reservation extraction.
- Project, Agent, Roadmap, and kernel capability documentation.
