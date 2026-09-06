# Initial Initramfs Contract

This document describes Phoenix's first initramfs parser, deterministic package builder, and
loaded-init lookup boundary. Parsing, packaging, and exact-content checks are Host Tested and Cross
Compiled. The archive has not been consumed on QEMU and is therefore Runtime Pending.

The representation follows the Linux kernel's documented `newc` record sizes and four-byte
alignment rules, but Phoenix deliberately accepts a much smaller subset. The format reference is
the [Linux initramfs buffer format](https://www.kernel.org/doc/html/latest/driver-api/early-userspace/buffer-format.html).
Using this established container format does not imply Linux ABI compatibility.

## Current implementation

`Initramfs::from_bytes` validates the complete borrowed archive before exposing entries. It is
allocation-free and accepts:

- exactly one uncompressed checksum-free `070701` `newc` archive;
- a mandatory `TRAILER!!!` record followed only by optional zero padding;
- at most 128 non-trailer entries;
- UTF-8 relative paths of at most 4095 bytes plus their NUL terminator;
- exactly one trailing NUL and no interior NUL in each encoded name;
- canonical components: no leading or trailing slash, empty component, `.`, or `..`;
- unique paths;
- regular files with exactly one link and inline data;
- directories with at least one link and no data;
- zero checksum and alignment-padding fields.

Every hexadecimal header field is validated even when the initial lookup layer does not consume
its value. Size, offset, and alignment arithmetic is checked before slicing. Iteration is infallible
only after the complete immutable source has passed validation.

Exact lookup returns borrowed entry bytes. `regular_file` rejects directories;
`executable_file` additionally requires at least one Unix execute bit. No data is copied or
extracted into a filesystem by this layer.

## Deterministic package

`cargo xtask build-init` first creates and strips the standalone AArch64 init ELF. It then emits
`phoenix-initramfs.cpio` with one executable regular file at canonical path `init` and one trailer.
The package uses fixed ownership, timestamps, device numbers, inode values, permissions, ordering,
and zero padding. It does not invoke a host `cpio` program.

`cargo xtask inspect-init` parses the generated archive with the production parser and requires:

- exactly one non-trailer entry;
- canonical path `init`;
- regular-file type and permission `0755`;
- byte-for-byte identity with the separately inspected ELF.

The loaded-init build receives the archive path through a build-only environment variable. Its
kernel artifact inspection requires the complete archive byte sequence to appear unchanged. At
boot the kernel validates the archive again, selects executable `init`, and only then passes those
borrowed bytes to the ELF loader.

## Invariants

- no entry is observable from a partially validated archive;
- malformed numeric fields, padding, names, bounds, types, links, and trailers fail explicitly;
- canonical paths cannot escape or ambiguously address the future initramfs root;
- duplicate paths cannot create last-writer-wins behavior;
- unsupported metadata cannot silently change an entry's interpreted type;
- archive and selected-file borrows remain tied to the immutable source bytes;
- packaging is deterministic for identical input ELF bytes;
- building or inspecting the archive cannot imply target execution or a mounted filesystem.

## Validation

Kernel host tests cover valid files and directories, exact lookup, permissions, missing and
non-regular files, non-executable files, truncation, bad magic, malformed hexadecimal fields,
nonzero checksum and padding, invalid UTF-8, missing or interior terminators, unsafe path shapes,
directory payloads, hard links, unsupported types, duplicates, missing or data-bearing trailers,
trailing garbage, oversized declared payloads, and entry-capacity exhaustion.

Host-tool tests construct the same archive twice, require byte equality, and round-trip it through
the production parser. Cross-target checks compile the parser and the loaded-init archive lookup.
Runtime execution is covered by `RUN-INIT-001`.

## TODO

- run the archive-to-init path through `cargo xtask test-init` and retain evidence;
- introduce a read-only inode/dentry ownership model rather than treating the archive as direct
  executable storage;
- define extraction, memory accounting, and lifetime rules for a writable in-memory root;
- add directories and a second program when the first VFS lookup consumer exists;
- decide whether reproducible package manifests should become a checked source file;
- add fuzzing against the bounded parser before accepting externally supplied archives;
- decide how firmware or a bootloader supplies initramfs bytes after compile-time embedding ends;
- add integrity/authenticity policy only with a concrete boot-trust model.

## Skipped work

CRC archives, concatenated archives, leading zero padding, compression, symlinks, hard links,
device nodes, sockets, FIFOs, non-UTF-8 names, extraction, writable files, VFS mounting, page cache,
ownership enforcement, extended attributes, signatures, and external bootloader delivery are not
implemented. The parser's strict subset is intentional; each relaxation requires a consumer,
threat review, tests, and an updated design contract.
