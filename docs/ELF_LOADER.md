# Initial ELF Loader Contract

This document describes the accepted executable format and current parser. Phoenix can validate a
strict ELF image and transactionally plan and assign its pages, but does not yet populate physical
memory, install hardware mappings, or execute the image.

## Current implementation

`ElfImage::parse` is read-only, allocation-free, and accepts this initial subset:

- ELF64, little-endian, current ELF version, System V ABI identifier;
- `ET_EXEC` fixed-address executable for machine `EM_AARCH64`;
- exactly 64-byte ELF headers and 56-byte program headers;
- 1 through 128 program headers;
- one or more readable `PT_LOAD` segments;
- power-of-two segment alignment at least 4 KiB;
- page-aligned file offsets and virtual starts with ELF alignment congruence;
- `p_filesz <= p_memsz`, non-zero memory size, and complete file bounds;
- page-rounded user virtual ranges outside the null page;
- no overlapping load pages and no writable-executable segment;
- an entry point contained in an executable load segment.

Unknown non-load program-header types are ignored. Unknown load permission bits are rejected. Each
validated load segment exposes borrowed initialized bytes, exact memory size, required zero-fill,
page-rounded virtual extent, and user permissions. Iteration revalidates entries and reports any
error rather than silently shortening the segment list.

This strict subset intentionally rejects some valid general-purpose ELF files. Phoenix will own the
first user linker script, so requiring clean page boundaries removes permission-merging ambiguity
from the earliest loader.

## Current planning transaction

`ProcessImagePlan` now performs the first parts of the loading transaction:

1. validate the whole image before allocating resources;
2. reserve all required virtual ranges in one user plan;
3. include a non-overlapping guarded stack;
4. create one clear-and-copy operation per mapped page;
5. assign all physical frames atomically against an allocator snapshot.

The future materializer must continue by clearing every frame, copying exactly the planned source
bytes, retaining zeroes through the rounded page end, installing final W^X permissions, and rolling
back every frame and table on any failure. See `docs/PROCESS_IMAGE.md` for the ownership boundary.

Executable bytes must never be writable at EL0. The source image may be released only after every
borrow and copy completes.

## Invariants

- all integer conversions, additions, multiplications, and file slices are checked;
- validation completes before an `ElfImage` value is returned;
- unknown classes, endianness, machine types, and fixed header-size changes are rejected;
- load ranges remain within user space and do not share a page;
- zero-fill size can never underflow;
- the parser performs no allocation and dereferences no address from the file.

## Validation

Host fixtures cover a text-plus-data executable, BSS zero-fill accounting, malformed identity,
wrong machine and header sizes, truncated data, file size larger than memory, W^X, unreadable load
segments, page overlap, non-executable entry, null-page placement, and unaligned loads. Cross-target
compilation checks that the parser remains `no_std` compatible.

## TODO

- add the first user linker script and reproducibly build a tiny AArch64 test executable;
- implement physical frame population and hardware mapping for the transactional plan;
- construct the initial guarded stack and auxiliary vector;
- decide whether program headers must themselves be available through `AT_PHDR`;
- record a formal process-image ownership model;
- add property/fuzz tests with a bounded malformed-input corpus;
- validate instruction-cache maintenance before executing freshly copied code;
- execute the loaded image under QEMU and retain its ELF metadata as test evidence.

## Skipped work

`ET_DYN`, relocations, a dynamic linker, interpreters, shared objects, TLS, symbol resolution,
unwinding, notes, section headers, GNU properties, RELRO, executable stacks, core files, and Linux
compatibility are deliberately unsupported. They should be added only with a concrete userspace
consumer and updated acceptance tests.
