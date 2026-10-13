# Terminal User Faults

> Construction-stage mechanism record: runtime-status and remaining-work lists below
> describe the earlier integration boundary. Current support, limits and evidence are
> maintained in [0.1-CONTRACT.md](0.1-CONTRACT.md) and
> [RUNTIME_VALIDATION.md](RUNTIME_VALIDATION.md); those records supersede historical
> pending statements here. The ownership design remains applicable unless the
> current contract explicitly refines it.

This document defines Phoenix's first boundary between an EL0 process fault and an EL1 kernel
fault. Classification and lifecycle behavior are Host Tested and Cross Compiled. No deliberate
fault has yet executed under QEMU.

## Current implementation

The architecture-neutral process model retains a terminal `UserFault` with:

- a normalized kind: address-size, translation, access-flag, permission, alignment, illegal
  instruction, bad system call, debug trap, or other;
- an access class: execute, read, write, or other;
- the interrupted instruction pointer;
- an optional faulting virtual address;
- the complete architecture-specific syndrome value.

On AArch64, classification accepts only the lower-AArch64 synchronous vector. The exception
dispatcher first gives a valid native `SVC #0` to the syscall adapter. If that adapter does not
accept the exception, the user-fault classifier applies this mapping:

| AArch64 exception | Phoenix kind | Access | Address |
| --- | --- | --- | --- |
| Lower-EL instruction abort | Decoded abort status | Execute | `FAR_EL1` unless `FnV` is set |
| Lower-EL data abort | Decoded abort status | Read or write | `FAR_EL1` unless `FnV` is set |
| Program-counter alignment | Alignment | Execute | Interrupted PC |
| Stack-pointer alignment | Alignment | Other | Saved `SP_EL0` |
| Rejected AArch64 `SVC` | Bad system call | Other | None |
| Unknown/trapped WFI, FP/SIMD, illegal state, or system register | Illegal instruction | Other | None |
| Lower breakpoint, software step, watchpoint, or AArch64 `BRK` | Debug trap | Other | None |
| Other consistent lower synchronous class | Other | Other | None |

Address-size, translation, access-flag, permission, and alignment abort statuses become matching
portable kinds. An unrecognized fault-status code remains `Other`; its raw syndrome is never lost.

Current-EL abort/debug classes, SError, an AArch32-only `BKPT`, asynchronous vector slots, and
lower-AArch32 vector slots are not user faults in this AArch64 path. They continue to the kernel
fatal diagnostic and emit `PHOENIX_EXCEPTION`.

## Terminal dispatch

The dynamically loaded init path applies this sequence:

```text
lower synchronous exception
        |
        +-- valid SVC #0 --> native syscall handling
        |
        +-- classified user fault --> Running -> Faulted(details)
        |                              PHOENIX_USER_FAULT
        |                              PHOENIX_INIT_FAIL: user-fault
        |                              halt
        |
        +-- inconsistent/kernel fault --> PHOENIX_EXCEPTION --> halt
```

If init is not in `Running`, the lifecycle transition fails without mutation and dispatch falls
back to `PHOENIX_EXCEPTION`. A user-fault marker therefore cannot conceal a broken process-state
invariant. The static EL0 conformance probe has no process object; it emits the same diagnostic
followed by `PHOENIX_EL0_FAIL: user-fault` and halts.

This is terminal isolation, not recovery. It distinguishes and records which active process
failed, but the one-process probe cannot switch to a scheduler or reclaim the active address space
from its exception stack.

## Invariants

- only an exception known to originate from AArch64 EL0 can become a `UserFault`;
- valid native syscalls are dispatched before fault classification;
- one running process can select exactly one terminal outcome: `Exited` or `Faulted`;
- a fault retains the original syndrome even when Phoenix has a portable classification;
- `FAR_EL1` is retained only when the syndrome does not mark it invalid;
- a process lifecycle mismatch is a kernel invariant failure, not a user fault;
- current-EL and asynchronous failures remain on the kernel fatal path;
- terminal fault handling performs no allocation and never dereferences the user fault address.

## Validation completed without QEMU

Host tests cover exact process fault retention, invalid transitions, reap of a faulted process,
lower instruction/data abort mapping, read/write/execute classification, invalid `FAR_EL1`, bad
SVC, and rejection of incompatible vector/class combinations. QEMU-output tests require the
appropriate probe failure sentinel after the user-fault marker. Static ELF inspection requires
both strings in every EL0-capable kernel variant. All paths are Cross Compiled for AArch64.

Runtime validation is `RUN-UFAULT-001` in `docs/RUNTIME_VALIDATION.md`.

## TODO

- add deliberate translation, permission, execute-never, alignment, illegal-instruction, and bad
  SVC probes under QEMU;
- transfer terminal faults to a scheduler-owned teardown context, retire TTBR0/ASID, and reap the
  retained resources;
- define a page-fault policy that can resolve demand, copy-on-write, and stack-growth faults before
  deciding whether they are terminal;
- translate selected terminal outcomes into native signals or wait status;
- define debugger attachment/continuation policy for debug traps;
- add per-process fault accounting and rate limits;
- decide which external-abort and hardware-error conditions can ever be attributed to a process.

## Skipped work

Phoenix does not yet resume a faulting instruction, demand-page memory, grow stacks, deliver
signals, build a core dump, notify a parent, schedule another process, retire an ASID, or reclaim a
faulted process. A user fault still halts the focused kernel image after producing deterministic
evidence.
