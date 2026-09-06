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
| 0 | `exit` | `x0=status` | Implemented by the probes; loaded init records a terminal process state before reporting success |
| 1 | `write` | `x0=fd`, `x1=user buffer`, `x2=length` | Implemented only by the loaded-init stdout probe |
| 2 | `read` | `x0=fd`, `x1=user buffer`, `x2=length` | Implemented only by the loaded-init file probe |
| 3 | `open` | `x0=path bytes`, `x1=path length`, `x2=mode` | Implemented only by the loaded-init file probe |
| 4 | `close` | `x0=fd` | Implemented only by the loaded-init file probe |

Unknown numbers are preserved and the opt-in probe returns `NotImplemented`; they are not parser errors. These
assignments may change while the revision and project version remain zero.

The probe `write` accepts only descriptor 1 and at most 256 bytes. It validates the complete range
against retained resident-page ownership, reads through the checked physical-memory backend, emits
bytes exactly to PL011, and returns the full length. It returns `BadFileDescriptor`,
`InvalidArgument`, or `BadAddress` before emitting output when validation fails. It does not yet
support short writes or a general file table. See `docs/USER_COPY.md`.

The probe `open` accepts a nonempty canonical UTF-8 path of at most 128 bytes and mode zero
(`OPEN_READ_ONLY`). It returns the lowest free regular-file descriptor starting at 3. The probe
`read` accepts at most 256 bytes, copies available data into a fully prevalidated writable user
range, returns zero at EOF, and advances the open-file offset only after the copy succeeds. `close`
releases that descriptor. The loaded-init table has four regular-file slots. These direct
initramfs semantics are detailed in `docs/FILE_DESCRIPTORS.md`.

## Process entry stack

The first native process receives a 16-byte-aligned `SP_EL0` pointing to `argc`, followed by
`argv`, `envp`, and a terminated auxiliary vector. Revision 0 currently supplies page size, ELF
entry, and Phoenix ABI revision entries. Exact word and string layout, limits, and validation are
defined in `docs/INITIAL_USER_STACK.md`.

## Return convention

Success is a non-negative value from zero through `i64::MAX`. Errors are encoded in `x0` as the
two's-complement negative of a positive error number from 1 through 4095. The initially named
errors are `NoSuchFile` (2), `InputOutput` (5), `BadFileDescriptor` (9), `NoMemory` (12),
`PermissionDenied` (13), `BadAddress` (14), `IsDirectory` (21), `InvalidArgument` (22),
`TooManyOpenFiles` (24), and `NotImplemented` (38). Decoding retains unnamed error numbers.

Values above `i64::MAX` cannot be encoded as success because they collide with the signed error
space. This rule is checked when constructing a return value.

The current `exit` argument is retained as a full opaque 64-bit `ExitStatus`; it is not truncated
to Unix's conventional low eight bits. Loaded init accepts its terminal success sentinel only when
the active process control can move from `Running` to `Exited`. Resource reclamation remains a
kernel lifecycle concern and does not occur on the exiting EL0 exception stack.

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
window, and bounded stdout/open/read/close request validation. File-table behavior and both user
copy directions have separate host tests. The types and adapter are Cross Compiled for AArch64.

## TODO

- generalize the probe-only SVC recognition into a production dispatcher;
- move terminal `exit` from the retained probe owner into scheduler-context teardown and reap;
- define partial I/O, interruption, and production transfer limits;
- move the probe file table into table-owned process resources and define stdin/stderr initialization;
- separate ABI revisioning from the project version when revision 1 is proposed;
- execute both EL0 conformance programs and retain their QEMU transcripts.

## Skipped work

Linux ABI compatibility, POSIX completeness, ioctl, signals, restartable calls, futexes, polling,
sockets, general VFS semantics, capabilities, tracing, seccomp, 32-bit compatibility, and vDSO
calls are not part of revision 0. No syscall behavior is currently claimed as Runtime Verified.
