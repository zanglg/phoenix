# Final AArch64 Kernel Address Space

This document describes Phoenix's implemented final `TTBR1_EL1` hierarchy for the pinned
AArch64 QEMU `virt` platform. Planning, allocation, materialization, cross-compilation, and ELF
inspection are complete. Publication on the recorded QEMU 9.1.3 AArch64 CPU path is Runtime
Verified; negative permission and guard-fault probes remain Runtime Pending.

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
| Data, BSS, boot tables, usable stack pages | `__data_start..__data_end` minus the offset, excluding the guard | Normal, read/write, non-executable |
| Boot-stack guard | `__boot_stack_guard_start..__boot_stack_guard_end` minus the offset | Deliberately unmapped |
| Other DTB-declared RAM | Complete frames in each memory node | Normal, read/write, non-executable |
| PL011 | Physical `0x09000000`, one 4 KiB page | Device-nGnRnE, read/write, non-executable |
| Runtime-stack arena | `0xffffff9000000000..0xffffff90000a0000` | Prelinked L2/L3 topology, invalid leaves; retained 16-slot pool owner |

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
the remaining 510 MiB uses 255 L2 blocks. Omitting the guard and adding the single PL011 page brings
the mapped portion to five table frames and 767 leaf mappings. The empty runtime-stack arena adds
one L2 and one L3 table without adding a leaf, bringing the current exact topology to seven table
frames and 767 leaf mappings. The configured limits are eight table frames and 1024 leaves,
so an unexpected DTB topology fails explicitly rather than truncating the address space.

The empty-topology reservation is transactional. It rejects zero or non-page-multiple lengths,
misaligned or non-upper-half starts, address overflow, existing leaf translations, and
table-capacity exhaustion. Once a child table is reserved, a coarser leaf cannot overwrite it;
page-level mappings may later populate individual L3 slots. Materialization links the reserved
parents while leaving every arena leaf descriptor invalid.

The platform builder mutates its unpublished plan with the planner's transactional operations,
avoiding repeated ownership moves of the large fixed-capacity value on the bounded early stack.
After descriptor materialization, the permanent owner discards
all leaf-planning metadata and retains only the compact table-role/frame array; a host test keeps
that owner below 512 bytes.

The QEMU platform builder verifies both ends of text, rodata, and data through the offline
translation model, proves the boot-stack guard is absent, and verifies the PL011 physical address,
page level, and device attributes before any table frame is allocated. It also checks all 160
pages of the provisional runtime-stack arena, rejects the plan if any already has a translation,
and reserves their empty L3 topology before table-frame allocation.

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

Both final-map runtime owner variants also retain one platform-created runtime-stack pool for the
reserved arena. This establishes virtual-slot ownership but does not allocate stack frames or
authorize live descriptor mutation.

This replaces the coarse higher-half bootstrap hierarchy. The standalone final-map probe
intentionally leaves `TTBR0_EL1` unchanged, so its temporary low device and RAM aliases remain.
The loaded-init path immediately replaces TTBR0 with its owned user hierarchy. General low-alias
retirement outside that path remains a separate runtime-sensitive operation.

Before the offline plan is consumed, both final-map builders now prove that every
reservation-filtered allocator frame is continuously mapped at the fixed offset with EL1 RW/NX
normal-memory attributes. They retain that compact proof beside the materialized tables and
allocator. `FinalDirectMapMemory` can be constructed only under an unsafe contract tying the proof
to those active retained tables. The loaded-init syscall adapter uses this capability after first
checking exact user-leaf ownership. Its post-refactor target execution is `RUN-MMU-003` Pending.
The standalone final-map owner also retains a bounded fallible heap. After publication it binds
the same capability to the active tables, acquires and clears four free managed frames, exercises
the documented allocation failures, and keeps both heap and remaining allocator permanently
owned.

## Focused QEMU probe

The `kernel-map-probe` image uses the real boot DTB, kernel linker bounds, reservation-aware frame
allocator, and bootstrap physical-memory backend. Before publication it emits
`PHOENIX_KERNEL_MAP_ENTER`. After the TTBR1 switch it reuses the mapped kernel text, rodata, stack,
data, and PL011 device page, initializes and exercises the bounded heap, emits
`PHOENIX_KERNEL_HEAP_OK`, then emits `PHOENIX_KERNEL_MAP_OK` and halts.

When QEMU is available, run:

```bash
cargo xtask test-kernel-map
```

The bounded harness cannot pass on the earlier boot, enter, or heap marker; both progress markers
must precede terminal success. Panic, exception, early exit, timeout, and excessive output are
failures. Output is retained in
`target/phoenix/aarch64-unknown-none-softfloat/debug/qemu-kernel-map.log`.

The `loaded-init-probe` uses the same builder and owner in its end-to-end path. It constructs both
kernel and user tables before publication, switches TTBR1 first, then installs the prepared TTBR0
user hierarchy and enters EL0. Its syscall-side direct-map adapter accepts only frames proven to
belong to the retained init address space.

## Validation

Host tests cover range decomposition, exact translation, permission replacement, deterministic
topology, table-capacity and overlap failures, atomic allocation, descriptor materialization,
retryable backend failure, and unpublished release. Platform tests cover the pinned 512 MiB
topology, critical mappings, linker-layout rejection, RAM-window rejection, exact runtime-stack
arena extent, seven-table/767-leaf result, initial leaf absence, reserved-parent materialization,
and injected mapping conflicts. Planner tests also cover reservations crossing an L3-table
boundary plus transactional alignment, half, capacity, and leaf/topology conflict failures.

The focused kernel image is Cross Compiled and ELF Inspected. Static checks require every
permission-domain symbol, page alignment and ordering, the first-2-MiB image bound, the expected
load addresses, and the heap and final success sentinels. On the recorded QEMU 9.1.3 `virt-9.1` run,
`cargo xtask test-kernel-map` reported root `0x0000000040000000`, five tables, 767 leaves, and
the ordered enter/success markers after TTBR1 publication. That run predates the new heap marker,
and also predates the two empty runtime-stack tables. It remains historical final-table evidence
but does not validate the current seven-table extended probe.

## TODO

- remove the temporary TTBR0 aliases when no low virtual pointer or bootstrap handle survives;
- migrate remaining post-switch raw helper users to the retained final-direct-map capability and
  rerun its valid/invalid target paths;
- rerun the heap-extended final-map probe and use observed consumers to replace its conformance
  sizing with an explicit production policy;
- derive the physical-address size from `ID_AA64MMFR0_EL1.PARange` before accepting RAM above
  the current 32-bit IPS envelope;
- enable caches only after cache-maintenance and memory-ordering requirements are designed and
  runtime tested;
- replace the provisional stack arena sizing with measured policy, then implement the AArch64
  backend under the mapped-stack ownership contract for thread and exception stacks;
- implement live mapping updates with break-before-make, local versus broadcast TLB maintenance,
  ASIDs, rollback, and SMP synchronization;

## Skipped work

This implementation does not provide dynamic kernel mappings, general heap consumers, per-CPU
tables, recursive mapping, KASLR, copy-on-write, demand paging, swapping, transparent huge pages,
16/64 KiB granules,
52-bit addresses, MTE, stage-2 translation, DMA/IOMMU mappings, or SMP TLB shootdown. It maps all
DTB-declared RAM into an EL1-only direct map; it does not use mapping permissions as an ownership
substitute for firmware-reserved or allocator-owned frames.

## Runtime status

`RUN-MMU-001` is Runtime Verified by the recorded focused probe. `RUN-MMU-002` remains open for
final TTBR0 alias retirement, `RUN-MMU-003` covers the new final-map access capability, and
controlled permission/guard faults remain separate Pending work.
