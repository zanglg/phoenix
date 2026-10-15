# 0.1.0-pre.1 validation report

All technical gates below passed on clean source commit
`7abc066470377d66994855ff109c39c6369ace3a`, version `0.1.0-pre.1`, on 2026-10-13.
The evidence-only follow-up commit adds this report and transcripts; its kernel,
userspace, manifests and build tooling are unchanged. A package rebuilt from that
follow-up records its own Git identity and therefore has different identity bytes.
The hashes retained here identify the exact tested source package.

## Environment and commands

- Host: x86_64 Linux, checkout `/workspace/phoenix`.
- Compiler: Rust 1.99.0 (`b940084d7`, 2026-09-28), stable, LLVM 23.1.1.
- Independent MSRV check: Rust 1.95.0 (`59807616e`, 2026-04-14).
- Emulator: QEMU 10.0.13, pinned `virt-9.1`, Cortex-A72, TCG, GICv3,
  512 MiB, one CPU, highmem off. Direct EL1 case turns virtualization off.
- Profile: debug, AArch64 soft-float, forced frame pointers, incremental disabled
  for kernel and userspace artifact builds.
- Tool paths were supplied by sourcing `/workspace/phoenix-env.sh`; ordinary
  installations can run the same commands from PATH. No project dependency was added.

| Command | Result | Retained transcript |
| --- | --- | --- |
| `cargo xtask ci` | Pass: formatting, target checks, Clippy, 232 kernel tests, 16 tooling tests and all six image inspections | [CI](candidate-ci.log) |
| `cargo xtask test-boot` | Pass | [boot](candidate-boot.log) |
| `cargo xtask test-memory` | Pass: three ranges, both endpoints, exhaustion and restoration | [memory](candidate-memory.log) |
| `cargo xtask test-kernel-map` | Pass: current final map, heap and stack integration | [map](candidate-map.log) |
| `cargo xtask test-el0` | Pass | [EL0](candidate-el0.log) |
| `cargo xtask test-init` | Pass: current GIC publication, native stack and file I/O | [init](candidate-init.log) |
| `cargo xtask test-runtime` | Pass: thirteen isolated guest runs, each exit 0 | [runtime suite](candidate-runtime.log) |
| `cargo xtask test-system` | Pass: eleven child reaps, nine faults, exact frame restoration, console input and guest exit 0 | [system](candidate-system.log) |
| `RUSTUP_TOOLCHAIN=1.95.0 cargo xtask check` | Pass: target and host | [MSRV check](candidate-msrv-check.log) |
| `RUSTUP_TOOLCHAIN=1.95.0 cargo xtask test` | Pass: 232 + 16 tests | [MSRV tests](candidate-msrv-test.log) |
| `cargo xtask package` | Pass: seven artifacts, provenance and checksums | [package](candidate-package.log) |

There were nineteen successful emulator launches. Every bounded run completed
within ten seconds. The five legacy probes were terminated by their harness after
the exact terminal marker; all thirteen runtime cases and the system exited QEMU
with guest-requested status 0. Raw serial logs are retained as `qemu-*.log` here;
the command transcripts also contain launch arguments, outputs and outcomes.

## Negative and lifetime evidence

The runtime suite directly observes EL2/EL1 entry, zeroed BSS, SP/VBAR, deliberate
panic, all 31 saved/restored registers and control state, boot/heap/stack guard
faults, RO/NX faults, IRQ deferral under lock, real timer acknowledgement/EOI/return,
preempted workers, sleep, exit, stack reclamation and fatal exception reporting.

The system validates successful and failing cross-page user copies, failed-read
offset rollback, archive/tmp I/O, all three file-resource limits, two normal child
exits, and nine contained faults: kernel read, null translation, stack NX, illegal
instruction, bad SVC, code write, low MMIO, low RAM and user-stack guard. Every
child restores frame accounting; root readback and ASID 2 reuse are checked in the
kernel. The structured ring reports its expected overwrite (`events: lost=1`).

The panic's four return addresses were resolved with the exact retained ELF and
`cargo xtask symbolize`, subtracting four bytes for call sites. The result identifies
`rust_begin_unwind`, `panic_fmt`, `runtime_probe::select` and `kernel_main`:
[symbolization](candidate-symbolized.log).

## Reproduction

The first package was copied aside. Only `target/phoenix/build/system` and
`target/phoenix/build/system-users` were removed with `cargo clean --target-dir`;
`cargo xtask package` then rebuilt them. All seven artifact byte sequences matched,
including the full linker map. [REPRODUCTION.txt](REPRODUCTION.txt) gives the hashes.
[PROVENANCE.txt](PROVENANCE.txt) and [SHA256SUMS](SHA256SUMS) identify the tested package.
The reproducibility scope is the same source/tools/options/checkout path, as defined
in [RELEASE.md](../RELEASE.md). See the ledger for the retained initial map mismatch
and its incremental-compilation correction.

## Review boundary

The current contract and unsafe review are [0.1-CONTRACT.md](../0.1-CONTRACT.md)
and [UNSAFE_REVIEW.md](../UNSAFE_REVIEW.md). Kernel-worker/heap and user-system
integration images remain separate. There is no concurrent userspace scheduling,
SMP, emergency exception stack, demand paging, TLS, networking, persistent storage,
POSIX/Linux compatibility or real-hardware support. Release-optimized artifacts
were not tested; the supported package is debug. No independent security audit
is claimed.

The candidate is technically reviewable. Final `0.1.0` promotion and tag still
require explicit owner acceptance under AGENTS.md and ROADMAP.md.
