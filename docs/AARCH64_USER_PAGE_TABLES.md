# AArch64 User Translation Tables

This document describes the ownership, materialization, and activation layer for lower-half
AArch64 user page tables. It builds the same 39-bit, 4 KiB, three-level format configured by the
bootstrap. Planning and ownership are Host Tested; target activation is Cross Compiled and Runtime
Pending.

## Current implementation

`UserPageTablePlan<TABLES, LEAVES>` begins with one mandatory L1 root and accepts page-aligned user
virtual addresses, populated physical frames, and generic final permissions. It automatically
creates and reuses:

- one L2 table for each occupied L1 index;
- one L3 table for each occupied `(L1, L2)` pair;
- one L3 leaf for each mapped 4 KiB user page.

Tables are retained in parent-before-child order and leaves in virtual-address order. Adding one
page or a complete `PopulatedProcessImage` is transactional: a duplicate page, unsupported
permission, descriptor error, or metadata limit leaves the previous plan unchanged.

AArch64 stage-1 permissions cannot represent an unreadable write-only or execute-only user page,
so this layer rejects generic mappings without read access. Current ELF, data, and stack policies
all include read permission. Read/execute pages are EL0-executable and EL1-never-execute;
read/write pages are execute-never; read-only data is execute-never. All user leaves are non-global.

## Table-frame ownership

`UserPageTablePlan::allocate` assigns frames for the root and every intermediate table against a
private allocator snapshot. It commits only when all frames exist and every physical address can
be encoded by the current 48-bit descriptor model. Failure leaves the allocator unchanged.

The resulting `AllocatedUserPageTables` is neither `Copy` nor `Clone`. It represents unique table
frame ownership but not ownership of the leaf data frames. Its `materialize` operation uses the
narrow `TranslationTableMemory` backend to:

1. clear all 512 entries in every private table frame;
2. link the L1 root to all L2 tables;
3. link L2 tables to all required L3 tables;
4. write final L3 user leaves.

Only a complete pass yields `MaterializedUserPageTables`. A backend failure identifies clear,
intermediate link, or final leaf work and returns the allocated ownership for a clean retry. An
unpublished materialized hierarchy can be released transactionally.

The QEMU `virt` bootstrap now supplies one checked target implementation of this backend while the
temporary TTBR1 RAM block remains active. It is not the final direct map and is Runtime Pending.

## Combined ownership before activation

The materialized table object owns only translation-table frames. Its L3 descriptors refer to
physical pages owned separately by `PopulatedProcessImage`. Neither object alone is sufficient to
authorize EL0 entry.

`PreparedUserAddressSpace::new` consumes both owners and compares every sorted leaf's virtual
address, physical frame, and permissions with every populated resident page. A count or identity
mismatch returns both original owners. Success is the first type state that holds all frames needed
by the inactive address space; its unpublished release reclaims data and table frames together as
one allocator transaction.

This restriction is intentional. A root physical address is useful for static inspection, but the
target activation API requires a permanently retained combined address-space owner, never a bare
table root or page plan. It publishes table writes with `DSB ISHST`, replaces `TTBR0_EL1`, completes an ASID-zero
stage-1 invalidation with `ISB`, `TLBI VMALLE1`, `DSB ISH`, and `ISB`, loads `SP_EL0`, `ELR_EL1`, and
masked EL0t state, then executes `eret`. The register primitive is crate-private so other modules
cannot bypass the ownership gate.

The loaded-init path places the owner and remaining allocator state in a one-time static runtime
slot before activation. The `'static` activation borrow prevents unpublished release while active,
and the same owner authorizes physical-frame-based user reads during synchronous syscalls. The
current terminal `exit` path halts rather than reclaiming it. Retirement and reclamation require a
process owner and ASID-aware switch path.

## Invariants

- only lower-half, page-aligned, readable user mappings are accepted;
- every user virtual page has exactly one leaf;
- every required intermediate table exists exactly once;
- topology and leaf updates are all-or-nothing;
- all table frames are uniquely owned and allocated transactionally;
- table memory is fully cleared before any descriptor is installed;
- parent descriptors point only at owned, validated table frames;
- leaves use populated frames and final W^X-safe user permissions;
- backend failure never converts partial table bytes into the materialized type state;
- combined ownership requires an exact page-for-leaf identity match;
- only the combined owner can invoke the target TTBR0/EL0 transition;
- table release is allowed only before publication to a live translation regime.

## Validation

Host tests cover table reuse across adjacent pages, distinct L2 and L1 regions, deterministic
ordering, unaligned and unreadable pages, duplicates, table and leaf capacity, transactional
out-of-memory behavior, exact descriptor links and outputs, injected materialization failure,
clean retry, leaf-identity mismatch, combined ownership, and atomic complete release. The module is
Cross Compiled for the AArch64 bare-metal target, including the ownership-retaining activation
path. A focused loaded-init variant now reaches this API with allocator-owned program, stack, and
table frames. Runtime register, barrier, invalidation, permission, and `eret` effects remain
unverified.

## TODO

- replace the bootstrap implementation with the final physical direct map;
- assign and recycle nonzero ASIDs with generation handling;
- add ASID-aware retirement and reclamation after process ownership exists;
- define break-before-make for changes to published descriptors;
- retain page-table accounting in the future process object;
- add copy-to-user and recoverable fault handling for mutable/concurrent address spaces;
- validate descriptor walks, access permissions, guard faults, and switching under QEMU/GDB.

## Skipped work

This module does not build the final TTBR1 kernel hierarchy, expose a permanent physical direct
map, modify live tables, handle multiple CPUs, shoot down remote TLBs, implement demand paging,
copy-on-write, shared memory, huge pages, ASLR, or reclaim leaf data. The static `el0-probe` keeps
its deliberately separate linked tables until the production address-space owner is complete.
