# 0.1.0-pre.2 validation and evidence

The candidate fixes the review's fault-test false positive and terminal-input
failure, adds runtime CI, and hardens the current synchronous process model.
Its source is the commit selected by annotated tag `v0.1.0-pre.2`. Full Git
identity, clean/dirty state and tool identities are recorded in the retained
package's `PROVENANCE.txt`; `SHA256SUMS` identifies the exact artifacts.

The local preflight passed with Rust 1.99.0, LLVM 23.1.1 and QEMU 10.0.13 on
x86_64 Linux. Independent Rust 1.95.0 checks and host tests passed. Target runs
use `virt-9.1`, Cortex-A72, TCG, GICv3, 512 MiB and one CPU. The direct-EL1
case disables virtualization. Debug builds force frame pointers and disable
incremental compilation for kernel/userspace artifacts.

| Command | Coverage / local preflight result |
| --- | --- |
| `cargo xtask ci` | Formatting, target checks, Clippy, six image inspections, 232 kernel + 19 tooling tests |
| `cargo xtask test-boot` | Boot smoke passed |
| `cargo xtask test-memory` | Reservation-filtered allocator, endpoints and exhaustion/recovery passed |
| `cargo xtask test-kernel-map` | Final map, guarded heap/stack integration passed |
| `cargo xtask test-el0` | Returning syscall and EL0 exit passed |
| `cargo xtask test-init` | Dynamically loaded native init and file I/O passed |
| `cargo xtask test-runtime` | Thirteen isolated boot/exception/guard/IRQ/thread scenarios passed |
| `cargo xtask test-system` | Five input and fault-oracle scenarios passed |
| `cargo xtask test-hardening` | Target rollback/recovery and 705 child lifetimes passed |
| `RUSTUP_TOOLCHAIN=1.95.0 cargo xtask check` | Target/library/tooling checks passed |
| `RUSTUP_TOOLCHAIN=1.95.0 cargo xtask test` | All 251 host tests passed |
| `cargo xtask package` on dirty source | Expected rejection before building/copying |

There are 24 emulator launches in the full suite: five legacy smoke runs,
thirteen runtime scenarios, five system scenarios and one hardening run.
Legacy harnesses terminate QEMU after the terminal marker. Runtime tests and
positive system/stress tests require guest exit 0. Invalid input must exit 1.
The wrong-fault negative control exits 0 but must be rejected by the host oracle.
This rejection is the successful outcome of that control, not a guest test pass.
Each run has a ten-second budget except stress, which allows thirty seconds.

## Fault assertions and negative control

Each of the nine intended faults must match its process generation, classified
kind, actual ELF fault-site PC, FAR and full ESR. This checks read/write/execute,
translation level, invalid SVC and undefined instruction behavior. NX uses a
fixed mapped stack target; SVC saves the following PC. Missing, duplicate,
reordered or incorrect records fail, even when init reports completion.

The negative control patches only a temporary raw image: all ten embedded copies
of the fault ELF start with an undefined instruction. Nine identical faults then
reach normal init completion but the independent oracle rejects the transcript.
Unmodified ELF/raw image/package files are preserved.

## Resource recovery

Stress boot exercises the real loader with every incomplete allocation budget
(0–9 frames), failure at each of 21 private image/table memory operations, and
all 254 unused ASIDs held. Each error leaves exact allocator state and the parent
unchanged. Diagnostic ASIDs were never installed and are explicitly retired.
A clean retry precedes real user execution.

The stress init runs 64 rounds of two normal children and nine intentional faults,
then one final normal child: 705 reaps and 576 contained faults. Every reap verifies
the entire free-frame set, remaining ASIDs and process-slot count, and rejects the
stale PID. Normal children check scrubbed stack bytes and exhaust/leave open their
eight descriptors. Init retains an independent descriptor and offset across all
children, verifies full tmp-file contents after ENOSPC, and continues after missing
or malformed executable requests. No concurrent user scheduling is claimed.

## Candidate receipt and retention

Before tagging, repeat the canonical/static and complete target suite on the clean
candidate commit, package it, rebuild from fresh system/kernel build directories,
and compare all seven artifact byte sequences. Check all package hashes and verify
the tag's commit equals package provenance. Retain a receipt containing full source
identity, command outcomes, serial logs, MSRV results and reproduction hashes.

Evidence is kept outside the source commit to avoid changing the tested/tagged
identity while adding transcripts. GitHub CI uploads serial logs and the package
as `phoenix-runtime-<workflow source SHA>` for thirty days. Preserve a local copy
for longer retention. The previous [pre.1 report](REPORT.md) and committed logs
are historical; they do not identify this candidate's artifacts.

The current limits remain in [0.1-CONTRACT.md](../0.1-CONTRACT.md). Worker/heap and
user-system integrations are separate; the latter masks IRQs and cannot recover
control from an infinite-loop user program. The supported package is debug.
Formal `0.1.0` promotion still requires explicit owner acceptance after review.
