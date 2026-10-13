# Runtime validation ledger

The 0.1 support boundary is defined in [0.1-CONTRACT.md](0.1-CONTRACT.md).
Host Tested, Cross Compiled and ELF Inspected evidence is kept distinct from
Runtime Verified behavior. Current evidence uses QEMU 10.0.13 on x86_64 Linux,
`virt-9.1,virtualization=on,gic-version=3,highmem=off`, Cortex-A72, TCG, 512 MiB
and one CPU. The direct-EL1 case changes only virtualization to off.

The current stable compiler is Rust 1.99.0. Rust 1.95.0 independently passes target
checking and all host tests. The supported artifact profile is debug with forced
frame pointers and incremental compilation disabled. The environment installs
Rust and QEMU locally under `/workspace/toolchains`; the user explicitly approved
installing and executing QEMU as an exception to the absent-emulator agent rule.

[validation/REPORT.md](validation/REPORT.md) records exact source identity, tool
versions, commands, outcomes and reproduction hashes. Retained raw logs live beside
that report. Each new harness requires ordered proof and a clean guest exit within
ten seconds. Legacy focused smoke tests stop QEMU after their terminal marker.

## Current evidence

Every Runtime Verified row below applies only to the stated bounded test and board.
Malformed inputs and injected hardware/backend failures also have host coverage;
that coverage does not imply those failures were reproduced in actual hardware.

| ID | State | Observed evidence |
| --- | --- | --- |
| RUN-BOOT-001 | Runtime Verified | `test-boot` reaches the build identity and PHOENIX_BOOT_OK |
| RUN-BOOT-002 | Runtime Verified | `test-runtime` state/EL1 cases observe EL2 and EL1 entry, then EL1 Rust |
| RUN-BOOT-003 | Runtime Verified | Higher-half Rust and stack continue across bootstrap and final TTBR1 |
| RUN-BOOT-004 | Runtime Verified | Full BSS checked before singleton writes; bounded/aligned SP and exact VBAR asserted |
| RUN-CONSOLE-001 | Runtime Verified | PL011 output/flush, stdin input and exact system completion |
| RUN-PANIC-001 | Runtime Verified | Intentional panic, bounded nonempty trace, exact-ELF symbolization and guest exit |
| RUN-SMOKE-001 | Runtime Verified | All bounded harnesses complete; new protocols require exit status 0 |
| RUN-DTB-001 | Runtime Verified | Real boot DTB accepted with CPU, RAM and reservation ownership |
| RUN-MM-001 | Runtime Verified | DTB allocator owns and tests all three free-range endpoints, exhausts and restores them |
| RUN-MM-002 | Runtime Verified | First/last private pages clear/write/read; outside bootstrap window rejected before access |
| RUN-MM-003 | Runtime Verified | `test-memory` reports boundary/exhaustion restoration and PHOENIX_MEMORY_OK |
| RUN-KHEAP-001 | Runtime Verified | Guarded alias read/write, aligned allocation, byte/metadata failure and reuse; both guards fault |
| RUN-MMU-001 | Runtime Verified | Current eight-table final map; text write and data execution cause permission faults |
| RUN-MMU-002 | Runtime Verified | System EL0 low RAM and PL011 aliases cause translation faults after TTBR0 replacement |
| RUN-MMU-003 | Runtime Verified | Retained direct-map clear/read/write; unmanaged and cross-page requests rejected |
| RUN-ASID-001 | Runtime Verified | System reads back every TTBR0 root/ASID; ASID 2 reused across normal/faulting children after TLBI |
| RUN-KSTACK-001 | Runtime Verified | Controlled boot-guard access reports expected translation fault and recovers |
| RUN-KSTACK-002 | Runtime Verified | Both runtime guards fault while usable leaves remain mapped; two workers enter, preempt, sleep, exit and return 16 frames |
| RUN-EXC-001 | Runtime Verified | Fatal BRK reaches vector 4 with exact syndrome/PC and complete register report |
| RUN-EXC-002 | Runtime Verified | x0–x30, SP_EL0, status, PC, syndrome and ERET round trip; fault FAR checked separately |
| RUN-SYNC-001 | Runtime Verified | Timer IRQ stays pending while lock held; enters and acquires it after mask restoration |
| RUN-IRQ-001 | Runtime Verified | GIC publication/activation, exact PPI 30, dispatch, EOI, return and delivery shutdown |
| RUN-TIME-001 | Runtime Verified | Monotonic observations, bound one-shot delivery, 2 ms preemption and absolute thread sleep |
| RUN-UFAULT-001 | Runtime Verified | Nine faults terminate only the child, retain PC/address/syndrome and resume parent after reclaim |
| RUN-ELF-001 | Runtime Verified | Strict independently linked init/child/fault ELF images are populated, mapped and entered |
| RUN-STACK-001 | Runtime Verified | Focused init verifies native SP, argc/argv/envp, auxv and terminators |
| RUN-UCOPY-001 | Runtime Verified | Valid/invalid cross-page source/destination copies; failed destination leaves first page and offset unchanged |
| RUN-FS-001 | Runtime Verified | Archive and tmp read/write/EOF/close; descriptor, file-count and byte-capacity exhaustion/recovery |
| RUN-INIT-001 | Runtime Verified | Current loaded-init GIC/publication protocol and minimal system both pass |
| RUN-EL0-001 | Runtime Verified | Static first-EL0 fixture handles unknown syscall and exit; dynamic programs enter/return separately |
| RUN-ABI-001 | Runtime Verified | Calls 0–5, argument/result preservation, EFAULT/ENOSYS/EAGAIN/EMFILE/ENOSPC and parent restoration |

## Retained failure history

The earlier QEMU 9.1.3 environment and its pending checklist are preserved in
[INITIAL_RUNTIME_LEDGER.md](validation/INITIAL_RUNTIME_LEDGER.md). Those historical
statuses do not override current observations.

The current dev baseline (413f8a348210) initially timed out after PHOENIX_BOOT_OK
in the final-map probe. Exception tracing showed code corruption after debug
planner stack use exceeded the existing 1 MiB stack (outer approximately 307 kB
plus nested approximately 796 kB). Replacing private by-value plan updates with
in-place updates fixed the runtime failure without enlarging the stack. The larger
system image separately required distinguishing the permission window from its
RW/NX stack tail; both bounds remain enforced. A concise failed transcript is
retained in `validation/baseline-map-failure.log`.

The first artifact reproduction check found differing incremental object names
in the linker map while the ELF/raw image matched. Artifact builds now disable
incremental compilation. The repeated seven-file comparison includes the full map
and passes without filtering its contents.

## Remaining boundary

No active 0.1 runtime gate remains pending at the documented scope. SMP, nested
interrupts, concurrent userspace, optimized builds, demand paging, signal recovery,
new devices and real hardware remain unimplemented/deferred rather than verified.
Future changes to capacities, stack workloads or these execution modes must add
new ledger entries and fresh target evidence.
