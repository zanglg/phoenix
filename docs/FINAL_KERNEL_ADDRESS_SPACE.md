# Final AArch64 Kernel Address Space

This document describes Phoenix's implemented final `TTBR1_EL1` hierarchy for the pinned
AArch64 QEMU `virt` platform. Planning, allocation, materialization, cross-compilation, and ELF
inspection are complete. Publication on a real AArch64 CPU remains Runtime Pending.

## Current implementation

Phoenix keeps a fixed physical-to-virtual offset of `0xffffff8000000000`. Every complete frame in
the DTB memory nodes is mapped at `physical + offset`. The current QEMU command supplies one
512 MiB RAM region at `0x40000000..0x60000000`; the bootstrap backend deliberately rejects a RAM
topology outside `0x40000000..0x80000000` because it could not safely initialize table frames
there before the switch.

The linker emits page-aligned permission-domain symbols and enforces their order:

| Domain | Physical range | Final EL1 attributes |
| --- | --- | --- |
| Text, vectors, optional probe text | `__text_start..__text_end` minus the offset | Normal, read-only, executable |
| Read-only data | `__rodata_start..__rodata_end` minus the offset | Normal, read-only, non-executable |
| Data, BSS, boot tables, stacks | `__data_start..__data_end` minus the offset | Normal, read/write, non-executable |
| Other DTB-declared RAM | Complete frames in each memory node | Normal, read/write, non-executable |
| PL011 | Physical `0x09000000`, one 4 KiB page | Device-nGnRnE, read/write, non-executable |

All mappings are EL1-only. The plan rejects EL0-accessible attributes and writable-executable
combinations. The linked image must end no later than physical `0x40200000`; this keeps all
permission exceptions inside the first 2 MiB RAM block. The linker fails instead of silently
weakening permissions when the image exceeds that boundary.

## Planning and topology

`kernel_page_table.rs` provides a fixed-capacity, allocation-free upper-half planner. It validates
the complete requested change in a copy before committing it and chooses the largest legal leaf:

- 1 GiB L1 block when virtual address, physical address, and remaining length permit it;
- otherwise a 2 MiB L2 block;
- otherwise a 4 KiB L3 page.

Permission overrides split the physical direct map at page boundaries. On the pinned 512 MiB
machine, the first 2 MiB is represented by 512 L3 pages so that text, rodata, and data differ;
the remaining 510 MiB uses 255 L2 blocks. The single PL011 page brings the exact topology to five
table frames and 768 leaf mappings. The configured limits are eight table frames and 1024 leaves,
so an unexpected DTB topology fails explicitly rather than truncating the address space.

The platform builder consumes its unpublished plan as it appends each range, avoiding a second
roughly 32 KiB rollback copy on the bounded early stack. Public incremental mutation remains
transactional for general callers. After descriptor materialization, the permanent owner discards
all leaf-planning metadata and retains only the compact table-role/frame array; a host test keeps
that owner below 512 bytes.

The QEMU platform builder verifies both ends of text, rodata, and data through the offline
translation model and verifies the PL011 physical address, page level, and device attributes
before any table frame is allocated.

## Ownership and publication

Table frames are allocated as one transaction from the reservation-aware boot allocator. Every
table is cleared before parent links or leaves are written. A backend failure returns the allocated
owner so the caller can retry or release unpublished frames; partially initialized tables are
never installed.

The integration probe moves the completely materialized hierarchy and the remaining allocator
into a one-time static runtime owner. Only a `'static` borrow of that owner can reach the unsafe
publication operation. Publication executes:

1. `DSB ISHST` to make descriptor writes visible;
2. write the owned root frame to `TTBR1_EL1`;
3. `ISB`;
4. `TLBI VMALLE1IS`;
5. `DSB ISH` and `ISB` before continuing.

This replaces the coarse higher-half bootstrap hierarchy. It intentionally leaves `TTBR0_EL1`
unchanged, so the temporary low device and RAM aliases remain until a later user address-space
transition replaces TTBR0. Removing those aliases is a separate runtime-sensitive operation.

## Focused QEMU probe

The `kernel-map-probe` image uses the real boot DTB, kernel linker bounds, reservation-aware frame
allocator, and bootstrap physical-memory backend. Before publication it emits
`PHOENIX_KERNEL_MAP_ENTER`. After the TTBR1 switch it reuses the mapped kernel text, rodata, stack,
data, and PL011 device page to emit `PHOENIX_KERNEL_MAP_OK`, then halts.

When QEMU is available, run:

```bash
cargo xtask test-kernel-map
```

The bounded harness cannot pass on the earlier boot or enter marker. Panic, exception, early exit,
timeout, and excessive output are failures. Output is retained in
`target/phoenix/aarch64-unknown-none-softfloat/debug/qemu-kernel-map.log`.

The `loaded-init-probe` uses the same builder and owner in its end-to-end path. It constructs both
kernel and user tables before publication, switches TTBR1 first, then installs the prepared TTBR0
user hierarchy and enters EL0. Its syscall-side direct-map adapter accepts only frames proven to
belong to the retained init address space.

## Validation completed without QEMU

Host tests cover range decomposition, exact translation, permission replacement, deterministic
topology, table-capacity and overlap failures, atomic allocation, descriptor materialization,
retryable backend failure, and unpublished release. Platform tests cover the pinned 512 MiB
topology, critical mappings, linker-layout rejection, and RAM-window rejection.

The focused kernel image is Cross Compiled and ELF Inspected. Static checks require every
permission-domain symbol, page alignment and ordering, the first-2-MiB image bound, the expected
load addresses, and the final success sentinel. None of this is runtime evidence.

## TODO

- run the focused probe on the pinned QEMU version and record the complete evidence;
- remove the temporary TTBR0 aliases when no low virtual pointer or bootstrap handle survives;
- replace the bootstrap-only physical-memory handle with a final direct-map API and lifetime;
- derive the physical-address size from `ID_AA64MMFR0_EL1.PARange` before accepting RAM above
  the current 32-bit IPS envelope;
- enable caches only after cache-maintenance and memory-ordering requirements are designed and
  runtime tested;
- add guarded kernel and exception stacks;
- define safe live mapping updates, break-before-make, local versus broadcast TLB maintenance,
  ASIDs, and SMP synchronization;

## Skipped work

This implementation does not provide dynamic kernel mappings, a heap, per-CPU tables, recursive
mapping, KASLR, copy-on-write, demand paging, swapping, transparent huge pages, 16/64 KiB granules,
52-bit addresses, MTE, stage-2 translation, DMA/IOMMU mappings, or SMP TLB shootdown. It maps all
DTB-declared RAM into an EL1-only direct map; it does not use mapping permissions as an ownership
substitute for firmware-reserved or allocator-owned frames.

## Runtime status

`RUN-MMU-001` covers the permission-separated TTBR1 switch. `RUN-MMU-002` remains open for final
TTBR0 alias retirement. The new probe narrows the first check but does not close either entry until
its output and environment are recorded in `docs/RUNTIME_VALIDATION.md`.
