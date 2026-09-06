# Process Image Planning and Frame Ownership

This document describes the architecture-neutral bridge from a validated executable to owned
physical pages. The implementation is Host Tested and Cross Compiled. A focused kernel variant
connects the Runtime Pending AArch64 bootstrap backend to a real ELF selected from the embedded
initramfs, native stack, dynamic tables, and ownership-gated activation. It has not executed on the
target.

## Current implementation

`ProcessImagePlan<MAPPINGS, PAGES>` combines a validated `ElfImage` with one `UserStackLayout`.
Construction is allocation-free and produces:

- one sorted, non-overlapping virtual mapping plan for every `PT_LOAD` segment and the usable
  stack;
- no mapping for the required stack guard;
- one sorted page operation for every program and stack page;
- the executable entry and either the empty-stack top or a constructed native initial stack
  pointer;
- an exact count of required physical frames.

Every page operation requires the destination page to be cleared first, then copies at most one
page of borrowed bytes at a checked page offset. ELF segment bytes begin at offset zero; an
optional native initial stack may begin partway through a page and cross page boundaries. This
single rule covers full file pages, a partial final file page, BSS-only pages, ELF padding, a
zero-only stack, and the complete argc/argv/envp/auxv stack image.

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
slice into it. Host tests use ordinary owned byte arrays. QEMU `virt` has a Cross Compiled,
Runtime Pending implementation using the temporary higher-half bootstrap RAM alias.

`AllocatedProcessImage::populate` clears every complete page before copying its initialized ELF
prefix. It returns `PopulatedProcessImage` only after every operation succeeds. A backend failure
reports the page index and whether clearing or initialization failed, and returns the original
frame ownership object. A retry starts by clearing all pages again, so bytes from a partial attempt
cannot survive unnoticed.

The successful populated type retains only resident virtual address, physical frame, permission,
purpose metadata, entry address, and final initial stack pointer. It no longer borrows initialized
byte slices, so the source ELF and stack-image storage may be released immediately after
population. The AArch64 layer can consume this resident owner with matching materialized tables to
create one prepared address-space owner.

## Invariants

- the complete ELF was validated before process-image planning begins;
- program mappings, usable stack pages, and the unmapped guard never overlap;
- page operations and address-space mappings are sorted by user virtual address;
- every planned page is aligned, has final W^X-safe permissions, and requires exactly one frame;
- all destination pages must be cleared before initialized bytes are copied;
- every initialized-byte slice and its destination offset fit one page;
- fixed-capacity exhaustion occurs before any external state changes;
- assigning or releasing frames is all-or-nothing with respect to the supplied allocator;
- only a successful complete clear-and-copy pass creates the populated type state;
- successful population ends every borrow of source ELF bytes;
- ownership moves linearly from allocated, to populated, to the prepared architecture address
  space; none of these resource owners is copyable;
- the source ELF and optional initial-stack bytes must outlive planning and population.

## Validation

Host tests cover multi-page text and data, partial final pages, zero-only stack pages, a native
initial stack crossing two pages at a nonzero first-page offset, virtual
ordering independent of program-header order, guard collision, mapping and page metadata limits,
deterministic frame pairing, exact release, out-of-memory rollback, a failed release against a
wrong allocator snapshot, complete zeroing, partial final pages, injected write failure, clean
retry, release of source storage, and combination with matching AArch64 table ownership. Target
checks compile the same ownership model for bare-metal AArch64. Static inspection requires the
exact validated initramfs containing the loader-accepted init ELF inside the resulting kernel
artifact.

## TODO

- replace the bootstrap memory backend with a final documented physical direct map;
- make instruction-cache maintenance explicit before newly copied executable bytes can run;
- define when a complete address space becomes visible through an ASID and `TTBR0_EL1`;
- execute the reproducibly built embedded ELF through this path and validate it in QEMU;
- integrate resource accounting and a process owner before supporting teardown outside tests.

## Skipped work

This module does not implement physical-memory access, a permanent direct map, hardware page-table
mutation, TLB invalidation, safe user copying, process identifiers, scheduling, VFS lookup,
initramfs, stack growth, demand paging, copy-on-write, ASLR, dynamic linking, TLS, or signals. The
static `el0-probe` remains a separate, smaller conformance bridge for isolating failures in this
larger dynamic path.
