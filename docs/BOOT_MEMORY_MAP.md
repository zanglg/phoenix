# Boot Physical Memory Map

This document describes how validated firmware memory information becomes Phoenix's first frame
allocator. The construction logic is Host Tested and Cross Compiled. Target helpers now borrow the
real DTB through the bootstrap high mapping and derive physical kernel bounds from linker symbols;
executing that complete path remains Runtime Pending.

## Current implementation

`memory_map_from_boot_info<CAPACITY>` creates a new fixed-capacity map as one local transaction. It
performs these operations in order:

1. add every root memory-node range reported by validated `BootInfo`;
2. remove every entry from the DTB memory reservation block;
3. remove every static `reg` range from direct `/reserved-memory` children;
4. remove the complete supplied physical kernel image range;
5. remove the complete source DTB range using its validated total size;
6. reject the result if no complete base page remains.

Usable ranges are rounded inward; reservations are rounded outward. Therefore a partial usable
page is never allocated and every page touched by firmware, kernel, or DTB bytes is withheld. The
kernel range is expected to cover all linked sections through `__kernel_end`, including bootstrap
tables, BSS, boot stack, optional embedded probe resources, and future embedded init data.

Dynamic reserved-memory requests using `size` without a fixed `reg` address are rejected because
silently ignoring them could allocate firmware-owned memory. A non-empty `/reserved-memory/ranges`
is also rejected because Phoenix does not yet translate child addresses. A missing or empty
`ranges` property is treated as the initial identity-address case.

Any DTB iteration failure, physical conversion overflow, metadata exhaustion, empty kernel image,
unsupported reservation form, or fully reserved map returns an error without publishing a partial
allocator. The caller freezes the successful map with `into_allocator` only after reviewing the
result.

## Invariants

- every free frame originates in at least one firmware-declared memory range;
- firmware reservation-map entries are never allocatable;
- static `/reserved-memory` child ranges are never allocatable;
- all pages touched by the loaded kernel image are never allocatable;
- all pages touched by the still-borrowed source DTB are never allocatable;
- fixed metadata exhaustion is distinct from physical-memory exhaustion;
- construction cannot mutate an existing allocator or leak a partial map;
- a successful map contains at least one complete free 4 KiB frame.

## Validation

Host integration tests build a valid synthetic DTB, then verify exact free frame ranges around the
kernel, DTB, firmware, and `/reserved-memory` reservations. Tests also cover DTB size reporting,
metadata exhaustion, an empty kernel image range, a map made empty by reservations, dynamic
reservation rejection, and non-identity child-range rejection. Existing memory tests cover
rounding, sorting, merging, splitting, and allocator behavior.

## TODO

- execute the implemented bounded boot-memory probe on QEMU and record its map evidence;
- define policy for dynamically allocated `/reserved-memory` children when a real consumer appears;
- add child-address translation if a supported platform supplies non-empty `ranges`;
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
