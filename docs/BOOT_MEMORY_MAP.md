# Boot Physical Memory Map

This document describes how validated firmware memory information becomes Phoenix's first frame
allocator. The construction logic is Host Tested and Cross Compiled; consuming the real QEMU DTB
and linker symbols remains Runtime Pending.

## Current implementation

`memory_map_from_boot_info<CAPACITY>` creates a new fixed-capacity map as one local transaction. It
performs these operations in order:

1. add every root memory-node range reported by validated `BootInfo`;
2. remove every entry from the DTB memory reservation block;
3. remove the complete supplied physical kernel image range;
4. remove the complete source DTB range using its validated total size;
5. reject the result if no complete base page remains.

Usable ranges are rounded inward; reservations are rounded outward. Therefore a partial usable
page is never allocated and every page touched by firmware, kernel, or DTB bytes is withheld. The
kernel range is expected to cover all linked sections through `__kernel_end`, including bootstrap
tables, BSS, boot stack, optional embedded probe resources, and future embedded init data.

Any DTB iteration failure, physical conversion overflow, metadata exhaustion, empty kernel image,
or fully reserved map returns an error without publishing a partial allocator. The caller freezes
the successful map with `into_allocator` only after reviewing the result.

## Invariants

- every free frame originates in at least one firmware-declared memory range;
- firmware reservation-map entries are never allocatable;
- all pages touched by the loaded kernel image are never allocatable;
- all pages touched by the still-borrowed source DTB are never allocatable;
- fixed metadata exhaustion is distinct from physical-memory exhaustion;
- construction cannot mutate an existing allocator or leak a partial map;
- a successful map contains at least one complete free 4 KiB frame.

## Validation

Host integration tests build a valid synthetic DTB, then verify exact free frame ranges around the
kernel, DTB, and firmware reservations. Tests also cover DTB size reporting, metadata exhaustion,
an empty kernel image range, and a map made empty by reservations. Existing memory tests cover
rounding, sorting, merging, splitting, and allocator behavior.

## TODO

- obtain physical `__kernel_start` and `__kernel_end` through one checked linker-layout helper;
- validate and borrow the firmware DTB from the physical boot argument;
- parse and reserve `/reserved-memory` child `reg` ranges in addition to the header reservation map;
- constrain allocations to memory reachable through the active physical-memory backend;
- select the production metadata capacity from captured QEMU DTBs and fail with diagnostics;
- emit a bounded early allocation audit before the first user image consumes frames;
- retain the DTB reservation until every borrowed property has been copied or released;
- validate the exact map under QEMU/GDB and record it in `RUN-MM-001`.

## Skipped work

This module does not discover ACPI memory, reclaim firmware boot services, distinguish NUMA nodes,
classify persistent or hot-pluggable memory, support memory above the active physical-address size,
or choose a scalable long-lived allocator. Those policies follow the first safe single-core QEMU
boot allocator.
