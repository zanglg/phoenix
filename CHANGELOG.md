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
- Fixed-capacity physical memory normalization, conservative reservation, and first-fit frame allocation.
- Host-tested AArch64 translation indices, stage-1 descriptors, mapping attributes, and fixed-capacity offline mapping plans.
- Pinned QEMU `virt` command generation and a bounded boot integration harness with sentinel, timeout, output-limit, and log handling.
- Host-tested AArch64 vector-slot model, stable exception-frame ABI, ESR decoding, SVC immediate extraction, and data-abort classification.
- Linked 2 KiB AArch64 EL1 vector table, complete register save/restore assembly, `VBAR_EL1` installation, and fatal exception diagnostics.
- Host-tested lower-39-bit user ranges, W^X mapping plans, and guarded initial stack placement.
- Strict allocation-free ELF64/AArch64 executable validation with load ranges, zero-fill, permissions, overlap, and entry-point checks.
- Native ABI revision-0 syscall values plus AArch64 `x8`/`x0..x5` request and signed return-register adaptation.
- Opt-in, statically validated first-EL0 probe with private TTBR0 tables, RX code, guarded zeroed stack, `eret`, returning unknown syscall, and terminal `exit(42)` path.
- Separate bounded `test-el0` QEMU protocol that cannot pass on the earlier boot marker.
- Transactional process-image planning for ELF segments plus a guarded stack, with per-page clear/copy work, unique frame ownership, and all-or-nothing allocation and release.
- Retry-safe populated-image type state over an abstract private-frame backend, with full-page clearing before initialized ELF bytes are copied.
- Allocator-owned AArch64 user-table topology with intermediate-table reuse, transactional frame assignment, retry-safe descriptor materialization, and exact combined leaf/table ownership before activation.
- Checked QEMU `virt` bootstrap physical-memory backend for private image frames and translation tables under the temporary TTBR1 RAM alias.
- Transactional DTB-derived boot memory map that removes firmware reservations, the complete kernel image, and the borrowed DTB before allocator creation.
- Conservative `/reserved-memory` parsing that reserves static child ranges and rejects unsupported dynamic or translated reservation forms.
- Project, Agent, Roadmap, and kernel capability documentation.
