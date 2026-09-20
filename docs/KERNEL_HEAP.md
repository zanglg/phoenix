# Fallible Kernel Heap

This document defines Phoenix's first kernel heap. The allocator core is Host Tested, and the
focused AArch64 final-map image now Cross Compiles a bounded post-TTBR1 initialization path. It is
not a global Rust allocator. The extended target path has not run, so target execution remains
Runtime Pending.

## Current implementation

`memory::KernelHeap<ALLOCATIONS>` manages byte allocations inside one contiguous range of
physical frames. Initialization clones the physical frame allocator, acquires the complete range
from that candidate, resolves its virtual start through `HeapBackingMemory`, validates the full
virtual extent, and clears every frame. Only after all of those steps succeed are both the heap
and physical allocator committed. Allocation metadata stays in the `KernelHeap` value, so the
heap never needs to allocate memory in order to describe its own free space.

The current API is deliberately fallible and explicit:

- zero-size allocation is rejected;
- alignment must be a nonzero power of two and is applied to the returned virtual address;
- checked arithmetic rejects byte-count, address, and alignment overflow;
- physical-frame exhaustion, heap-byte exhaustion, and fixed allocation-metadata exhaustion are
  distinct results;
- allocation uses deterministic address-ordered first fit;
- release validates the exact heap, lifecycle epoch, allocation identifier, address, size, and
  alignment before changing state;
- duplicate release, a token from another heap or epoch, and a modified token are distinguishable;
- removing a record makes adjacent holes one logical free range, so coalescing needs no separate
  fallible free-list mutation;
- backing cannot be returned while an allocation is live;
- successful backing release is one transactional contiguous-frame update and permits deliberate
  reinitialization with a new lifecycle epoch.

`HeapBackingMemory` is the only memory-access boundary. It supplies the virtual start of the exact
owned frame range and clears one private frame at a time. A host backend can use ordinary test
storage. The AArch64 `FinalDirectMapMemory` backend is now Cross Compiled and accepts only the
reservation-filtered managed domain proved against the retained final TTBR1 owner. Merely having a
direct-map descriptor does not confer ownership: the frames must first have been removed from the
physical allocator by `KernelHeap::initialize`.

The heap returns `HeapAllocation` tokens rather than raw references or pointers. The address in a
token is useful only while that token and the heap backing remain live. Higher-level owning types
must preserve this lifetime before any safe reference-producing API can be added.

The `kernel-map-probe` runtime owner contains the materialized final tables, their direct-map
proof, the surviving physical allocator, and an initially empty `KernelHeap<4>`. Only after the
owner is installed and TTBR1 is published does the probe bind `FinalDirectMapMemory` to those
exact tables and transactionally acquire four contiguous frames. The probe checks a 64-byte
aligned allocation, release to the same first-fit address, recoverable byte exhaustion,
recoverable four-record metadata exhaustion, complete token release, exact physical-frame
retention, and continued progress. It then emits `PHOENIX_KERNEL_HEAP_OK`; the existing
`PHOENIX_KERNEL_MAP_OK` remains terminal and is accepted only after the subsequent runtime-stack
probe also emits `PHOENIX_KERNEL_STACK_OK`.

Four frames and four live records are deliberately a conformance bound, not a production sizing
claim. No other boot variant or subsystem consumes heap allocations yet.

## Ownership and failure invariants

- an initialized heap uniquely owns its complete `FrameRange`; those frames are absent from the
  physical allocator until `release_backing` succeeds;
- initialization failure never publishes a partial heap or consumes frames from the caller's
  allocator;
- a clear failure may have zeroed candidate frames, but ownership was never published and the
  original allocator remains unchanged;
- every backing frame is cleared before the first allocation can be returned;
- active records remain sorted, non-overlapping, nonempty, and wholly inside the backing range;
- allocation failure does not mutate records or consume an allocation identifier;
- deallocation failure does not mutate records;
- backing release fails while any allocation is live and is transactional with the physical
  allocator;
- lifecycle epochs prevent a token from an earlier initialization of the same heap object from
  freeing a new allocation;
- the current type has one mutable owner and contains no synchronization. It may be used only
  during single-core, non-reentrant execution. Phoenix now has an IRQ-save lock primitive, but
  before IRQ handlers, preemption, or SMP share the heap an owning wrapper must adopt it and define
  interrupt-context allocation, lock ordering, and publication policy.

## Validation

Host tests cover initialization-before-use, repeated initialization, zero metadata capacity,
invalid frame requests, physical OOM, virtual-address failure, misalignment and overflow, complete
frame clearing, injected clear failure and retry, byte alignment, zero-size requests, fragmented
OOM, implicit coalescing, metadata exhaustion, duplicate/foreign/modified tokens, busy backing
release, contiguous-frame restoration, and stale-epoch rejection. Physical allocator tests also
cover transactional contiguous release, coalescing, duplicate release, and partly foreign ranges.

`cargo xtask ci` cross-compiles this code into every configured AArch64 kernel variant and
performs the existing ELF inspections. The final-map inspection additionally requires the heap
success marker, and host tests require it to precede final-map success in the runtime protocol.
Static inclusion does not prove that the AArch64 path initialized or used the heap.

## TODO

- rerun the extended final-map probe on the pinned QEMU board and record the exact heap evidence;
- select production backing size and fixed live-allocation capacity from observed kernel
  consumers rather than the four-frame/four-record conformance bound;
- move suitable fixed boot plans and later VFS/process/scheduler ownership into explicit heap
  allocations while preserving their recoverable OOM paths;
- introduce typed owning containers that cannot outlive their `HeapAllocation` token;
- decide whether allocation bytes are cleared on every reuse or only when crossing a security
  ownership boundary; backing initialization is currently fully cleared;
- add bounded allocation accounting, subsystem quotas, and useful OOM diagnostics without
  allocating in the failure path;
- wrap the heap in the existing IRQ-save lock only after defining interrupt-context allocation,
  lock ordering, and object-publication rules;
- evaluate a more scalable allocator only after real allocation-size and lifetime traces exist.

## Skipped work

Phoenix does not register this heap as `#[global_allocator]`, implement infallible `Box`/`Vec`
allocation, map pages dynamically, grow or shrink backing, allocate discontiguous backing,
reclaim memory under pressure, compact live allocations, provide per-CPU caches, support IRQ/SMP
access, or make loaded init depend on heap allocation. OOM is returned to the caller; the heap
does not invoke an OOM handler or silently overcommit. The focused boot probe treats failure of
its required initialization contract as fatal, while deliberately induced byte and metadata OOM
remain returned values that the probe checks without panicking.

## Runtime status

`RUN-KHEAP-001` is Runtime Pending. The required boot path and ordered
`PHOENIX_KERNEL_HEAP_OK`/`PHOENIX_KERNEL_STACK_OK`/`PHOENIX_KERNEL_MAP_OK` protocol now exists,
but has only been Cross Compiled and ELF Inspected. Closing the item requires
`cargo xtask test-kernel-map` to demonstrate
post-TTBR1 initialization, exact frame retention, aligned success, recoverable byte and metadata
exhaustion, release/reuse, and continued kernel progress. The earlier positive final-map transcript
predates this path and supplies none of that evidence.
