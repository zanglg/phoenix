# Phoenix

Phoenix is an experimental Unix-like kernel written in Rust. Development starts with AArch64
on QEMU `virt`; RISC-V 64 and x86_64 are future ports, not current claims of support.

## Current status

The project version is **0.0.0**. The first AArch64 bootstrap is integrated, with a deliberately
narrow support claim:

- the Rust workspace follows stable Rust and declares Rust 1.95 as its minimum version;
- the `no_std` kernel links as an AArch64 ELF and can be converted to a raw image;
- static inspection verifies its machine type, entry, load addresses, page-table alignment,
  BSS bounds, and 64 KiB boot stack;
- rust-analyzer can use the configured kernel target from an x86_64 development host;
- host-side unit tests and validation are available through `cargo xtask`;
- a bounded QEMU runner and boot-test harness are implemented and host tested;
- runtime boot and serial output have **not** been validated because the current development
  environment has no QEMU.

[`boot.S`](kernel/src/arch/aarch64/boot.S) is Phoenix's first AArch64 bootstrap implementation.
The boot contract and current validation boundary are recorded in [`docs/BOOT.md`](docs/BOOT.md).

## Development targets

| Architecture | Rust target | Initial platform | Status |
| --- | --- | --- | --- |
| AArch64 | `aarch64-unknown-none-softfloat` | QEMU `virt` | Primary development target |
| RISC-V 64 | `riscv64gc-unknown-none-elf` | QEMU `virt` | Deferred |
| x86_64 | `x86_64-unknown-none` | QEMU `q35` | Deferred |

Only the first row is configured and checked in version 0.0.0.

## Quick start

Install [rustup](https://rustup.rs/), enter the repository, and run:

```bash
cargo xtask doctor
cargo xtask ci
```

The project toolchain follows the current stable Rust release and installs the AArch64 target,
rustfmt, Clippy, rust-analyzer, `rust-src`, and LLVM tools. QEMU is reported by `doctor` and
remains optional for emulator-free validation. Runtime commands fail clearly when it is absent
rather than treating the test as skipped.

Canonical commands are:

```bash
cargo xtask doctor
cargo xtask fmt
cargo xtask check
cargo xtask lint
cargo xtask test
cargo xtask build
cargo xtask inspect
cargo xtask qemu-command
cargo xtask run
cargo xtask test-boot
cargo xtask ci
```

`build` produces an ELF, raw image, and linker map under `target/`; `inspect` validates those
artifacts without executing them. `qemu-command` prints the pinned command without starting an
emulator. `run` is interactive and `test-boot` is bounded; neither is part of emulator-free `ci`.
See [`docs/QEMU.md`](docs/QEMU.md) for their exact validation boundary.

## LSP target

The default rust-analyzer architecture is controlled by one file:

```toml
# .cargo/lsp.toml
[build]
target = "aarch64-unknown-none-softfloat"
```

VS Code is configured to pass that file to rust-analyzer. Other editors should set the
rust-analyzer `cargo.configPath` option to `.cargo/lsp.toml`.

When a future port exists, install its Rust target and change this one `target` value. For
example:

```toml
target = "riscv64gc-unknown-none-elf"
```

This LSP setting does not change the host architecture used to run xtask. Rust-analyzer covers
Rust source; assembly such as `boot.S` requires separate editor support.

## Project records

- [`FEATURES.md`](FEATURES.md) is the long-lived kernel capability catalog. Entries may remain
  deferred and unscheduled.
- [`ROADMAP.md`](ROADMAP.md) contains the single ordered path to the first release.
- [`CHANGELOG.md`](CHANGELOG.md) records completed, visible changes.
- [`AGENTS.md`](AGENTS.md) defines the repository rules for coding agents.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) defines the `dev` to `main` contribution flow.
- [`docs/RUNTIME_VALIDATION.md`](docs/RUNTIME_VALIDATION.md) holds checks that require QEMU or
  later target hardware.

Recording a feature does not authorize its implementation. It must first be promoted into the
ordered Roadmap, and only the first incomplete section is active.

## Versioning

Phoenix reserves SemVer-compatible version numbers for releases. The project version has one
source of truth: `workspace.package.version` in [`Cargo.toml`](Cargo.toml).

During the long pre-release construction period, the Cargo version remains `0.0.0`, the ordered
Roadmap describes progress, and Git commit hashes identify exact states. Development steps do not
create releases or tags. `CHANGELOG.md` remains under `Unreleased` during this period.

The first formal release is `0.1.0`, created only after the release boundary in `ROADMAP.md` is
satisfied. Only releases receive annotated `vX.Y.Z` tags, and strict SemVer compatibility meaning
begins when the corresponding public contract is documented. ABI-specific versions will be
introduced only when those ABIs actually exist.

The workspace declares Rust 1.95 as its minimum supported Rust version (MSRV). Development follows
`stable`; the exact compiler identity will be embedded in release provenance. If a stable update
causes a kernel regression, the toolchain may be temporarily pinned with the reason documented.

## License

Phoenix is licensed under either Apache License 2.0 or the MIT license, at your option.
