# Changelog

All notable changes to Phoenix are documented here. The project uses SemVer-compatible release
numbers and remains explicitly unstable before 1.0.0. During initial construction, changes remain
under `Unreleased` and the Cargo version remains 0.0.0 until the 0.1.0 release gate is satisfied.

## [Unreleased]

### Added

- Initial Rust 2024 workspace with a single Phoenix version source.
- Stable Rust toolchain policy, Rust 1.95 MSRV, and AArch64 bare-metal target.
- `no_std` AArch64 kernel binary with the initial bootstrap, higher-half linker layout, BSS
  initialization, 512 KiB boot-construction stack, PL011 early console, panic path, and safe halt loop.
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
- Checked target-side DTB borrowing and linker-symbol conversion for the physical boot image
  reservation under the temporary QEMU `virt` high-memory alias.
- Opt-in boot-memory integration image and bounded `test-memory` protocol covering real DTB
  parsing, reservations, frame allocation, higher-half byte readback, scrub, and release.
- Allocation-free native initial user stack with argc, argv, envp, minimal auxiliary vector,
  16-byte alignment, checked capacity, and page-offset-aware process-image population.
- Ownership-gated AArch64 user-address-space activation that requires a permanently retained
  complete leaf/table owner before publishing TTBR0 and entering EL0t.
- Separately linked AArch64 `phoenix-init` conformance ELF plus canonical build and static
  inspection through the production loader contract.
- Opt-in loaded-init kernel variant and bounded QEMU protocol connecting the real DTB allocator,
  embedded ELF population, native stack, dynamic user tables, ownership-gated EL0 entry, and
  terminal `exit(42)`, with exact embedded-artifact inspection.
- Strict allocation-free `newc` initramfs validation, canonical lookup, deterministic host
  packaging of the first init ELF, and exact archive embedding in the loaded-init kernel.
- Fully prevalidated cross-page `copy_from_user`, retained loaded-init runtime ownership, bounded
  stdout syscall handling, and a userspace message/return-value conformance check.
- Fully prevalidated `copy_to_user`, a generation-safe fixed-capacity read-only initramfs file
  table, bounded `open`/`read`/`close` probes, and end-to-end initramfs file-I/O conformance logic.
- Allocator-owned mixed-level AArch64 kernel tables, page-granular text/rodata/data permission
  overrides, a QEMU `virt` RAM direct map and PL011 device page, one-time TTBR1 publication with
  barriers/TLB invalidation, and a bounded Runtime Pending final-map probe.
- Loaded-init integration of the retained final TTBR1 owner, explicit bootstrap-memory capability
  revocation, ownership-checked post-switch user-frame copies, and ordered QEMU progress evidence.
- One linker-reserved boot-stack guard page, explicit direct-map holes, final-plan absence checks,
  and static artifact validation of guard size and placement.
- Architecture-neutral process IDs, strict lifecycle transitions, generation-safe bounded process
  ownership, reap semantics, and loaded-init ready/running/exited integration.
- Nonzero 8-bit AArch64 ASIDs, monotonic no-reuse allocation, checked TTBR0 bit composition, and
  ASID-1 activation for both the static and loaded EL0 paths.
- Project, Agent, Roadmap, and kernel capability documentation.
