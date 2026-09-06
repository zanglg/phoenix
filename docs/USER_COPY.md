# Checked User Memory Copy

This document describes Phoenix's first `copy_from_user` boundary and the bounded console-write
consumer. The generic range walk and ABI request validation are Host Tested. The concrete AArch64
bootstrap backend, active-address-space retention, syscall path, and userspace call are Cross
Compiled but Runtime Pending.

## Current implementation

`copy_from_user` accepts a raw 64-bit ABI pointer, a kernel destination slice, one
`PopulatedProcessImage`, and a `UserMemoryReader` backend. It never dereferences the raw user
virtual pointer. Instead it:

1. checks that the pointer fits the build's address size and lies in the lower user region;
2. constructs the complete half-open range with checked arithmetic;
3. walks the entire range page by page before performing any read;
4. requires an owned resident page and user-read permission for every byte;
5. translates each page through the populated image's virtual-to-physical ownership metadata;
6. asks the backend to copy from those physical frames, splitting at page boundaries.

An unmapped page, canonical-boundary crossing, or permission failure occurs before the backend can
modify the destination. A backend failure may leave the private destination buffer partly changed;
callers must discard it unless the operation returns success. A zero-length operation still
validates pointer width and the user-region boundary but requires no mapped page.

The QEMU `virt` bootstrap backend reads only frames inside its checked temporary high-RAM alias.
The loaded-init probe retains the complete prepared address space and remaining allocator state in
a one-time static runtime slot before `eret`. Lower-EL syscall dispatch may borrow that immutable
ownership metadata while EL0 is stopped in the exception; the slot is never replaced or reclaimed.

## Initial write consumer

Native revision-0 `write` remains a deliberately bounded probe service:

- only file descriptor 1 is accepted;
- the maximum request is 256 bytes;
- the complete user range must pass `copy_from_user` before any output is emitted;
- bytes are written exactly to the early PL011 console, without text or newline conversion;
- success returns the complete requested length; no short write exists yet;
- bad descriptors return `BadFileDescriptor`;
- an oversized or unrepresentable length returns `InvalidArgument`;
- user-range or backend failures return `BadAddress`.

The standalone init writes `Phoenix init: hello from EL0\n`, checks the exact return length, and
only then calls `exit(42)`. The kernel records a successful nonempty write before accepting that
exit as `PHOENIX_INIT_OK`. The QEMU classifier separately requires the exact userspace message to
precede the terminal marker.

## Ownership and concurrency invariants

- user-copy consults the same resident-frame owner used to construct the active page tables;
- no raw user pointer is converted into a Rust reference;
- the whole mapping range is validated before the first backend read or external output;
- leaf ownership, table ownership, and allocator state outlive every probe syscall;
- the one-time runtime slot is initialized before EL0 entry and never mutated afterward;
- EL0 is not executing while its synchronous syscall is copied;
- current mappings cannot change, and only one CPU exists in the probe;
- release/acquire ordering connects successful output with terminal-exit classification;
- no success is reported for partial copying or partial console output.

## Validation

Host tests cover same-page and cross-page copying, exact physical reads, unmapped starts, crossing
from a mapped page into a gap, user-limit overflow, zero-length semantics, complete prevalidation
before backend access, backend failure, standard-output selection, operation validation, and the
write-size bound. The concrete composition and static runtime lifetime compile for bare-metal
AArch64. Init ELF inspection requires the message bytes and their exact linker-symbol extent.

The QEMU harness is host tested to reject `PHOENIX_INIT_OK` when the userspace message did not
precede it. Target execution is covered by `RUN-UCOPY-001` and `RUN-INIT-001`.

## TODO

- run the loaded-init write path on QEMU and retain its complete serial evidence;
- replace the bootstrap high-RAM backend with the final physical direct map;
- move the static runtime slot into a process owner with locking and lifecycle states;
- implement `copy_to_user` with the same full-range prevalidation;
- add recoverable architecture fault handling before supporting user virtual dereferences or
  mappings that can change concurrently;
- define partial-transfer behavior, interruption, and larger chunked writes;
- connect descriptors to a real file table and console object;
- validate writable user buffers and cross-page data modified by EL0;
- define per-process I/O and copy accounting.

## Skipped work

The current path does not provide a general fault fixup table, direct user virtual access, writable
copy-to-user, pinning, concurrent unmapping, multiple address spaces, multiple CPUs, short writes,
blocking I/O, descriptor duplication, pipes, terminals, VFS files, signals, or process teardown.
The 256-byte limit and static lifetime are probe constraints, not stable ABI promises.
