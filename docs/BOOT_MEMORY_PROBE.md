# Boot Memory Integration Probe

This document describes the opt-in `boot-memory-probe` kernel feature. It joins Phoenix's parsed
DTB, linker-derived image reservation, physical memory map, frame allocator, and temporary
higher-half RAM access in one bounded target path. The image is Cross Compiled and ELF Inspected;
the complete probe is Runtime Pending.

## Purpose and activation

The default image still prints `PHOENIX_BOOT_OK` and halts. `cargo xtask test-memory` builds with
`--features boot-memory-probe`, performs the normal static artifact inspection, launches the same
pinned QEMU `virt` board, and waits for the distinct `PHOENIX_MEMORY_OK` sentinel. The earlier boot
marker is progress only and cannot pass this test. A parse, reservation, allocation, mapped access,
readback, scrub, or release failure reaches the existing `PHOENIX_PANIC` path.

Emulator-free `cargo xtask ci` builds and inspects this variant but never starts QEMU.

## Current implementation

After vectors and the early console are active, the probe:

1. treats `x0` as the physical start of a DTB inside bootstrap RAM;
2. validates its fixed header before borrowing the complete declared blob through TTBR1;
3. extracts the boot CPU, RAM ranges, firmware reservations, and static `/reserved-memory` ranges;
4. converts `__kernel_start..__kernel_end` from linked virtual to physical addresses;
5. creates a fixed-capacity memory map and reserves firmware, kernel, and live DTB pages;
6. freezes that map as an allocator and obtains its deterministic first free frame;
7. clears the frame, writes a fixed byte pattern through the high RAM alias, and reads it back;
8. scrubs and releases the frame, then verifies the original free-frame count is restored;
9. prints bounded diagnostics and `PHOENIX_MEMORY_OK`.

The map currently has capacity for 16 disjoint free ranges. That is an explicit bootstrap limit,
not a long-term allocator choice. Metadata exhaustion is fatal and visible instead of dropping a
range. The DTB borrow ends before frame mutation starts, while its physical pages remain reserved
in the allocator.

## Invariants

- the feature is opt-in and does not change the default boot result;
- the DTB header and full declared size must both lie within the temporary RAM alias;
- no frame touching firmware reservations, the linked image, or the live DTB can be allocated;
- the probe frame is privately allocator-owned during every write and read;
- bytes are compared before the frame is scrubbed and returned;
- allocator free-frame count must be identical before allocation and after release;
- no runtime success is claimed until the dedicated sentinel is observed on the pinned board.

## Validation

- Host Tested: DTB validation and extraction, map normalization and reservation, frame allocator
  rollback/release, translated-range bounds, read-output classification, and QEMU command creation;
- Cross Compiled: the complete feature-gated target path and physical memory operations;
- ELF Inspected: normal entry, load addresses, bootstrap table, vectors, BSS, stack, embedded
  memory sentinel, raw image, and map artifact;
- Runtime Pending: DTB placement/content, linker conversion, exact free map, RAM alias access,
  byte readback, and sentinel output.

## TODO

- run `cargo xtask test-memory` and preserve the full serial log and QEMU version;
- compare the printed DTB CPU, size, free-frame count, and allocated address with a captured DTB;
- inspect the translation and reservation boundaries under GDB after the first run;
- replace the provisional 16-range capacity using evidence from supported firmware inputs;
- constrain allocator output to the active physical-access window before supporting larger RAM;
- retain a compact boot allocation audit for production image loading;
- replace the temporary coarse alias with the final permission-separated physical-memory access
  policy.

## Skipped work

The probe does not install final TTBR1 tables, enable caches, allocate production page tables,
populate an ELF process image, activate a dynamic TTBR0, initialize a heap, discover non-DTB
firmware, reclaim memory, or provide a scalable allocator. It validates the earliest ownership and
access chain without turning diagnostic state into permanent kernel state.
