# User Address Space

This document defines the architecture-neutral validation and planning layer for Phoenix's first
EL0 address spaces. The separate process-image layer assigns and populates frames transactionally,
and the AArch64 layer materializes unpublished translation tables. Activation remains an
architecture-specific operation on the combined owner.

## Current implementation

The initial AArch64 regime gives EL0 the lower 39-bit region:

```text
0x0000000000000000                           null page, always unmapped
0x0000000000001000                           first mappable user address
...
0x0000007ffffff000                           default initial stack top
0x0000008000000000                           first non-user address
```

`UserAddr` accepts dereferenceable addresses below `2^39`. `UserRange` is a non-empty half-open
range and permits its exclusive end to equal `2^39`. This distinction lets the model represent the
last byte of the region without treating the limit itself as a valid pointer.

`UserAddressSpacePlan<CAPACITY>` is a fixed-capacity, allocation-free set of page-aligned mappings.
It sorts by virtual start, rejects overlap and the null page, and records permissions plus a small
purpose tag. It does not own backing frames or page-table entries.

`UserStackLayout` places a downward-growing stack below a chosen aligned top and requires one or
more unmapped guard pages. The default top leaves the highest user page unused, so the initial
stack pointer itself remains canonical and can be 16-byte aligned.

The native initial-stack builder can move that pointer downward while constructing argc, argv,
envp, and auxiliary-vector bytes. Process-image population clears and initializes those stack pages
before any translation root is publishable. The exact ABI is in `docs/INITIAL_USER_STACK.md`.

The first `copy_from_user` implementation validates a complete range against populated
virtual-to-physical ownership metadata, then reads through an explicit physical-memory backend. It
never creates a reference from an untrusted user virtual pointer. See `docs/USER_COPY.md`.

## Permission policy

User permissions are explicit read/write/execute booleans with these enforced rules:

- a mapping must grant at least one permission;
- writable-executable mappings are rejected;
- program text is normally read/execute;
- read-only data is non-executable;
- data and stack are read/write and non-executable;
- guard pages are absent mappings, not no-permission descriptors.

The generic permission type is deliberately separate from AArch64 descriptor bits. Materialization
translates it through the architecture layer without exposing an AArch64 type to process code. The
current AArch64 table owner is documented in `docs/AARCH64_USER_PAGE_TABLES.md`.

## Invariants

- user addresses never enter the 39-bit canonical gap or kernel half;
- active mappings are non-empty, page aligned, ordered, and disjoint;
- page zero is never mapped;
- no active mapping is writable and executable;
- guard and usable stack ranges touch but do not overlap;
- capacity exhaustion is reported before mutating a full plan;
- a plan describes intent only and cannot imply physical ownership.

## Validation

Host tests cover the user boundary, range-end sentinel, address overflow, W^X, empty permissions,
null and unaligned mappings, sorting, lookup, overlap, capacity exhaustion, guarded-stack placement,
bad page counts, and stack underflow. The same module is Cross Compiled for AArch64.

## TODO

- replace fixed ASID-zero TTBR0 installation with process-owned ASID allocation and retirement;
- add `copy_to_user` and general recoverable-fault support beyond immutable owned mappings;
- define stack growth limits and resource accounting;
- validate kernel isolation and guard-page faults on QEMU.

## Skipped work

Demand paging, copy-on-write, shared mappings, memory-mapped files, swapping, overcommit, userfault,
transparent huge pages, ASLR, JIT writable/executable transitions, and multiple threads are not part
of the initial plan. The lower address-space shape is not yet a stable ABI promise.
