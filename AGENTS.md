# Phoenix Agent Contract

## Mission and current boundary

Phoenix is an experimental Unix-like Rust kernel. The current development target is AArch64 on
QEMU `virt`. The Cargo version remains 0.0.0 throughout the linear pre-release construction path.
Follow the Roadmap in dependency order. A runtime-pending item may remain open while work moves
to the next explicitly host-verifiable item, but only when that work does not depend on an
unobserved hardware result. Never convert static evidence into a runtime support claim.

RISC-V 64 and x86_64 are deferred ports. The source tree must not prevent them, but agents must
not create speculative implementations for them.

## Read before changing files

1. Read this file.
2. Read `ROADMAP.md` and identify the first incomplete section.
3. Check `FEATURES.md`; a `Captured` or `Deferred` entry is not implementation authorization.
4. Inspect the working tree and preserve unrelated user changes.

## Repository map

- `Cargo.toml`: Phoenix version and kernel workspace policy.
- `kernel/`: freestanding kernel library, AArch64 bootstrap, and image linker layout.
- `userspace/init/`: first independently linked native AArch64 ELF conformance program.
- `xtask/`: standalone host-side development tool.
- `.cargo/lsp.toml`: rust-analyzer's default kernel target.
- `kernel/src/arch/aarch64/boot.S`: first AArch64 bootstrap implementation.
- `docs/BOOT.md`: current boot contract and runtime-validation boundary.
- `docs/AARCH64_PAGING.md`: temporary and final page-table model, permissions, and remaining work.
- `docs/AARCH64_USER_PAGE_TABLES.md`: user-table topology, ownership, materialization, and activation boundary.
- `docs/QEMU_VIRT_BOOTSTRAP_MEMORY.md`: checked target access to private RAM under the temporary TTBR1 alias.
- `docs/BOOT_MEMORY_MAP.md`: DTB-to-allocator construction and mandatory boot reservations.
- `docs/BOOT_MEMORY_PROBE.md`: opt-in end-to-end boot memory integration and runtime boundary.
- `docs/AARCH64_EXCEPTIONS.md`: vector, frame, syndrome, and remaining runtime-handler contract.
- `docs/USER_ADDRESS_SPACE.md`: EL0 virtual region, permissions, mapping plan, and guarded stack.
- `docs/ELF_LOADER.md`: strict initial executable subset and transactional loading requirements.
- `docs/PROCESS_IMAGE.md`: executable/stack page planning and transactional frame ownership.
- `docs/INITIAL_USER_STACK.md`: native argc/argv/envp/auxv layout and process-image integration.
- `docs/FIRST_USER_PROGRAM.md`: standalone init ELF contract and static validation boundary.
- `docs/LOADED_INIT_PROBE.md`: end-to-end embedded ELF loading, ownership, entry, and runtime boundary.
- `docs/NATIVE_ABI.md`: unstable native syscall numbers, AArch64 registers, and return encoding.
- `docs/EL0_PROBE.md`: opt-in static first-user-mode bridge and exact validation boundary.
- `docs/QEMU.md`: pinned virtual board, runtime commands, timeout, sentinels, and evidence policy.
- `docs/RUNTIME_VALIDATION.md`: hardware-dependent checks deferred until an emulator is available.
- `FEATURES.md`: comprehensive capability catalog.
- `ROADMAP.md`: ordered path to the first formal release.
- `CHANGELOG.md`: completed changes.

## Architecture boundaries

- Keep generic kernel mechanisms independent of a concrete instruction set.
- Place CPU registers, exception entry, page-table format, context switching, and inline assembly
  under the selected architecture implementation.
- Place machine discovery, MMIO addresses, interrupt controllers, firmware, and board devices
  under the selected platform implementation.
- Do not put architecture-specific types in generic interfaces without a documented reason.
- Do not create an abstraction or crate only because a future port might need it. Extract a
  narrow interface when a real boundary is known.

## Safety rules

- Every `unsafe` block must have an adjacent `// SAFETY:` comment describing its invariants.
- Keep unsafe regions as small as practical.
- Do not broaden an unsafe region to silence a compiler error.
- Changes involving atomics, locks, page tables, DMA, interrupt context, or context switching must
  document the relevant ownership and ordering invariants.

## Canonical validation

```bash
cargo xtask doctor
cargo xtask fmt
cargo xtask check
cargo xtask lint
cargo xtask test
cargo xtask build
cargo xtask inspect
cargo xtask build-init
cargo xtask inspect-init
cargo xtask qemu-command
cargo xtask run
cargo xtask test-boot
cargo xtask test-memory
cargo xtask test-el0
cargo xtask test-init
cargo xtask ci
```

Run `cargo xtask ci` before reporting completion. Never state that an unexecuted check passed.
QEMU is optional in 0.0.0. Static artifact inspection is required. `run`, `test-boot`,
`test-memory`, `test-el0`, and `test-init` are runtime-only commands; do not install or invoke QEMU
in an environment where it is unavailable.

Use these validation states consistently:

- **Host Tested**: behavior executed in host-side tests.
- **Cross Compiled**: compiled for the configured bare-metal target.
- **ELF Inspected**: artifact structure checked without execution.
- **Runtime Pending**: implementation exists but has not run on the target platform.
- **Runtime Verified**: the documented target command and observed evidence passed.

Record every Runtime Pending claim in `docs/RUNTIME_VALIDATION.md`.

## Feature and documentation policy

- Put newly discovered kernel capabilities in `FEATURES.md`.
- Keep unscheduled work as `Captured` or `Deferred`; do not create placeholder implementations.
- Add work to `ROADMAP.md` only when it belongs on the committed path to 0.1.0.
- Update `CHANGELOG.md` only for completed changes.
- Do not leave long-lived source TODOs; record them in `FEATURES.md` with context.
- Update README support claims whenever a target becomes buildable or bootable.

## Prohibited changes

- Do not add dependencies, crates, architectures, or platforms silently.
- Do not perform unrelated refactors.
- Do not copy code under an incompatible license.
- Do not delete or weaken failing checks to obtain a green result.
- Do not change the page size, boot contract, ABI, or memory model without recording the decision.

## Definition of Done

- The requested scope and its non-goals are respected.
- Formatting, target checks, Clippy, and available tests pass.
- New unsafe code has documented invariants.
- `FEATURES.md`, `ROADMAP.md`, `CHANGELOG.md`, and README agree with the implementation.
- The final report lists commands actually run, their results, omissions, and the next scoped step.
