# Fallible Kernel Heap

This document defines Phoenix's first kernel heap. The allocator core is Host Tested and Cross
Compiled, but it is not a global Rust allocator and is not yet initialized by an AArch64 boot
path. Target execution therefore remains Runtime Pending.

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
  during single-core, non-reentrant execution. Before IRQ handlers, preemption, or SMP share it,
  Phoenix must add an IRQ-safe lock and document acquire/release ordering around both heap
  metadata and object publication.

## Validation

Host tests cover initialization-before-use, repeated initialization, zero metadata capacity,
invalid frame requests, physical OOM, virtual-address failure, misalignment and overflow, complete
frame clearing, injected clear failure and retry, byte alignment, zero-size requests, fragmented
OOM, implicit coalescing, metadata exhaustion, duplicate/foreign/modified tokens, busy backing
release, contiguous-frame restoration, and stale-epoch rejection. Physical allocator tests also
cover transactional contiguous release, coalescing, duplicate release, and partly foreign ranges.

`cargo xtask ci` cross-compiles this code into every configured AArch64 kernel variant and
performs the existing ELF inspections. Static inclusion does not prove that an AArch64 boot path
has initialized or used the heap.

## TODO

- initialize a bounded heap after final TTBR1 publication using the implemented
  `FinalDirectMapMemory` backend and the retained physical allocator;
- select backing size and fixed live-allocation capacity from observed kernel consumers;
- move suitable fixed boot plans and later VFS/process/scheduler ownership into explicit heap
  allocations while preserving their recoverable OOM paths;
- introduce typed owning containers that cannot outlive their `HeapAllocation` token;
- decide whether allocation bytes are cleared on every reuse or only when crossing a security
  ownership boundary; backing initialization is currently fully cleared;
- add bounded allocation accounting, subsystem quotas, and useful OOM diagnostics without
  allocating in the failure path;
- define an IRQ-safe lock, interrupt-context allocation policy, lock ordering, and memory-ordering
  rules before concurrent use;
- evaluate a more scalable allocator only after real allocation-size and lifetime traces exist.

## Skipped work

Phoenix does not register this heap as `#[global_allocator]`, implement infallible `Box`/`Vec`
allocation, map pages dynamically, grow or shrink backing, allocate discontiguous backing,
reclaim memory under pressure, compact live allocations, provide per-CPU caches, support IRQ/SMP
access, or wire the heap into boot. OOM is returned to the caller; the heap does not panic, invoke
an OOM handler, or silently overcommit.

## Runtime status

`RUN-KHEAP-001` is Runtime Pending. Closing it requires a boot path to initialize the heap only
after final TTBR1 publication, demonstrate that its frames were removed from the retained physical
allocator, exercise aligned success plus recoverable byte and metadata exhaustion, release and
reuse allocations without corruption, and retain deterministic kernel progress. No existing
positive boot, final-map, EL0, or loaded-init transcript supplies that evidence.
