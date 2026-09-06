# AArch64 Paging

This document describes both the temporary boot translation regime and the Host Tested model for
the final EL1 stage-1 tables. It does not claim that final tables have been installed or exercised
on an AArch64 CPU.

## Translation regime

Phoenix uses a 4 KiB granule and 39-bit virtual-address regions. Translation starts at level 1,
with 512 entries at every level:

| Level | Virtual-address bits | Leaf size |
| --- | --- | --- |
| L1 | 38:30 | 1 GiB block |
| L2 | 29:21 | 2 MiB block |
| L3 | 20:12 | 4 KiB page |

`TTBR0_EL1` covers `0x0000000000000000..0x0000007fffffffff`; `TTBR1_EL1` covers
`0xffffff8000000000..0xffffffffffffffff`. Addresses in the gap are rejected by the software
model. The kernel link offset is `0xffffff8000000000`, so physical RAM beginning at
`0x40000000` has its initial higher-half alias at `0xffffff8040000000`.

The current descriptor model supports physical output addresses below 48 bits. Bootstrap uses a
more conservative 32-bit IPS until CPU feature discovery replaces its fixed TCR value.

## Temporary bootstrap tables

`boot.S` owns one statically allocated L1 table and installs it through both translation-table
base registers. It maps low MMIO as Device-nGnRnE, RAM at `0x40000000..0x7fffffff` through a low
identity alias, and the same RAM through the linked higher-half alias. These 1 GiB blocks are
writable and too coarse for the final security boundary. Their only purpose is to enable the MMU,
switch to the higher-half stack, and enter Rust.

The low alias must remain until execution, the stack, the DTB pointer, and every live physical
reference have been migrated to their final virtual form. Removing it requires a TLB maintenance
sequence and runtime evidence; it is not implied by the current implementation.

The opt-in EL0 conformance probe replaces TTBR0 with static three-level user tables after moving
console access to the TTBR1 device alias. It does not replace the coarse TTBR1 kernel mapping and
does not count as the final address space. See `docs/EL0_PROBE.md`.

## Host Tested model

`kernel/src/arch/aarch64/paging.rs` provides:

- canonical-address validation and L1/L2/L3 index extraction for both address-space halves;
- 1 GiB block, 2 MiB block, 4 KiB page, and next-table descriptor construction;
- normal-WBWA and Device-nGnRnE attribute-index selection compatible with the bootstrap MAIR;
- explicit EL1-only or EL0-accessible read/write permissions;
- EL1 or EL0 execution domains, with writable-executable mappings rejected;
- a fixed-capacity, allocation-free `MappingPlan` that sorts mappings, rejects overlap, translates
  addresses, and removes exact mappings.

The plan deliberately models intended mappings without allocating page-table frames or mutating
live tables. This separates range and permission validation from system-register and TLB effects.

`kernel/src/arch/aarch64/user_page_table.rs` builds the lower-half user hierarchy on top of these
descriptors. It reuses intermediate tables, atomically allocates their frames, and materializes all
links and leaves through a private-table backend. See `docs/AARCH64_USER_PAGE_TABLES.md` for the
ownership boundary; final TTBR1 work remains separate.

## Permission policy

The supported constructors encode these baseline rules:

- kernel text: read-only, executable at EL1, never executable at EL0;
- kernel read-only data: read-only and never executable;
- kernel data and device mappings: read/write and never executable;
- user text: read-only, executable at EL0, never executable at EL1, non-global;
- user data: read/write, never executable, non-global.

No writable mapping can be executable. An EL0 execution permission requires EL0 access, and an
EL1 execution permission requires an EL1-only mapping. Device memory is always exposed by the
non-executable convenience constructor.

## Invariants

- all table addresses and L3 outputs are 4 KiB aligned;
- block outputs are aligned to their block size;
- a planned mapping remains within one canonical TTBR half;
- planned virtual ranges do not overlap and remain sorted;
- descriptor physical addresses fit their configured field;
- an invalid descriptor is zero;
- the generic offline mapping plan does not own memory; only the prepared user address-space type
  combines materialized table frames with populated leaf-frame ownership.

## Validation

Host tests cover canonical boundaries, table indices, mapping sizes, descriptor types and output
addresses, privilege and execute bits, W^X rejection, alignment, physical width, deterministic
ordering, overlap, capacity, translation, and unmapping. The module is also Cross Compiled with
the configured AArch64 bare-metal target.

Runtime installation remains tracked as `RUN-MMU-001` and `RUN-MMU-002` in
`docs/RUNTIME_VALIDATION.md`.

## TODO

- derive supported physical-address size from `ID_AA64MMFR0_EL1.PARange`;
- replace the bootstrap private-table backend and retained ASID-zero activation with process-owned
  address-space lifecycle support;
- map kernel text, rodata, data, stack, heap, DTB, and MMIO with precise permissions;
- define table-update locking, break-before-make, barriers, and TLB invalidation;
- define ASID allocation and TTBR0 lifetime for processes;
- install final `TTBR1_EL1`, then retire the temporary low RAM alias safely;
- add guard pages around kernel stacks;
- decide whether direct-map and recursive/self-map regions are needed.

## Skipped work

Five-level or 52-bit translation, 16/64 KiB granules, transparent huge pages, copy-on-write,
demand paging, swapping, KASLR, memory tagging, stage-2 translation, IOMMU tables, and SMP TLB
shootdown are intentionally outside this module. They remain candidates in `FEATURES.md`, not
implicit behavior of the current plan.
