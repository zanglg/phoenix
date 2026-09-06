# Read-only Initramfs File Descriptors

This document describes Phoenix's first process-local file table. The table and its transaction
rules are Host Tested and Cross Compiled. Its loaded-init syscall composition is Runtime Pending.
It is intentionally a direct read-only initramfs consumer, not yet a general VFS.

## Current implementation

`ReadOnlyFileTable` borrows one completely validated `Initramfs` and owns a fixed-capacity array of
open-file descriptions. Descriptors zero through two remain reserved for standard streams;
regular files receive the lowest available descriptor starting at three. The loaded-init probe has
four regular-file slots.

`open` accepts only a canonical UTF-8 path relative to the archive root. It rejects empty,
absolute, trailing-slash, empty-component, current-directory, parent-directory, and NUL-containing
paths. The selected entry must be a regular file and have at least one Unix read bit. Each open has
an independent byte offset.

`read` is represented internally as two operations:

1. `read_window` borrows at most the requested bytes without changing the offset;
2. `commit_read` advances by the complete window only after `copy_to_user` succeeds.

End of file is an empty successful window. A generation number changes whenever a slot is reopened,
so a window retained across close/reopen cannot advance the replacement file. A window also becomes
stale after another committed read changes the current offset. `close` invalidates the open file
but leaves its generation history intact.

## Probe ABI composition

The revision-0 loaded-init probe provides:

- `open(path, path_length, 0)` with a maximum 128-byte path;
- `read(fd, user_buffer, length)` with a maximum 256-byte request;
- `close(fd)` for descriptors owned by this table.

`open` copies the complete path from resident user frames before UTF-8 and canonical-path checks.
`read` obtains a window, copies it only into fully prevalidated writable user frames, and commits the
offset afterward. Therefore an invalid user destination does not consume file bytes. A physical
backend error can partly modify user memory, but still leaves the file offset unchanged and returns
`BadAddress`.

The first initramfs contains executable `init`, directory `etc`, and read-only file `etc/motd`.
`init` opens the file, reads its exact contents into its stack, writes those bytes to stdout, checks
EOF with a second read, closes the descriptor, and only then exits successfully.

## Ownership and concurrency invariants

- the table never outlives its immutable, fully validated archive bytes;
- an open description borrows bytes rather than copying or extracting them;
- descriptor allocation is deterministic and bounded;
- a read offset changes only through a matching generation-and-offset commit;
- a failed user copy never commits the read window;
- user pointers are never converted directly into Rust references;
- the probe mutates its table only during synchronous exception handling while EL0 is stopped;
- the current single-CPU dispatcher is non-reentrant; a general implementation will require
  process ownership and synchronization.

## Validation

Host tests cover independent opens, lowest-descriptor allocation, bounded reads, EOF, deferred
offset updates, stale windows, close/reopen generation changes, table exhaustion, bad descriptors,
directories, unreadable files, missing files, and invalid paths. User-copy tests separately cover
writable permissions, cross-page writes, unmapped ranges, and backend failure. ABI tests cover all
three operation numbers, bounds, read-only flags, and argument extraction.

The standalone init ELF and deterministic three-entry initramfs are statically inspected for exact
path and payload bytes. Runtime execution is tracked by `RUN-FS-001`, `RUN-UCOPY-001`, and
`RUN-INIT-001`.

## TODO

- move the table into a real process object with lifecycle and locking;
- introduce VFS node and open-file interfaces when a second filesystem or device file exists;
- represent stdin, stdout, and stderr as ordinary descriptor objects;
- define credentials, ownership, and access checks beyond the current any-read-bit rule;
- support directory lookup, `stat`, seek, descriptor duplication, and inherited descriptors;
- define partial reads, interruption, blocking, and concurrent offset semantics;
- choose whether the initial root remains archive-backed or is extracted into a writable ramfs;
- add resource accounting and per-process descriptor limits.

## Skipped work

Writable files, creation, unlink, rename, mounts, symbolic links, hard links, device nodes,
directories as descriptors, current working directories, absolute paths, page cache, memory mapping,
polling, pipes, sockets, terminals, advisory locks, async I/O, and persistence are not implemented.
The numeric calls and capacity limits remain revision-0 probe details, not stable promises.
