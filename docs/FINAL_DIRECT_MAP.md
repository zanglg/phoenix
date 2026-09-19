# Final Direct-Map Capability

This document defines the checked bridge from allocator-managed physical frames to the active
AArch64 QEMU `virt` final TTBR1 direct map. Its planning and ownership checks are Host Tested and
Cross Compiled. The refactored target access path has not been rerun and is Runtime Pending.

## Current implementation

The physical allocator exposes a copyable `ManagedFrameSet<CAPACITY>` snapshot of its complete
reservation-filtered domain. The snapshot includes both free and allocated frames and remains
stable across allocation and release. Membership proves only that a frame originated in the
allocator's managed RAM; it does not prove current ownership.

Before a final kernel page-table plan is consumed, `final_direct_map_layout` walks every leaf that
covers the managed snapshot. For every interval it requires:

- a valid translation at `physical + KERNEL_VIRTUAL_OFFSET`;
- the exact original physical address;
- normal EL1 read/write, PXN and UXN attributes;
- continuous coverage through the end of every managed range.

The validator rejects an empty domain, a hole, a shifted physical output, incorrect permissions,
page-table errors, and address overflow. It advances by actual L1/L2/L3 leaf boundaries rather
than sampling only range endpoints.

Successful validation produces `FinalDirectMapLayout<RANGES>`. The focused final-map and
loaded-init builders derive this proof from the same plan and allocator that they subsequently
move into their permanent runtime owner. The large offline plan can then be discarded without
discarding the exact reservation-filtered RAM authorization.

After that hierarchy is active, unsafe construction of
`FinalDirectMapMemory<'a, TABLES, RANGES>` binds the proof to a borrow of the retained
`MaterializedKernelPageTables`. The lifetime prevents the access capability from outliving its
table owner. Checked translation rejects frames or contiguous ranges outside the managed snapshot,
cross-page accesses, and overflow.

The capability implements the existing private-memory interfaces for heap backing, kernel-stack
clearing, process-image population, user copies, and translation-table materialization. The
loaded-init syscall path now validates user-leaf ownership first and then uses this final-map
capability; it no longer calls the bootstrap-window raw copy helpers after TTBR1 publication.
The focused final-map path also binds the capability only after TTBR1 activation and uses it to
clear four allocator-owned heap frames before publishing the heap probe marker.

## Invariants

- the managed snapshot is derived only after firmware, kernel-image, DTB, and platform
  reservations have been removed;
- a snapshot is an address-domain proof, never an allocation or unique-ownership token;
- every accepted frame is covered by the exact final plan at the fixed offset with kernel-data
  attributes;
- the proof is built before that plan is consumed into allocator-owned table frames;
- the runtime owner permanently retains the materialized tables, proof, and corresponding physical
  allocator;
- constructing `FinalDirectMapMemory` is unsafe and requires that the retained tables came from
  the proved plan and are already active in TTBR1;
- every byte operation remains inside one checked 4 KiB frame;
- mutation additionally relies on the calling subsystem's frame owner; direct-map membership alone
  never authorizes writes;
- the current boot/runtime paths are single-core and non-reentrant. IRQ or SMP sharing requires
  synchronization and memory-ordering rules around both ownership and byte access.

## Validation

Host tests prove that managed snapshots remain stable across allocation and release. Platform
tests cover mixed large/page-leaf traversal, reservation holes in the managed domain, exact offset
translation, unmanaged frames, cross-page access, empty domains, missing leaves, wrong physical
outputs, and read-only mappings.

`cargo xtask check`, `cargo xtask lint`, and the complete `cargo xtask ci` build the proof,
capability, retained runtime fields, loaded-init user-copy adapter, and final-map heap backend for
AArch64. The same capability now backs the focused runtime-stack clear, descriptor access, physical
verification, and scrub path. ELF inspection requires both heap and stack markers in that focused
image. These checks do not execute the new target paths.

## TODO

- rerun the final-map heap/stack path and loaded-init copy path on the pinned target;
- remove or narrow the public raw direct-map helpers once every post-switch caller is migrated;
- couple proof and table materialization more tightly so an unsafe caller cannot pair artifacts
  derived from different plans;
- define IRQ-safe ownership and cache/memory-ordering rules before concurrent access;
- add target-negative probes for an unmanaged frame and a cross-page request without causing an
  uncontrolled kernel exception.

## Skipped work

This capability does not allocate frames, prove current object ownership, install or modify page
tables, map reserved firmware/kernel/device frames, provide DMA access, bypass page permissions,
register a global allocator, or make live mappings safe. It does not claim that arbitrary addresses
inside the former 1 GiB bootstrap window remain valid after the switch.

## Runtime status

`RUN-MMU-003` is Runtime Pending. Closing it requires the retained proof and exact active table
owner to serve valid managed-frame reads, writes, and clears after final TTBR1 publication while
rejecting unmanaged/cross-page requests without a target fault. The previously recorded positive
final-map and loaded-init transcripts predate this capability refactor and are not expanded into
that claim.
