# AArch64 Exceptions

This document defines Phoenix's Host Tested exception metadata and the frame layout reserved for
the future EL1 assembly entry path. No vector table is installed and no exception has been handled
on a target yet.

## Vector table model

`VBAR_EL1` must point to a 2 KiB-aligned, 2 KiB table. It contains 16 entries of 128 bytes. Phoenix
models each slot as the product of:

- source: current EL with `SP_EL0`, current EL with `SP_ELx`, lower AArch64 EL, or lower AArch32 EL;
- kind: synchronous, IRQ, FIQ, or SError.

The model computes and reversibly decodes every architectural offset from `0x000` through `0x780`.
The ordering follows Arm's
[`Exception model`](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/Exception%20model.pdf)
guide. `VBAR_EL1` remains unset because the assembly entries and handler contract are the next
runtime-bearing step.

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

The future assembly entry must allocate the whole frame before calling Rust and must restore it
exactly before `eret`. It must not assume that an exception from EL0 arrives on a trusted user
stack. Current-EL `SP_EL0` entries will be treated as a kernel invariant violation until Phoenix
defines a deliberate use for that stack selection.

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

## Invariants

- vector offsets are aligned, unique, and contained within the 2 KiB table;
- the Rust frame size, alignment, and every assembly-visible field offset are compile-time checked;
- raw syndrome values and unknown enumerants are never silently discarded;
- data-abort transfer fields are exposed only when `ISV` says they are valid;
- decoding has no allocation, MMIO, system-register, or unsafe behavior;
- changing the frame ABI requires coordinated source, assembly, inspection, and document changes.

## Validation

Host tests cover all vector offsets, invalid offsets, frame size/alignment/register boundaries,
known and unknown exception classes, SVC immediates, instruction length, data-abort flags and
access metadata, every initially classified fault-status family, and wrong-class rejection. The
same code is Cross Compiled for AArch64.

The future assembly table, `VBAR_EL1` write, exception entry/return, register preservation, and
fault output remain Runtime Pending in `docs/RUNTIME_VALIDATION.md`.

## TODO

- implement a 2 KiB-aligned assembly vector table with compile-time slot-size checks;
- save and restore the exact `ExceptionFrame` layout;
- install `VBAR_EL1` before enabling any exception source;
- choose dedicated exception-stack and nested-exception policies;
- dispatch current-EL faults, EL0 faults/syscalls, IRQ, FIQ, and SError separately;
- validate `FAR_EL1` only for classes and syndrome values where the architecture defines it;
- add readable diagnostic formatting without allocation;
- define recoverable user faults versus fatal kernel faults;
- add guarded exception stacks and a double-fault/emergency path;
- add QEMU probes for register preservation, deliberate aborts, `SVC`, and `eret`.

## Skipped work

AArch32 user mode, FP/SIMD/SVE context, debug monitor support, RAS records, nested virtualization,
pointer-authentication keys, memory-tagging state, asynchronous interrupt dispatch, signals, and
user-mode page-fault recovery are deliberately not implemented here. Capturing their existence in
the feature catalog does not expand this frame implicitly.
