# Phoenix

Phoenix is an experimental Unix-like Rust kernel for AArch64 QEMU `virt`.
The 0.1 technical path now includes protected memory, exceptions, timer-driven
kernel threads, independent user processes, console I/O and an in-memory filesystem.
The first formal release is **0.1.0**. Development lives on `dev`;
`main` is the owner review boundary. The owner accepted this first release for
validation and tagging on `dev`; development continues there.
The active development version is **0.2.0-dev.0**, with cooperative user scheduling
and EL0 timer preemption; [the 0.2 design](docs/0.2-SCHEDULING.md) records the boundary.

## Build and run

Install rustup and QEMU with the AArch64 system emulator, then run:

```bash
cargo xtask doctor
cargo xtask ci
cargo xtask test-runtime
cargo xtask test-system
cargo xtask test-hardening
cargo xtask test-scheduling
cargo xtask run-system
```

At the interactive console, type `ok` followed by Enter. The conformance init
checks files and child processes, accepts LF, CR or CRLF, reports
`PHOENIX_SYSTEM_EXIT status=0 fault=false`, then halts.
It is a minimal native system, not an interactive shell. The automatic system
test supplies input and requires both ordered evidence and a clean guest exit.

The supported configuration is `virt-9.1`, Cortex-A72, GICv3, TCG, 512 MiB RAM,
one CPU and 4 KiB pages. The target is `aarch64-unknown-none-softfloat`.
The toolchain follows stable Rust; the declared MSRV is Rust 1.95. Exact tested
compiler and QEMU versions are recorded in the [runtime ledger](docs/RUNTIME_VALIDATION.md).

## Current capabilities

- EL2/EL1 boot, complete BSS validation, aligned stack and installed vectors;
- complete integer exception-frame preservation, bounded panic traces and offline symbols;
- DTB reservations, fallible frame allocation, permission-separated final TTBR1,
  guarded heap and stacks with transactional mapping/TLB maintenance;
- real GICv3 timer delivery, IRQ-safe locking, round-robin kernel threads,
  preemption, sleep, idle and stack reclamation in the focused runtime image;
- strict ELF loading, isolated user roots, checked cross-page copies,
  synchronous run and cooperative single-child spawn/wait/yield, fault containment,
  frame reclamation and ASID reuse;
- read-only initramfs, bounded writable `tmp/`, process-local file descriptors,
  nonblocking console input and polled output;
- structured diagnostic ring, resource-exhaustion tests and deterministic packaging.

The kernel-thread/heap image and user-system image are separate integration targets.
The development system schedules two user processes with a 2 ms timer quantum.
IRQs are enabled in EL0 and masked throughout kernel dispatch; syscalls complete
before switching. A non-yielding child cannot starve its runnable parent.
User sleep and the no-runnable-process idle path remain planned.
Limits, error behavior, syscall numbers and ownership rules are defined
in the [0.1 contract](docs/0.1-CONTRACT.md). Tests cover the stated bounded workloads.
SMP, FP/SIMD, networking, persistent storage, POSIX/Linux compatibility, real hardware,
RISC-V and x86_64 are deferred.

## Validation and artifacts

`cargo xtask ci` runs formatting, target checks, Clippy, host tests, and ELF/image
inspection for every configured variant. It does not require or invoke QEMU.
GitHub CI also runs a separate QEMU job with recorded Rust/emulator versions,
and retains serial logs and the exact system package as workflow artifacts.
The complete target suite is:

```bash
cargo xtask test-boot
cargo xtask test-memory
cargo xtask test-kernel-map
cargo xtask test-el0
cargo xtask test-init
cargo xtask test-runtime
cargo xtask test-system
cargo xtask test-hardening
cargo xtask test-scheduling
```

The first five commands retain focused boot protocols. `test-runtime` runs thirteen
isolated cases, including negative protection tests, and uses guest-driven exits.
`test-system` checks LF/CR/CRLF, invalid input and a wrong-fault negative control.
Its oracle requires the exact PC, FAR, ESR and kind for all nine user fault cases.
`test-hardening` injects loader failures, exhausts frame/ASID budgets, then runs
705 child lifetimes with exact resource restoration and 576 contained faults.
`test-scheduling` exercises 64 cooperative rounds, 257 child reaps and 128 faults,
both wait/completion orderings and complete preserved GPR/NZCV/stack/FD checks.
It also preempts two non-yielding contexts, checks all 31 GPRs/NZCV/private stacks,
and requires init to progress while its child loops forever. The same successful
guest output with timer preemption disabled must be rejected by the host oracle.
All emulator tests are bounded and retain logs under
`target/phoenix/aarch64-unknown-none-softfloat/debug/`.
See [QEMU.md](docs/QEMU.md) and the [validation ledger](docs/RUNTIME_VALIDATION.md).

```bash
cargo xtask package
cargo xtask symbolize <exact-kernel-ELF> <0xADDRESS>...
```

Packaging collects the system kernel ELF, raw image, linker map, userspace ELFs,
initramfs, provenance and SHA-256 checksums under `target/phoenix/packages/<version>/`.
It requires a clean worktree and rejects source changes during the build.
The supported package is the inspected debug profile; release optimization is not
part of the target-validation claim. Reproduction requirements are documented in
[RELEASE.md](docs/RELEASE.md).

`build`/`run` retain the small boot smoke image; use `build-system`/`run-system`
for the full minimal system. `build-init`/`inspect-init` inspect the standalone
first-user-program fixture. `cargo xtask help` lists all commands.

## Development records

- [ROADMAP.md](ROADMAP.md): ordered technical path and release acceptance gate.
- [FEATURES.md](FEATURES.md): implemented scope and unscheduled capabilities.
- [CHANGELOG.md](CHANGELOG.md): completed changes.
- [AGENTS.md](AGENTS.md): repository engineering contract.
- [CONTRIBUTING.md](CONTRIBUTING.md): `dev` to `main` review flow.
- [0.1-CONTRACT.md](docs/0.1-CONTRACT.md): current supported interfaces and limits.
- [UNSAFE_REVIEW.md](docs/UNSAFE_REVIEW.md): ownership and ordering review.

The rust-analyzer target lives in `.cargo/lsp.toml`. It changes editor checking,
not the architecture of the host running xtask.

The version has one source of truth in `workspace.package.version`. Construction
uses `0.0.0`; technically complete candidates use `0.1.0-pre.N`. A candidate is
promoted to `0.1.0` only after explicit owner acceptance of the reviewed result.
The accepted 0.1 release retains the bounded experimental contract; future
release acceptance is not inferred from passing automated tests.

Phoenix is licensed under MIT OR Apache-2.0.
