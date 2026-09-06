# Process Image Planning and Frame Ownership

This document describes the architecture-neutral bridge from a validated executable to owned
physical pages. The implementation is Host Tested and Cross Compiled. Physical access is
expressed through a backend contract, but no AArch64 direct-map backend, hardware page tables, or
production EL0 entry exists yet.

## Current implementation

`ProcessImagePlan<MAPPINGS, PAGES>` combines a validated `ElfImage` with one `UserStackLayout`.
Construction is allocation-free and produces:

- one sorted, non-overlapping virtual mapping plan for every `PT_LOAD` segment and the usable
  stack;
- no mapping for the required stack guard;
- one sorted page operation for every program and stack page;
- the executable entry and initial stack pointer;
- an exact count of required physical frames.

Every page operation requires the destination page to be cleared first, then copies at most one
page of borrowed ELF bytes at offset zero. This single rule covers full file pages, a partial final
file page, BSS-only pages, bytes from `p_filesz` through `p_memsz`, and padding through the rounded
page boundary. Stack pages have no initialized bytes and therefore remain entirely zero.

The ELF subset requires page-aligned segment starts and file offsets, so the initial implementation
does not need cross-page source offsets. The plan preserves each segment's final permissions and
purpose, while keeping physical frames and AArch64 descriptors out of the generic virtual plan.

## Transactional frame ownership

`ProcessImagePlan::allocate` assigns one deterministic first-fit frame per user page. It operates
on a private copy of `FrameAllocator` and commits that copy only after all requests succeed. An
allocator error or out-of-memory result therefore leaves the caller's allocator byte-for-byte
unchanged.

The returned `AllocatedProcessImage` is intentionally neither `Copy` nor `Clone`: it represents
unique ownership of its frames. Each entry pairs one virtual page operation with its physical
frame. This is the handoff expected by the future page population and translation-table builder.

`AllocatedProcessImage::release` also operates on an allocator snapshot. On success it consumes
the ownership object and commits all releases. On failure it returns both the unchanged ownership
object and the error, while leaving the supplied allocator unchanged. This prevents a wrong or
stale allocator from causing partial reclamation or an untracked leak.

## Population type state

`ProcessImageMemory` is the narrow backend needed to clear a private frame and copy a checked byte
slice into it. A future kernel implementation may satisfy it through a physical direct map or a
temporary mapping window; host tests use ordinary owned byte arrays.

`AllocatedProcessImage::populate` clears every complete page before copying its initialized ELF
prefix. It returns `PopulatedProcessImage` only after every operation succeeds. A backend failure
reports the page index and whether clearing or initialization failed, and returns the original
frame ownership object. A retry starts by clearing all pages again, so bytes from a partial attempt
cannot survive unnoticed.

The successful populated type retains only resident virtual address, physical frame, permission,
and purpose metadata. It no longer borrows initialized byte slices, so the source ELF storage may
be released immediately after population. The AArch64 layer can consume this resident owner with
matching materialized tables to create one prepared address-space owner.

## Invariants

- the complete ELF was validated before process-image planning begins;
- program mappings, usable stack pages, and the unmapped guard never overlap;
- page operations and address-space mappings are sorted by user virtual address;
- every planned page is aligned, has final W^X-safe permissions, and requires exactly one frame;
- all destination pages must be cleared before initialized bytes are copied;
- no initialized-byte slice exceeds a page;
- fixed-capacity exhaustion occurs before any external state changes;
- assigning or releasing frames is all-or-nothing with respect to the supplied allocator;
- only a successful complete clear-and-copy pass creates the populated type state;
- successful population ends every borrow of source ELF bytes;
- ownership moves linearly from allocated, to populated, to the prepared architecture address
  space; none of these resource owners is copyable;
- the source ELF bytes must outlive planning and population.

## Validation

Host tests cover multi-page text and data, partial final pages, zero-only stack pages, virtual
ordering independent of program-header order, guard collision, mapping and page metadata limits,
deterministic frame pairing, exact release, out-of-memory rollback, a failed release against a
wrong allocator snapshot, complete zeroing, partial final pages, injected write failure, clean
retry, release of source storage, and combination with matching AArch64 table ownership. Target
checks compile the same ownership model for bare-metal AArch64.

## TODO

- implement the `ProcessImageMemory` backend through a documented physical direct map;
- make instruction-cache maintenance explicit before newly copied executable bytes can run;
- define when a complete address space becomes visible through an ASID and `TTBR0_EL1`;
- construct `argc`, `argv`, `envp`, and the minimal auxiliary vector on the guarded stack;
- load a reproducibly built embedded ELF fixture through this path and validate it in QEMU;
- integrate resource accounting and a process owner before supporting teardown outside tests.

## Skipped work

This module does not implement physical-memory access, a permanent direct map, hardware page-table
mutation, TLB invalidation, safe user copying, process identifiers, scheduling, VFS lookup,
initramfs, stack growth, demand paging, copy-on-write, ASLR, dynamic linking, TLS, or signals. The
static `el0-probe` remains a separate conformance bridge until these production ownership stages
are connected.
