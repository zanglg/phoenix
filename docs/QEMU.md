# QEMU AArch64 Integration Testing

This document describes the QEMU `virt` runner and bounded boot test. The recorded first
validation used QEMU 9.1.3 on an Apple Silicon host; its transcripts are retained under the
corresponding `target/phoenix/.../qemu-*.log` paths.

## Platform choice

QEMU `virt` is the primary Phoenix development board. It is intentionally not a model of one
physical board, supports modern AArch64 CPUs and common virtual devices, and avoids coupling the
first kernel port to vendor hardware. QEMU documents RAM at `0x40000000` as stable; other device
addresses must normally come from its generated DTB.

The test baseline is pinned to:

| Setting | Value | Reason |
| --- | --- | --- |
| Machine | `virt-9.1` | Versioned machine behavior rather than the moving `virt` alias |
| Virtualization | enabled | Exercise Phoenix's supported EL2-to-EL1 path |
| GIC | version 3 | GICv3 physical timer delivery is exercised by test-runtime |
| CPU | `cortex-a72` | Explicit 64-bit CPU model supported by the pinned board |
| Accelerator | TCG | Works independently of the host CPU architecture |
| RAM | 512 MiB | Fits the temporary RAM block and keeps tests modest |
| CPUs | 1 | SMP remains deferred |
| High memory | disabled | Keeps the initial physical topology below 4 GiB |
| Console | PL011 on stdio | Matches the early polled console |

The official board reference is
[`virt` generic virtual platform](https://qemu.readthedocs.io/en/master/system/arm/virt.html).
QEMU's loader treats a non-ELF `-kernel` input as an AArch64 image, falls back to a raw binary when
there is no Image header, loads it 512 KiB above RAM, and passes the generated DTB address in `x0`.
Those behaviors match Phoenix's raw image and bootstrap contract. Direct boot was observed with
the pinned command; untested failure paths remain Runtime Pending.

## Commands

Print the exact command without requiring QEMU:

```bash
cargo xtask qemu-command
```

Build, statically inspect, and run interactively:

```bash
cargo xtask run
```

Build, inspect, then execute the bounded integration test:

```bash
cargo xtask test-boot
```

Build the opt-in first-user-mode image and require its final EL0 marker:

```bash
cargo xtask test-el0
```

Build the opt-in DTB, allocator, and RAM-access integration image and require its final marker:

```bash
cargo xtask test-memory
```

Build the final permission-separated TTBR1 image and require output after the live table switch:

```bash
cargo xtask test-kernel-map
```

Build the standalone init, embed it in the dynamic loader path, and require its stack/file-I/O
validated terminal marker:

```bash
cargo xtask test-init
```

`run`, `test-boot`, `test-memory`, `test-kernel-map`, `test-el0`, and `test-init` first verify that
`qemu-system-aarch64`, the pinned machine, and the CPU model exist. They refuse to substitute
another machine silently. The normal emulator-free `cargo xtask ci` command never launches QEMU.

Static inspection also requires each image variant's own terminal sentinel to be embedded in its
ELF. Both final-map variants must contain `PHOENIX_GIC_PREPARED` and
`PHOENIX_IRQ_RUNTIME_PUBLISHED`; the standalone final-map image must additionally contain
`PHOENIX_KERNEL_HEAP_OK` and `PHOENIX_KERNEL_STACK_OK`. EL0-capable variants must contain
`PHOENIX_USER_FAULT` and their matching failure sentinel. These checks prevent a default image or
incomplete path built under the wrong feature set from masquerading as a focused probe artifact.

## Boot-test protocol

The integration test captures both QEMU output streams and waits at most 10 seconds. The first
terminal sentinel determines the result:

- `PHOENIX_BOOT_OK`: pass;
- `PHOENIX_PANIC`: fail immediately;
- `PHOENIX_EXCEPTION`: fail immediately and retain the register report;
- `PHOENIX_MEMORY_OK`: pass for `test-memory` only;
- `PHOENIX_KERNEL_HEAP_OK`: required progress after TTBR1 publication and bounded heap checks;
- `PHOENIX_KERNEL_STACK_OK`: required progress after bounded live stack publication, alias
  round-trip, retirement, scrub, and ownership restoration;
- `PHOENIX_GIC_PREPARED`: required progress after final TTBR1 publication, finite-budget GICv3
  preparation with Group-1 delivery closed;
- `PHOENIX_IRQ_RUNTIME_PUBLISHED`: required progress after physical-timer disable/observation and
  pinned runtime/IRQ-lookup publication with timer signaling and `DAIF.I` still closed;
- `PHOENIX_KERNEL_MAP_OK`: pass for `test-kernel-map` only, after ordered
  `PHOENIX_KERNEL_MAP_ENTER`, `PHOENIX_GIC_PREPARED`, `PHOENIX_IRQ_RUNTIME_PUBLISHED`,
  `PHOENIX_KERNEL_HEAP_OK`, and `PHOENIX_KERNEL_STACK_OK`;
- `PHOENIX_KERNEL_MAP_OK` without all earlier required markers in order: protocol fail;
- `PHOENIX_EL0_OK`: pass for `test-el0` only;
- `PHOENIX_EL0_FAIL`: fail `test-el0` immediately;
- `PHOENIX_USER_FAULT`: diagnostic prelude only; an EL0 variant must then emit its own failure
  sentinel, which fails that test;
- `PHOENIX_INIT_OK`: pass for `test-init` only;
- `PHOENIX_INIT_FAIL`: fail `test-init` immediately;
- `PHOENIX_INIT_OK` without the earlier ordered final-map enter, GIC-prepared, IRQ-runtime-
  published, and success markers, EL0 enter marker, exact greeting, and `etc/motd` output:
  protocol fail;
- QEMU exit before the required terminal sentinel: fail;
- timeout: fail and terminate QEMU;
- more than 1 MiB without a sentinel: fail and terminate QEMU;
- output read error: fail.

Because the kernel currently halts after output, the harness terminates QEMU after observing a
terminal sentinel. It writes boot output to
`target/phoenix/aarch64-unknown-none-softfloat/debug/qemu-boot.log`, memory-probe output to the
adjacent `qemu-memory.log`, final-map output to `qemu-kernel-map.log`, and EL0 output to
`qemu-el0.log`. It emits one stable summary such as `QEMU_TEST_RESULT=pass`, `probe-failure`,
`protocol-failure`, `panic`, `timeout`, or `early-exit`. Loaded-init output is written to
`qemu-init.log`. A missing emulator is an error for runtime commands, never a skipped or passing
test. Focused probes ignore the intermediate `PHOENIX_BOOT_OK` marker, so none can pass before its
own path completes.

## Recorded validation run

The first recorded run used QEMU 9.1.3 (`v9.1.3`), `virt-9.1`, `cortex-a72`, TCG, 512 MiB RAM,
one CPU, and the debug profile on source commit `8792647e370e` plus uncommitted validation changes.
With `/Volumes/workspace/qemu/monterey-aarch64/build` prepended to `PATH`, these bounded commands
all returned `QEMU_TEST_RESULT=pass` within their ten-second limit:

- `cargo xtask test-boot` — `PHOENIX_BOOT_OK`;
- `cargo xtask test-memory` — DTB size 1,048,576, boot CPU 0, 130,530 free frames, and
  `PHOENIX_MEMORY_OK`;
- `cargo xtask test-kernel-map` — root `0x40000000`, five tables, 767 leaves, and ordered
  `PHOENIX_KERNEL_MAP_ENTER`/`PHOENIX_KERNEL_MAP_OK`;
- `cargo xtask test-el0` — `PHOENIX_EL0_ENTER`/`PHOENIX_EL0_OK`;
- `cargo xtask test-init` — ordered final-map and EL0 markers, exact two-line init output, and
  `PHOENIX_INIT_OK`.

The original 512 KiB construction stack overflowed during the first final-map probe. QEMU's
exception trace showed an outer approximately 305 KiB frame plus a nested approximately 530 KiB
planner frame. The recorded fix increases only the construction stack to 1 MiB and uses in-place
transactional planner updates; it does not change the 4 KiB granule, QEMU RAM size, or address
layout.

That recorded final-map run predates the bounded GIC preparation, heap, and runtime-stack paths,
did not emit `PHOENIX_GIC_PREPARED`, `PHOENIX_IRQ_RUNTIME_PUBLISHED`,
`PHOENIX_KERNEL_HEAP_OK`, or `PHOENIX_KERNEL_STACK_OK`, and used five table frames before the
runtime-stack arena gained two prelinked empty tables and GIC mapping gained another L3 table with
48 leaves. It remains evidence for historical final TTBR1 publication, not for the current
eight-table GIC/timer-runtime-published/heap/stack protocol, `RUN-KHEAP-001`, `RUN-IRQ-001`, or runtime-stack publication;
those boundaries remain Runtime Pending until a new run is captured.

## How to report a run

Run `cargo xtask doctor`, record `qemu-system-aarch64 --version`, then run each focused command in
dependency order, ending with `cargo xtask test-init`. Preserve:

1. the Phoenix commit hash;
2. the complete `QEMU_TEST_*` lines;
3. the corresponding `qemu-boot.log`, `qemu-memory.log`, `qemu-kernel-map.log`, `qemu-el0.log`, or
   `qemu-init.log`;
4. whether the host is x86_64 or AArch64;
5. any local command changes.

Do not update the runtime ledger to Runtime Verified solely because the success sentinel appears.
First confirm the build identity, DTB argument, expected EL transition, higher-half entry, and
process termination behavior against `docs/BOOT.md`.

## Validation of the harness

Host tests cover exact command construction, shell-safe display, exact machine-name discovery,
sentinel detection across arbitrary read boundaries, per-probe terminal-marker selection,
user-fault-to-probe-failure classification, and the rule that the earlier of competing sentinels
wins. The recorded validation run also exercised the QEMU child-process, timeout-bound capture,
sentinel detection, logging, and termination path.

## TODO

- retain the first-run logs outside generated build output before cleaning artifacts;
- add a deliberate-panic image and a separate positive panic-path test;
- add a deterministic guest-driven exit device after platform discovery is available;
- dump and retain the generated DTB for compatibility fixtures;
- add GDB attach support using the same pinned board configuration;
- add deliberate user/kernel fault images and validate their distinct terminal protocols;
- decide when a QEMU-enabled CI runner becomes required.

## Skipped work

KVM/HVF acceleration, multiple CPUs, real boards, UEFI, networking, storage, graphical devices,
secure-world firmware, migration, and performance benchmarking are deliberately excluded from the
current runner. QEMU execution is also deliberately excluded from emulator-free CI rather than
represented as a passing test.

## Current 0.1 extensions

The original smoke protocols above are retained. Current results use QEMU 10.0.13
on x86_64 Linux with the same pinned `virt-9.1` board; see the runtime ledger for
source identity and retained logs. The earlier construction-state remarks about
unobserved IRQs or pending negative tests are superseded by that ledger.

`cargo xtask test-runtime` builds the final-map image and executes thirteen isolated
cases: state (EL2), state (direct EL1), panic, registers, boot guard, readonly, NX,
timer/IRQ lock, threads, heap lower guard, heap upper guard mapped stack guards and a fatal exception report.
Each command line includes `-semihosting-config enable=on,target=native` and one
explicit `phoenix.probe=` boot argument. Success requires the ordered case-specific
proof, exactly one `PHOENIX_TEST_OK` line and QEMU exit status 0, inside ten seconds.
The panic case accepts only the deliberate panic with a real trace. Other cases
reject panic/fatal-exception output. QEMU is killed and reaped on timeout/error.

`cargo xtask test-system` builds three independent user ELFs and a deterministic
archive. It boots five scenarios: LF, CR and CRLF input; invalid input (expected
exit 1); and a temporary image whose nine fault entries all begin with an illegal
instruction. The latter exits 0 but must be rejected by the system oracle.

The oracle obtains fault PCs from the exact stripped ELF's exported symbols and
requires each kind, PC, FAR and full ESR in process-generation order, followed by
its resource-restoration record. The fixed NX stack address is checked directly;
SVC PC is the following instruction. Expected successful runs have twelve reaps,
nine faults, file/console completion and `PHOENIX_SYSTEM_EXIT status=0 fault=false`.
Conformance counts belong to xtask; the kernel reports generic init outcomes.

`cargo xtask test-hardening` enables `phoenix.system-stress`. Boot diagnostics run
through the real loader with each insufficient frame budget (0–9), every one of
21 backend-operation failures, and all 254 unused ASIDs reserved. Exact rollback
and a clean retry are required. The stress init then executes 64 rounds plus a
final normal child, producing 705 reaps and 576 individually checked faults.
Child stacks must be scrubbed, child descriptors may not leak across lifetimes,
and a parent descriptor/offset survives. Every reap checks the whole allocator
free set, ASID capacity, process slots and stale PID rejection. This run has a
30-second budget; other cases retain ten seconds and the 1 MiB output limit.

`run-system` supplies no test argument or semihosting switch; interactive init
reports its status then halts. CR/LF acceptance is in conformance init; the native
console remains a raw nonblocking byte stream.

Logs are `qemu-<case>.log`, including `qemu-system-{lf,cr,crlf,bad-input,wrong-fault,stress}.log`.
The harness prints command lines, paths and results. GitHub CI runs all commands
with Rust 1.99.0 and QEMU 10.0.13 on Debian trixie, and retains serial/package
artifacts for 30 days. The tested debug kernel uses forced frame pointers.

## 0.2 development scheduling suite

`cargo xtask test-scheduling` builds the system image and four independent user
ELFs (init/child/fault/cooperative), then runs five scenarios with 30-second
budgets and no console input:

| Boot argument | Evidence |
| --- | --- |
| `phoenix.system-cooperative` | IRQ-masked baseline: 64 rounds, 257 reaps, 128 exact faults, register/stack/FD isolation and both wait orderings |
| `phoenix.system-cooperative-preempt` | Same lifecycle workload with real EL0 timer handoffs; peers poll actual shared-file progress |
| `phoenix.system-preempt` | Two non-yielding register-check loops, an infinite child, multiple ticks in each context, real handoffs and successful init exit |
| `phoenix.system-preempt-off` | Same image/output with timer disabled: guest exits 0 but preemption oracle must reject it |
| `phoenix.system-sleep` | Sixteen overlapping sleep rounds, 32 paired deadlines/wakes, nonzero idle IRQ count, fixed idle stack top and exact reaps |

The no-timer case prints `QEMU_PREEMPTION_NEGATIVE_REJECTED=true`. Other positive
cases require guest exit 0 and their complete independent host oracle. Sleep
records include the measured counter frequency, initial counter, requested ns,
deadline and wake counter; the oracle recomputes rounded-up durations and rejects
early, missing or duplicate wakes. Logs are `qemu-system-<scenario>.log` under the
standard target directory. The full development suite now has 29 QEMU launches.

Normal/interactive/stress system boots enable IRQs only in EL0. All kernel
dispatch remains IRQ-masked. If no user can run, a naked EL1h WFI loop uses the
permanently retained exception stack; only this exact PC range may receive an
EL1 scheduler IRQ. The design and invariant review are in 0.2-SCHEDULING.md and
UNSAFE_REVIEW.md. The released 0.1 tag retains its original synchronous boundary.
