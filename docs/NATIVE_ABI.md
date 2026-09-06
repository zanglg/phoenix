# Native Phoenix ABI Revision 0

This document records the first unstable userspace call convention. Revision 0 is a development
contract tied to the 0.0.0 source tree; it is not a compatibility promise.

## AArch64 system-call convention

EL0 issues `SVC #0`. On entry to EL1:

| Purpose | Register |
| --- | --- |
| System-call number | `x8` |
| Arguments 0 through 5 | `x0` through `x5` |
| Return value | `x0` |

The exception frame preserves all registers. The AArch64 adapter extracts the request without
changing `ELR_EL1`; for an AArch64 `SVC`, the architectural return address already identifies the
following instruction. The eventual dispatcher must accept only the lower-AArch64 synchronous
vector with exception class `SupervisorCallAArch64` and immediate zero.

## Initial number assignments

| Number | Name | Intended arguments | Status |
| ---: | --- | --- | --- |
| 0 | `exit` | `x0=status` | Implemented only by the opt-in static and loaded-init probes |
| 1 | `write` | `x0=fd`, `x1=user buffer`, `x2=length` | Implemented only by the loaded-init stdout probe |

Unknown numbers are preserved and the opt-in probe returns `NotImplemented`; they are not parser errors. These
assignments may change while the revision and project version remain zero.

The probe `write` accepts only descriptor 1 and at most 256 bytes. It validates the complete range
against retained resident-page ownership, reads through the checked physical-memory backend, emits
bytes exactly to PL011, and returns the full length. It returns `BadFileDescriptor`,
`InvalidArgument`, or `BadAddress` before emitting output when validation fails. It does not yet
support short writes or a general file table. See `docs/USER_COPY.md`.

## Process entry stack

The first native process receives a 16-byte-aligned `SP_EL0` pointing to `argc`, followed by
`argv`, `envp`, and a terminated auxiliary vector. Revision 0 currently supplies page size, ELF
entry, and Phoenix ABI revision entries. Exact word and string layout, limits, and validation are
defined in `docs/INITIAL_USER_STACK.md`.

## Return convention

Success is a non-negative value from zero through `i64::MAX`. Errors are encoded in `x0` as the
two's-complement negative of a positive error number from 1 through 4095. The initially named
errors are `BadFileDescriptor` (9), `NoMemory` (12), `BadAddress` (14), `InvalidArgument` (22), and
`NotImplemented` (38). Decoding retains unnamed error numbers.

Values above `i64::MAX` cannot be encoded as success because they collide with the signed error
space. This rule is checked when constructing a return value.

## Invariants

- the generic ABI preserves all six arguments and unknown call numbers;
- architecture register extraction exists only in the AArch64 layer;
- a syscall does not advance the saved PC a second time;
- pointer-shaped arguments remain untrusted integers until user-copy validation;
- a handler writes only the documented return register unless the syscall contract says otherwise;
- ABI revision zero never implies Linux syscall-number or semantic compatibility.

## Validation

Host tests cover known and unknown numbers, all argument positions, register adaptation, PC
preservation, maximum success values, named negative errors, the boundary outside the error
window, and bounded stdout-request validation. The types and adapter are Cross Compiled for
AArch64.

## TODO

- generalize the probe-only SVC recognition into a production dispatcher;
- implement `exit` process teardown after process ownership exists;
- define short writes, interruption, and maximum transfer sizes;
- replace the probe descriptor with a process file table and define stderr initialization;
- separate ABI revisioning from the project version when revision 1 is proposed;
- execute both EL0 conformance programs and retain their QEMU transcripts.

## Skipped work

Linux ABI compatibility, POSIX completeness, ioctl, signals, restartable calls, futexes, polling,
sockets, filesystems, capabilities, tracing, seccomp, 32-bit compatibility, and vDSO calls are not
part of revision 0. No syscall behavior is currently claimed as Runtime Verified.
