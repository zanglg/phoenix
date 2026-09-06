# QEMU AArch64 Integration Testing

This document describes the implemented QEMU `virt` runner and bounded boot test. The command has
not been executed in the current development environment because `qemu-system-aarch64` is absent.

## Platform choice

QEMU `virt` is the primary Phoenix development board. It is intentionally not a model of one
physical board, supports modern AArch64 CPUs and common virtual devices, and avoids coupling the
first kernel port to vendor hardware. QEMU documents RAM at `0x40000000` as stable; other device
addresses must normally come from its generated DTB.

The test baseline is pinned to:

| Setting | Value | Reason |
| --- | --- | --- |
| Machine | `virt-9.2` | Versioned machine behavior rather than the moving `virt` alias |
| Virtualization | enabled | Exercise Phoenix's supported EL2-to-EL1 path |
| GIC | version 3 | Initial interrupt-controller target, though interrupts are not enabled yet |
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
Those behaviors match Phoenix's raw image and bootstrap contract, but remain Runtime Pending until
observed with the pinned command.

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

`run`, `test-boot`, and `test-el0` first verify that `qemu-system-aarch64`, the pinned machine, and the CPU
model exist. They refuse to substitute another machine silently. The normal emulator-free
`cargo xtask ci` command never launches QEMU.

## Boot-test protocol

The integration test captures both QEMU output streams and waits at most 10 seconds. The first
terminal sentinel determines the result:

- `PHOENIX_BOOT_OK`: pass;
- `PHOENIX_PANIC`: fail immediately;
- `PHOENIX_EXCEPTION`: fail immediately and retain the register report;
- `PHOENIX_EL0_OK`: pass for `test-el0` only;
- `PHOENIX_EL0_FAIL`: fail `test-el0` immediately;
- QEMU exit before either sentinel: fail;
- timeout: fail and terminate QEMU;
- more than 1 MiB without a sentinel: fail and terminate QEMU;
- output read error: fail.

Because the kernel currently halts after output, the harness terminates QEMU after observing a
terminal sentinel. It writes boot output to
`target/phoenix/aarch64-unknown-none-softfloat/debug/qemu-boot.log` and EL0 output to the adjacent
`qemu-el0.log`. It emits one stable summary such as `QEMU_TEST_RESULT=pass`, `panic`, `timeout`, or
`early-exit`. A missing emulator is an error for runtime commands, never a skipped or passing test.
`test-el0` ignores the intermediate `PHOENIX_BOOT_OK` marker, so it cannot pass before the EL0
syscall path completes.

## How to report the first run

Run `cargo xtask doctor`, record `qemu-system-aarch64 --version`, then run
`cargo xtask test-boot`. Preserve:

1. the Phoenix commit hash;
2. the complete `QEMU_TEST_*` lines;
3. `qemu-boot.log`;
4. whether the host is x86_64 or AArch64;
5. any local command changes.

Do not update the runtime ledger to Runtime Verified solely because the success sentinel appears.
First confirm the build identity, DTB argument, expected EL transition, higher-half entry, and
process termination behavior against `docs/BOOT.md`.

## Validation of the harness

Host tests cover exact command construction, shell-safe display, exact machine-name discovery,
sentinel detection across arbitrary read boundaries, and the rule that the earlier of competing
sentinels wins. The QEMU child-process path itself has not executed locally.

## TODO

- record the first real QEMU version, command, serial transcript, and result;
- decide whether `virt-9.2` remains the long-lived baseline after that run;
- add a deliberate-panic image and a separate positive panic-path test;
- add a deterministic guest-driven exit device after platform discovery is available;
- dump and retain the generated DTB for compatibility fixtures;
- add GDB attach support using the same pinned board configuration;
- add tests for exceptions, page-table replacement, allocator integration, and EL0 entry;
- decide when a QEMU-enabled CI runner becomes required.

## Skipped work

KVM/HVF acceleration, multiple CPUs, real boards, UEFI, networking, storage, graphical devices,
secure-world firmware, migration, and performance benchmarking are deliberately excluded from the
current runner. QEMU execution is also deliberately excluded from emulator-free CI rather than
represented as a passing test.
