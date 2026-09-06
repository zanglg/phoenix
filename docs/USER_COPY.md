# Checked User Memory Copy

This document describes Phoenix's first `copy_from_user` and `copy_to_user` boundaries plus their
bounded syscall consumers. The generic range walks and ABI request validation are Host Tested. The
concrete AArch64 bootstrap/final-direct-map backends, active-address-space retention, syscall
paths, and userspace calls are Cross Compiled but Runtime Pending.

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

The QEMU `virt` bootstrap backend populates frames while the temporary high-RAM alias is active.
The loaded-init probe then publishes the final physical direct map and uses a narrower active
adapter: it first rejects any frame not owned by the retained prepared address space, then performs
the exact checked copy through the permanent higher-half offset. Lower-EL syscall dispatch may
borrow that immutable ownership metadata while EL0 is stopped in the exception; the address space
is never replaced or reclaimed, while its disjoint file-table field has exclusive single-core
mutation.

`copy_to_user` applies the same pointer, range, residency, and complete prevalidation rules but
requires write permission and copies a kernel slice into owned physical frames. A backend failure
can partly modify user memory, so higher-level transactions must report failure and avoid committing
related state. The initial file read uses that rule to leave its file offset unchanged on failure.

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

The standalone init writes `Phoenix init: hello from EL0\n`, checks the exact return length, then
uses the checked reverse copy through `read` and a writable stack buffer. The kernel records a
successful nonempty write before accepting `exit(42)` as `PHOENIX_INIT_OK`. The QEMU classifier
separately requires the ordered greeting and initramfs file output before the terminal marker.

## Ownership and concurrency invariants

- user-copy consults the same resident-frame owner used to construct the active page tables;
- no raw user pointer is converted into a Rust reference;
- the whole mapping range is validated before the first backend access or external output;
- leaf ownership, table ownership, and allocator state outlive every probe syscall;
- the one-time runtime slot is initialized before EL0 entry and its address-space ownership never
  changes afterward;
- EL0 is not executing while its synchronous syscall is copied;
- current mappings cannot change, and only one CPU exists in the probe;
- release/acquire ordering connects successful output with terminal-exit classification;
- no success is reported for partial copying or partial console output.

## Validation

Host tests cover same-page and cross-page reading, cross-page writing, readable and writable
permissions, unmapped starts, crossing into a gap, user-limit overflow, zero-length semantics,
complete prevalidation before backend access, both backend failure paths, operation validation, and
all syscall bounds. The concrete composition and static runtime lifetime compile for bare-metal
AArch64. Init ELF inspection requires the message and path bytes plus exact linker-symbol extents.

The QEMU harness is host tested to reject `PHOENIX_INIT_OK` when the exact ordered userspace output
did not precede it. Target execution is covered by `RUN-UCOPY-001` and `RUN-INIT-001`.

## TODO

- run the loaded-init write path on QEMU and retain its complete serial evidence;
- replace the probe-local ownership-checked direct-map adapter with a process/VM-owned API;
- move the static runtime's now-explicit lifecycle control and resources into a synchronized
  process-table owner;
- add recoverable architecture fault handling before supporting user virtual dereferences or
  mappings that can change concurrently;
- define partial-transfer behavior, interruption, and larger chunked writes;
- replace the direct initramfs table and special console path with general descriptor objects;
- validate writable user buffers and cross-page data modified by EL0 on QEMU;
- define per-process I/O and copy accounting.

## Skipped work

The current path does not provide a general fault fixup table, direct user virtual access, pinning,
concurrent unmapping, multiple address spaces, multiple CPUs, short writes,
blocking I/O, descriptor duplication, pipes, terminals, general VFS objects, signals, or process teardown.
The 256-byte limit and static lifetime are probe constraints, not stable ABI promises.
