# Memory Foundations

This document describes the Host Tested physical-memory foundations. Final AArch64 page tables,
heap allocation, and runtime integration are not part of the current implementation.

## Address model

`PhysAddr` and `VirtAddr` are distinct transparent types. They provide checked addition,
subtraction, and power-of-two alignment. Phoenix uses 4 KiB base pages, represented by `Page` for
virtual addresses and `PageFrame` for physical addresses.

All ranges are half-open `[start, end)`. Empty ranges are valid, touching ranges do not overlap,
and reversed or overflowing ranges are rejected. No implicit physical/virtual conversion exists.

## Physical memory map

`MemoryMap<CAPACITY>` stores a fixed-capacity, sorted set of non-overlapping frame ranges without
a heap:

- usable byte ranges are rounded inward so only complete pages become allocatable;
- reserved byte ranges are rounded outward so every touched page is removed;
- adjacent and overlapping usable regions are merged;
- reservations can split regions;
- an operation that exceeds metadata capacity returns an error without partially changing the
  map.

`memory_map_from_boot_info` implements the first boot integration: it adds DTB memory ranges and
then reserves firmware reservation-map entries, static `/reserved-memory` children, the complete
kernel physical image, and the DTB blob itself. Bootstrap tables and stacks are covered by the
supplied full linker-image range.

## Frame allocator

Freezing a memory map creates a deterministic first-fit allocator. It supports one-frame and
contiguous allocations with power-of-two frame alignment. It records the complete managed set so
that release can reject foreign frames and double frees. Released neighboring frames are
coalesced.

The fixed range capacity is an explicit early-boot resource. Metadata exhaustion is distinct from
physical memory exhaustion. This makes failure testable without requiring a heap before the frame
allocator exists.

## Invariants

- only complete 4 KiB frames are returned;
- free ranges remain sorted, disjoint, and maximally coalesced;
- reserved or unmanaged frames cannot be introduced through release;
- failed map mutations are transactional;
- allocation failure is returned rather than converted to panic.

## Validation

Host tests cover alignment, overflow, half-open boundaries, sorting, merging, splitting,
capacity exhaustion, deterministic allocation, aligned contiguous allocation, physical
exhaustion, foreign release, double free, and coalescing. The same module is Cross Compiled for
AArch64 by `cargo xtask ci`.

## TODO and skipped work

- choose the production range capacity using observed QEMU DTBs;
- connect the Host Tested `BootInfo` map builder to linker-provided physical bounds at runtime;
- add explicitly designed dynamic and translated `/reserved-memory` policies when needed;
- reserve the DTB for the lifetime of its borrowed references;
- add an early allocation audit log and runtime self-check;
- select a scalable allocator only after allocation patterns are observable;
- define synchronization before sharing an allocator between CPUs.

Buddy allocation, NUMA, memory hotplug, DMA constraints, IOMMU integration, huge-page promotion,
heap allocation, and OOM policy are deliberately skipped. None is required to validate the early
single-core physical-memory ownership model.
