# AArch64 Paging

This document describes both the temporary boot translation regime and the Host Tested stage-1
mapping mechanisms. A focused image now constructs and publishes final `TTBR1_EL1` tables, but it
has not been exercised on an AArch64 CPU. The platform layout and publication boundary are in
`docs/FINAL_KERNEL_ADDRESS_SPACE.md`.

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
more conservative 32-bit IPS until CPU feature discovery replaces its fixed TCR value. It leaves
`TCR_EL1.AS` clear for an 8-bit ASID field; process ASIDs start at one and are described in
`docs/AARCH64_ASIDS.md`.

## Temporary bootstrap tables

`boot.S` owns one statically allocated L1 table and installs it through both translation-table
base registers. It maps low MMIO as Device-nGnRnE, RAM at `0x40000000..0x7fffffff` through a low
identity alias, and the same RAM through the linked higher-half alias. These 1 GiB blocks are
writable and too coarse for the final security boundary. Their only purpose is to enable the MMU,
switch to the higher-half stack, and enter Rust.

The focused final-kernel-map image replaces the coarse TTBR1 hierarchy but intentionally keeps
TTBR0 unchanged. The low alias must remain until the DTB pointer and every live physical reference
have been migrated to their final virtual form. Removing it requires a distinct TLB maintenance
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
links and leaves through a private-table backend. `kernel_page_table.rs` builds a separate
mixed-level upper-half hierarchy, applies ordered page-granular permission overrides or explicit
holes to a direct map, atomically owns its table frames, and exposes publication only through a
permanent borrow.
See `docs/AARCH64_USER_PAGE_TABLES.md` and `docs/FINAL_KERNEL_ADDRESS_SPACE.md` for their distinct
ownership boundaries.

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
- the generic offline mapping plan does not own memory;
- materialized kernel tables own every table frame, and target publication requires their
  permanent retention;
- user mappings additionally retain populated leaf-frame ownership because those pages are not a
  permanent physical direct map.

## Validation

Host tests cover canonical boundaries, table indices, mapping sizes, descriptor types and output
addresses, privilege and execute bits, W^X rejection, alignment, physical width, deterministic
ordering, overlap, capacity, translation, unmapping, mixed-level direct-map decomposition,
permission overrides, exact guard holes, table ownership, and retry-safe materialization. The
focused final-map image is also Cross Compiled and ELF Inspected with the configured AArch64
bare-metal target.

Runtime installation remains tracked as `RUN-MMU-001`, `RUN-MMU-002`, and `RUN-KSTACK-001` in
`docs/RUNTIME_VALIDATION.md`.

## TODO

- derive supported physical-address size from `ID_AA64MMFR0_EL1.PARange`;
- replace the bootstrap private-table backend and retained ASID-1 activation with process-table
  address-space ownership;
- add a guarded heap and replace the static guarded boot stack with owned thread/exception stacks;
- define live table-update locking and break-before-make beyond the current one-time publication;
- define safe ASID retirement, epochs, and reuse after TTBR0 lifetime ends;
- runtime-validate final `TTBR1_EL1`, then retire the temporary low RAM alias safely;
- decide whether a recursive/self-map region is needed alongside the implemented direct map.

## Skipped work

Five-level or 52-bit translation, 16/64 KiB granules, transparent huge pages, copy-on-write,
demand paging, swapping, KASLR, memory tagging, stage-2 translation, IOMMU tables, and SMP TLB
shootdown are intentionally outside this module. They remain candidates in `FEATURES.md`, not
implicit behavior of the current plan.
