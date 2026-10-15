# Runtime validation ledger

The formal 0.1 support boundary is defined in [0.1-CONTRACT.md](0.1-CONTRACT.md).
Host Tested, Cross Compiled and ELF Inspected evidence is kept distinct from
Runtime Verified behavior. Current evidence uses QEMU 10.0.13 on x86_64 Linux,
`virt-9.1,virtualization=on,gic-version=3,highmem=off`, Cortex-A72, TCG, 512 MiB
and one CPU. The direct-EL1 case changes only virtualization to off.

The current stable compiler is Rust 1.99.0. Rust 1.95.0 independently passes target
checking and all host tests. The supported artifact profile is debug with forced
frame pointers and incremental compilation disabled. The environment installs
Rust and QEMU locally under `/workspace/toolchains`; the user explicitly approved
installing and executing QEMU as an exception to the absent-emulator agent rule.

[validation/PRE2_REPORT.md](validation/PRE2_REPORT.md) describes the accepted
prerelease validation and source/artifact identity. Formal release runs use the
new tested dev source identity and retain their own receipt as described in
RELEASE.md. CI retains exact serial logs
and packages; preserve a local evidence copy for retention beyond 30 days. The
[pre.1 report](validation/REPORT.md) and logs remain historical evidence.
Each harness requires ordered proof and the expected guest exit within ten seconds;
the 705-lifetime hardening run has a 30-second budget. Legacy focused smoke tests
stop QEMU after their terminal marker.

## Current evidence

### 0.2 development extension

The cooperative checkpoint passed `cargo xtask ci` (234 kernel and 20 tooling
tests) and all nine target commands: the existing 24 QEMU launches plus the new
cooperative run. This is development evidence, not a formal 0.2 release receipt.

`test-scheduling` runs the `phoenix.system-cooperative` boot mode without console
input and with a 30-second bound. An observed run completed 64 rounds, 257 reaps,
128 exact ELF-derived illegal-instruction faults and init status 0. Both
wait-before-exit and exit-before-wait are exercised for normal and fault exits.
The two normal processes check all preserved GPRs, NZCV, SP, private stack words,
shared-file handoff and independent descriptor offsets. Ready and zombie children
occupy the single slot; nested spawn and absent-child wait are rejected. The final
legacy synchronous run also succeeds. Raw evidence is `qemu-system-cooperative.log`
under the standard target directory; the host oracle also uses a retained observed
protocol fixture and rejects corrupted lifetimes, faults and context proof.

| ID | State | Observed evidence |
| --- | --- | --- |
| RUN-USCHED-001 | Runtime Verified | Cooperative two-process round robin, blocked-parent and same-process yields, complete saved-context checks, exact resource reuse |
| RUN-USCHED-002 | Runtime Verified | Non-yielding init/infinite child: observed 66/65 ticks and 130 timer handoffs; full 31-GPR/NZCV/SP/private-stack checks; no-timer successful guest is rejected |
| RUN-USCHED-003 | Runtime Pending | User sleep and no-runnable idle; not implemented yet |

The preemption checkpoint passed canonical CI (235 kernel and 21 tooling tests),
the 705-lifetime/576-fault hardening workload with timer entries in both process
slots, and all scheduling scenarios. The strict cooperative scenario keeps IRQs
masked. A separate timer-enabled variant uses actual shared-file progress polling:
counting yields does not guarantee a peer has reached its next milestone under
preemption. Its corrected run observed 265 root ticks, 56 child ticks and 196
timer handoffs across the same 257 lifetimes/128 faults. The non-yielding case
and successful-but-rejected no-timer control also pass. Full suite size is now
28 QEMU launches; exact-source final reruns remain part of candidate preparation.

The following rows retain the 0.1 release scope; new scheduling evidence does not
retroactively expand that tag's contract.

Every Runtime Verified row below applies only to the stated bounded test and board.
Malformed inputs and injected hardware/backend failures also have host coverage;
that coverage does not imply those failures were reproduced in actual hardware.

| ID | State | Observed evidence |
| --- | --- | --- |
| RUN-BOOT-001 | Runtime Verified | `test-boot` reaches the build identity and PHOENIX_BOOT_OK |
| RUN-BOOT-002 | Runtime Verified | `test-runtime` state/EL1 cases observe EL2 and EL1 entry, then EL1 Rust |
| RUN-BOOT-003 | Runtime Verified | Higher-half Rust and stack continue across bootstrap and final TTBR1 |
| RUN-BOOT-004 | Runtime Verified | Full BSS checked before singleton writes; bounded/aligned SP and exact VBAR asserted |
| RUN-CONSOLE-001 | Runtime Verified | PL011 output/flush, CR/LF/CRLF input, invalid-input rejection and generic init outcome |
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
| RUN-ASID-001 | Runtime Verified | 705 child lifetimes read back TTBR0 root/ASID and reuse ASID 2 after TLBI; 254 unused IDs exhausted and retired before successful retry |
| RUN-KSTACK-001 | Runtime Verified | Controlled boot-guard access reports expected translation fault and recovers |
| RUN-KSTACK-002 | Runtime Verified | Both runtime guards fault while usable leaves remain mapped; two workers enter, preempt, sleep, exit and return 16 frames |
| RUN-EXC-001 | Runtime Verified | Fatal BRK reaches vector 4 with exact syndrome/PC and complete register report |
| RUN-EXC-002 | Runtime Verified | x0–x30, SP_EL0, status, PC, syndrome and ERET round trip; fault FAR checked separately |
| RUN-SYNC-001 | Runtime Verified | Timer IRQ stays pending while lock held; enters and acquires it after mask restoration |
| RUN-IRQ-001 | Runtime Verified | GIC publication/activation, exact PPI 30, dispatch, EOI, return and delivery shutdown |
| RUN-TIME-001 | Runtime Verified | Monotonic observations, bound one-shot delivery, 2 ms preemption and absolute thread sleep |
| RUN-UFAULT-001 | Runtime Verified | Nine exact kind/ELF-PC/FAR/ESR cases repeat for 576 contained faults; all-illegal-instruction QEMU negative control is rejected |
| RUN-ELF-001 | Runtime Verified | Strict independently linked init/child/fault ELF images are populated, mapped and entered |
| RUN-STACK-001 | Runtime Verified | Focused init verifies native SP, argc/argv/envp, auxv and terminators |
| RUN-UCOPY-001 | Runtime Verified | Valid/invalid cross-page source/destination copies; failed destination leaves first page and offset unchanged |
| RUN-FS-001 | Runtime Verified | Archive/tmp I/O and ENOSPC content preservation; child FD exhaustion/reaping, independent parent FD/offset, and successful operation after capacity failures |
| RUN-RECOVERY-001 | Runtime Verified | Real loader rollback with 0–9 free frames and each of 21 backend-operation failures; ASID exhaustion, clean retry and subsequent EL0 execution |
| RUN-LIFETIME-001 | Runtime Verified | 705 reaps restore the entire free-frame set, ASID count and process slots; stale PID rejection and reused-stack scrubbing |
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

The formal release requires all current target commands plus the recorded CI job
to pass on its tagged source; its receipt identifies the retained evidence. SMP, nested
interrupts, concurrent userspace, optimized builds, demand paging, signal recovery,
new devices and real hardware remain unimplemented/deferred rather than verified.
A non-cooperating user program can retain the CPU indefinitely in the synchronous
user-system model; worker preemption in another image does not resolve that limit.
Future changes to capacities, stack workloads or these execution modes must add
new ledger entries and fresh target evidence.
