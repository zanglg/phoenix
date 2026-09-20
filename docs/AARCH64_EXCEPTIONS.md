# AArch64 Exceptions

This document defines Phoenix's exception metadata, frame ABI, assembly vector table, and fatal
diagnostic path. The implementation is Host Tested, Cross Compiled, and ELF Inspected. The EL0
probes Runtime Verified the narrow `SVC #0` entry-and-return path; deliberate fault diagnostics and
full register-preservation probes remain Runtime Pending.

## Vector table model

`VBAR_EL1` must point to a 2 KiB-aligned, 2 KiB table. It contains 16 entries of 128 bytes. Phoenix
models each slot as the product of:

- source: current EL with `SP_EL0`, current EL with `SP_ELx`, lower AArch64 EL, or lower AArch32 EL;
- kind: synchronous, IRQ, FIQ, or SError.

The model computes and reversibly decodes every architectural offset from `0x000` through `0x780`.
The ordering follows Arm's
[`Exception model`](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/Exception%20model.pdf)
guide. `vectors.S` emits all slots at those offsets and fails assembly if an entry exceeds 128
bytes. The linked table is checked for 2 KiB size and alignment before it is installed in
`VBAR_EL1` at the beginning of `kernel_main` while exceptions remain masked.

## Exception frame ABI

`ExceptionFrame` has C layout, 16-byte alignment, and an asserted size of 288 bytes:

| Field | Offset | Size |
| --- | ---: | ---: |
| `x0..x30` | 0 | 248 |
| `SP_EL0` | 248 | 8 |
| `ELR_EL1` | 256 | 8 |
| `SPSR_EL1` | 264 | 8 |
| `ESR_EL1` | 272 | 8 |
| `FAR_EL1` | 280 | 8 |

Compile-time assertions make layout drift a build failure. The 288-byte size preserves the
AArch64 16-byte stack alignment. `x30` is saved explicitly; there is no implicit call-frame link
register. FP/SIMD, debug, pointer-authentication, and SVE state are not part of this initial frame.

Every assembly entry masks DAIF, allocates the whole frame, saves all general registers and system
state, and passes the frame plus vector index to `phoenix_exception_dispatch`. If a future
dispatcher returns, assembly writes back mutable SP/PC/status state, restores all registers, and
uses `eret`. Current-EL `SP_EL0` slots switch to `SP_EL1` before touching memory, so they do not
trust the interrupted stack.

The default Rust dispatcher is deliberately fatal for every asynchronous slot. It emits `PHOENIX_EXCEPTION`, the decoded
vector, PC, saved status, syndrome, and fault address, then halts. The opt-in EL0 paths first
recognize their narrow `SVC #0` contract and exercise the assembly return epilogue. Other
consistent lower-AArch64 synchronous exceptions are classified as terminal user faults; current-EL,
asynchronous, SError, and inconsistent exception classes retain the fatal kernel path. General
recovery and production syscall dispatch are not enabled. See `docs/USER_FAULTS.md`.

The architecture GIC module now has a Host Tested type-state API that can activate delivery behind
an unsafe `PublishedIrqDispatcher` proof, classify one `ICC_IAR1_EL1` result, and require EOI of
every non-special INTID. No current vector path constructs that proof or owns an `ActiveGicV3`, so
this protocol is not yet wired into `phoenix_exception_dispatch` and makes no target IRQ claim.

## Syndrome decoding

`Syndrome` preserves the raw `ESR_EL1` value and decodes:

- six-bit exception class without discarding unrecognized values;
- instruction-length bit;
- common 25-bit ISS;
- AArch64 `SVC` immediate for the future syscall path;
- lower/current-EL data aborts.

The data-abort decoder exposes WnR, S1PTW, CM, EA, FnV, and conditionally valid access information
(`ISV`, transfer size, sign extension, target register width/number, and acquire/release). It
classifies address-size, translation, access-flag, permission, and alignment faults while retaining
all other six-bit DFSC values as raw `Other` cases.

Unknown exception classes and fault status codes are data, not parser failures. A class-specific
decoder fails only when applied to the wrong exception class.

The separate user-fault adapter converts supported lower-EL abort, alignment, illegal-instruction,
bad-SVC, and debug classes into architecture-neutral process diagnostics while preserving the raw
syndrome and only retaining `FAR_EL1` when valid.

## Invariants

- vector offsets are aligned, unique, and contained within the 2 KiB table;
- the Rust frame size, alignment, and every assembly-visible field offset are compile-time checked;
- no general register is modified before its original value is stored;
- an exception never calls Rust on an untrusted `SP_EL0` stack;
- raw syndrome values and unknown enumerants are never silently discarded;
- data-abort transfer fields are exposed only when `ISV` says they are valid;
- decoding has no allocation, MMIO, system-register, or unsafe behavior;
- only a lower-AArch64 synchronous vector with a consistent syndrome class can be classified as a
  user fault;
- changing the frame ABI requires coordinated source, assembly, inspection, and document changes.
- Group-1 delivery must remain closed until all EL1 IRQ slots can find a non-transferable active
  GIC owner and complete timer as well as unexpected acknowledgements.

## Validation

Host tests cover all vector offsets, invalid offsets, frame size/alignment/register boundaries,
known and unknown exception classes, SVC immediates, instruction length, data-abort flags and
access metadata, every initially classified fault-status family, portable user-fault mapping,
invalid fault-address handling, and incompatible vector/class rejection. The same code is Cross
Compiled for AArch64. Assembly-time size checks cover each vector entry, and ELF inspection
verifies the complete table's address, alignment, and size.

Successful EL0 syscalls show that the installed vector and exception-return path is usable in the
recorded configuration. Direct `VBAR_EL1` inspection, full register preservation, and deliberate
fault output remain Runtime Pending in `docs/RUNTIME_VALIDATION.md`.

## TODO

- validate vector installation and every save/restore path on QEMU;
- choose dedicated exception-stack and nested-exception policies;
- dispatch current-EL faults, IRQ, FIQ, and SError separately;
- connect EL1 IRQ slots to the prepared GIC delivery type state only after publishing controller
  lookup, stack, and IRQ-safe shared state;
- validate `FAR_EL1` only for classes and syndrome values where the architecture defines it;
- expand diagnostic formatting with class-specific detail without allocation;
- add recoverable user page-fault policy before terminal classification;
- add guarded exception stacks and a double-fault/emergency path;
- add QEMU probes for register preservation, deliberate aborts, `SVC`, and `eret`.

## Skipped work

AArch32 user mode, FP/SIMD/SVE context, debug monitor support, RAS records, nested virtualization,
pointer-authentication keys, memory-tagging state, asynchronous interrupt dispatch, signals, and
user-mode page-fault recovery are deliberately not implemented here. Capturing their existence in
the feature catalog does not expand this frame implicitly.
