# AArch64 Address-Space Identifiers

This document describes Phoenix's initial AArch64 ASID policy, allocation model, TTBR0 encoding,
and explicit reuse deferral. The pure model is Host Tested and both EL0 paths are Cross Compiled;
target register effects remain Runtime Pending.

## Current translation regime

The bootstrap leaves `TCR_EL1.AS` clear, selecting the architecturally required 8-bit ASID field in
`TTBR0_EL1[63:48]`. Phoenix therefore accepts values 0 through 255 at the hardware level but
reserves ASID 0 for bootstrap translations. Process address spaces use only 1 through 255.

`AddressSpaceId` can contain only a nonzero 8-bit value. `ttbr0_value` combines that value with the
page-aligned physical L1 root while rejecting a root outside the implemented 48-bit BADDR field.
The user-table materializer has already validated its root through a table descriptor, so target
activation treats a composition failure as a violated internal invariant rather than silently
truncating the address.

## Allocation policy

`AsidAllocator` issues the lowest never-before-issued value, beginning with 1. It is monotonic and
has no release operation. After ASID 255 it reports `Exhausted` and remains exhausted.

This deliberate limit is safer than immediate reuse. Reusing an ASID requires a specified epoch,
complete invalidation of stale translations on every relevant processing element, ordering around
TTBR publication, and coordination with any CPU still executing or caching the old address space.
Phoenix has not implemented those mechanisms and therefore cannot safely recycle an identifier.

## Current activation

Both the static EL0 conformance probe and the dynamically loaded init path use ASID 1. Before the
first EL0 fetch, the architecture entry path:

1. executes `DSB ISHST` after all descriptor writes;
2. writes the root plus nonzero ASID to `TTBR0_EL1`;
3. executes `ISB`;
4. conservatively executes `TLBI VMALLE1`;
5. completes `DSB ISH` and `ISB`;
6. installs SP, PC, and PSTATE and executes `eret`.

The loaded-init runtime permanently retains both the issued identifier and its allocator state
beside the prepared address space. The static probe has one fixed address space for the lifetime of
the boot. Neither path reuses ASID 1.

The all-ASID local invalidation is intentionally conservative for the current single CPU. A future
context switch should avoid it when installing a never-used ASID and use an exact, documented
invalidation protocol on reuse.

## Invariants

- ASID zero never identifies a process address space;
- the active 8-bit field is never populated by truncating a wider value;
- the TTBR root and ASID bit fields are checked independently before composition;
- one allocator never issues the same ASID twice;
- exhaustion never wraps or changes allocator state;
- an ASID remains retained for at least as long as its tables and possible cached translations;
- ASID identity supplements table/frame ownership and never replaces it;
- no reuse is permitted until TLB invalidation and cross-CPU lifetime rules are implemented.

## Validation completed without QEMU

Host tests cover zero and out-of-range rejection, all 255 monotonic allocations, sticky exhaustion,
exact TTBR0 bit composition, and rejection of a root above 48 bits. The kernel library and static
and loaded EL0 variants are Cross Compiled for `aarch64-unknown-none-softfloat`.

Runtime evidence is tracked as `RUN-ASID-001` in `docs/RUNTIME_VALIDATION.md`. Compilation cannot
prove that QEMU observes the encoded ASID or that the maintenance sequence has the intended effect.

## TODO

- read `ID_AA64MMFR0_EL1.ASIDBits` and enable 16-bit ASIDs only together with `TCR_EL1.AS`;
- define ASID epochs and generation rollover instead of extending the current monotonic allocator;
- retire TTBR0 safely before returning address-space frames or an ASID to reusable pools;
- specify `TLBI ASIDE1IS`/range invalidation, barriers, and completion for local and SMP cases;
- bind the ASID, tables, and process identity in one table-owned address-space resource;
- add context-switch rules for never-used, inactive, active, and migrated address spaces;
- runtime-verify TTBR0's ASID field and stale-translation rejection under QEMU/GDB.

## Skipped work

Phoenix does not yet support ASID reuse, 16-bit ASIDs, per-CPU active-address-space tracking,
remote TLB shootdown, live address-space migration, kernel page-table identifiers, virtualization
VMIDs, or performance claims about context switching. The 255-process-lifetime bound is explicit
development behavior, not the intended long-term capacity.
