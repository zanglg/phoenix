# Kernel Stacks

This document describes the stack protection that Phoenix currently implements and the larger
kernel-stack policy that remains to be built. The linker-owned boot guard becomes effective only
after the final AArch64 `TTBR1_EL1` hierarchy is installed; the owned runtime-stack mechanism is
currently a Host Tested allocation, ownership, and restricted live-leaf-mapping foundation.

## Current implementation

Phoenix now has two separate stack foundations.

### Boot-construction stack

The linker reserves this ordered physical and higher-half virtual layout inside the writable
kernel image:

```text
page-aligned BSS end
optional alignment padding
4 KiB boot-stack guard (unmapped by final TTBR1)
1 MiB boot-construction stack (RW, PXN, UXN)
optional statically linked probe storage
kernel image end
```

`__boot_stack_guard_start` and `__boot_stack_guard_end` describe exactly one page, and the linker
requires the guard end to equal `__boot_stack_bottom`. The complete image, including the guard's
physical frame, remains reserved from the physical allocator. Reserving a frame and mapping it are
separate decisions: the final direct-map planner deliberately emits no descriptor for the guard.

The bootstrap's initial coarse mappings still cover this page. The guard becomes effective only
when `kernel-map-probe` or `loaded-init-probe` publishes the final TTBR1 hierarchy. Both paths keep
executing on the higher-half boot stack across that switch. The loaded-init path subsequently
replaces TTBR0 as well, so it retains no ordinary low-address alias to the guard frame.

The stack is intentionally large because the current allocation-free debug path constructs fixed
page-table and process-image plans on the stack. The first real final-map run showed that the
former 512 KiB reservation was insufficient: its outer construction frame and nested planner
frame exceeded it. The linker now reserves 1 MiB, while the platform builder uses in-place
transactional mapping to reduce the nested frame. This is an observed runtime correction, not an
automated compiler-enforced bound.

### Owned runtime-stack plans

`memory::KernelStackPool<SLOTS, STACK_PAGES>` manages a caller-selected, page-aligned virtual
arena without allocating metadata. Every deterministic lowest-free slot has this layout:

```text
4 KiB lower guard (unmapped)
STACK_PAGES private usable frames (EL1 RW, PXN, UXN)
4 KiB upper guard (unmapped)
```

The stack grows down from the exclusive end of the usable range, immediately below the upper
guard. Both guard pages consume virtual address space but no physical frames. The generic pool
does not choose an AArch64 address. Platform integration must supply a non-overlapping arena and
retain one pool owner; the QEMU `virt` policy described below now does so for both final-TTBR1 boot
paths.

Allocation clones the physical allocator, acquires all usable frames, then clears every frame
through `KernelStackMemory`. Only complete success commits the allocator and marks the virtual
slot allocated. The resulting non-copyable `OwnedKernelStack` retains the slot generation,
intended `Thread` or `Exception` role, guarded layout, and frames in increasing virtual order.
Unpublished release validates the exact pool and generation and returns every frame as one
transaction before recycling the slot. Generation overflow retires a slot instead of aliasing an
old owner. Failed allocation can have cleared candidate frames, but it does not publish their
ownership and leaves the original physical allocator unchanged.

The QEMU `virt` platform now assigns the previously generic pool one provisional virtual arena:
`0xffffff9000000000..0xffffff90000a0000`. It contains 16 slots, each with eight usable pages and
two guard pages. Every final-map build walks all 160 pages and rejects any existing leaf
translation in that interval, then reserves and materializes the arena's L2/L3 topology with every
L3 leaf invalid. This adds one L2 and one L3 table to the pinned hierarchy so a future live mapper
will not need to allocate or link intermediate tables after TTBR1 publication. Both the focused
final-map and loaded-init runtime owners retain exactly one `QemuVirtKernelStackPool`, so later
allocation cannot accidentally construct competing pool metadata for that boot path. The sizes
are an initial bounded policy for integration, not measured production stack sizing.

The owned stack can supply page-by-page pairs to an offline architecture page-table plan. Host
tests use the real AArch64 planner to map only usable pages with `kernel_data` attributes and prove
both guards remain absent. No boot path invokes live table mutation, switches the stack pointer, or
enters an exception on these owners yet.

`KernelStackMappingBackend` and `MappedKernelStack` now define the ownership boundary for future
live integration. Publishing consumes `OwnedKernelStack`; success couples it to a unique backend
mapping token, while failure returns the unchanged owner. The mapped state exposes its role,
guarded layout, and aligned initial SP, but deliberately has no frame/slot release operation.
Unpublishing must first consume the exact token; success recovers the unpublished owner, while
failure returns the complete mapped state so ownership cannot disappear. The mapped owner is
intentionally neither `Send` nor `Sync` until cross-CPU transfer and TLB shootdown are defined.
The trait's low-level map/unmap methods are themselves `unsafe`; safe callers can reach them only
through these consuming type-state transitions and therefore cannot retain an unpublished release
path beside a live token.

Implementing the backend is `unsafe`. Its contract requires exact EL1 RW/NX usable-page mappings,
invalid guards, all-or-nothing publication, and completed descriptor/TLB visibility. Unpublication
must remove all usable pages and complete TLB maintenance before success; failure must preserve or
restore the complete mapping and return its token. The contract also requires serialization against
IRQ, preemption, and SMP mutation.

`Aarch64KernelStackMapper` now implements that narrow contract for the prelinked arena. It locates
only L3 frames owned by the retained `MaterializedKernelPageTables`, checks both guards and every
usable descriptor, and constructs exact `kernel_data` leaves before changing memory. All fallible
reads and validation finish first. The commit phase performs infallible aligned descriptor stores,
then `DSB ISHST`, one `TLBI VAAE1IS` per page, `DSB ISH`, and `ISB`. Retirement first proves that
the mapping token names the same root, layout, and frames and that every live descriptor is exact;
only then does it invalidate the batch and complete the same maintenance before returning the
unpublished owner. It never performs valid-to-valid replacement, so break-before-make is not part
of this restricted transition.

`FinalDirectMapMemory` implements stack clearing and the active-table descriptor backend. Both are
restricted to allocator-managed frames proved against the retained TTBR1 hierarchy. Constructing
the mapper remains unsafe and requires the single boot CPU, masked/non-mutating exceptions,
exclusive descriptor access, and the exact active table owner. No boot path allocates or publishes
an owned runtime stack yet.

## Invariants

- the guard is exactly one 4 KiB frame and immediately precedes the downward-growing stack;
- the guard frame belongs to the reserved kernel image and can never be returned by the boot
  allocator;
- the final offline translation model must resolve the guard virtual address to no mapping;
- the first and last usable stack frames remain kernel-only, writable, and non-executable;
- the stack and guard layout must fit inside the linker's first-2-MiB permission window;
- static artifact inspection rejects missing, misaligned, reordered, or wrongly sized symbols;
- no current code intentionally probes the guard until recoverable kernel-fault testing exists;
- each runtime slot has exactly one lower guard, one nonempty usable range, and one upper guard;
- a pool's slot layouts are page-aligned, disjoint, adjacent, and contained by its checked arena;
- the platform arena is canonical TTBR1 space with retained L2/L3 table topology but no valid
  leaf descriptor, and one runtime pool owner is retained in both final-map boot paths;
- a coarser L1/L2 leaf can never replace reserved child-table topology, while later L3 leaves may
  populate individual usable pages;
- an allocated owner uniquely retains every usable physical frame and its virtual slot generation;
- physical OOM, slot exhaustion, allocator failure, and injected clear failure leave the pool and
  caller's physical allocator unchanged;
- every usable frame is cleared before an owner can be returned;
- unpublished release either restores every frame and recycles the slot or changes neither owner;
- guards never appear in the owner's mapping iterator, while every usable page appears exactly
  once in increasing virtual order;
- publication failure returns the unchanged unpublished owner and leaves no partial mapping;
- safe code cannot call the low-level backend methods directly and bypass the consuming ownership
  transitions;
- a mapped owner retains its frames and slot until the exact backend token is successfully retired;
- unpublication failure returns a still-complete mapped owner and cannot expose physical release;
- the AArch64 mapper accepts only zero invalid leaves for publication and only the exact owned
  descriptors for retirement; all recoverable errors occur before its first write;
- a successful leaf batch completes store visibility, all-ASID per-page invalidation, and final
  barriers before mapping use or physical-frame release;
- current live updates are only invalid-to-valid publication and valid-to-invalid retirement;
  valid-to-valid replacement is outside this backend and would require break-before-make;
- mapped ownership cannot be sent or shared across Rust concurrency domains in the current
  single-core policy;
- current pool mutation requires one non-reentrant mutable owner. IRQ, preemption, and SMP sharing
  require an IRQ-safe lock plus documented acquire/release ordering before use.

## Validation

Host tests verify that direct-map overrides can leave an exact page hole without losing adjacent
permissions or large mappings. Platform tests verify that the QEMU `virt` final plan leaves the
declared guard absent while retaining text, rodata, data, and PL011 mappings. Every AArch64 kernel
variant is cross-linked and inspected for the two guard symbols, their exact size and adjacency,
the 1 MiB stack size, and the overall image bound.

Host tests additionally cover arena capacity, alignment and overflow, exact two-guard layouts,
deterministic slot assignment, slot exhaustion, physical OOM, full-frame clearing, injected clear
failure, transactional foreign-pool rejection, frame restoration, slot reuse, and both stack-role
labels. Release tests also prove that a wrong physical allocator retains the owner and changes
neither allocator nor slot. An integration test feeds owned page pairs to the AArch64 offline
planner and verifies EL1-only RW/NX translations plus absent guards. `cargo xtask ci` Cross
Compiles this mechanism in all AArch64 variants and retains the existing ELF inspections. Platform
tests verify the exact 640 KiB arena, prove every initial translation absent, and reject an injected
mapping at its first page. Planner tests reserve empty topology across an L3-table boundary, reject
misalignment, lower-half input, existing leaves, coarser-leaf replacement, and capacity exhaustion
transactionally, and materialize linked parent tables while proving the selected leaf stays
invalid. The pinned platform model now contains seven table frames and the same 767 leaves. A
fault-injecting mapping model verifies failed publication, successful type-state transition,
rejection by a foreign backend instance, failed unpublication with token retention, successful
retry, and frame/slot release only after returning to the unpublished state.

The concrete AArch64 mapper test materializes the prelinked hierarchy, injects a descriptor-read
failure before publication, verifies zero writes and zero maintenance, publishes exact RW/NX
leaves with guards unchanged, rejects a deliberately corrupted live descriptor without mutation,
injects a retirement preflight failure, then retries retirement. It proves the second maintenance
batch precedes recovery of the unpublished owner and only that state can release frames. The
target-specific barrier/TLBI implementation and final-direct-map descriptor backend Cross Compile;
neither has executed on QEMU.

The recorded final-map and loaded-init probes crossed final TTBR1 publication while using the
1 MiB construction stack. This validates the positive live-stack mapping, but not an intentional
underflow into the guard.

This does not prove that an AArch64 translation fault occurs on stack underflow. That evidence is
tracked as `RUN-KSTACK-001` in `docs/RUNTIME_VALIDATION.md`.

It also does not prove that an owned runtime stack can be safely published, entered, switched, or
retired. That separate target boundary is `RUN-KSTACK-002` and remains Runtime Pending.

## Runtime status

`RUN-KSTACK-001` remains Runtime Pending for an intentional fault on the existing boot guard.
`RUN-KSTACK-002` remains Runtime Pending for owned-stack publication, entry, and retirement. The
recorded positive final-map and loaded-init runs validate only the linker-owned construction stack
and do not close either negative or dynamic-stack boundary. Those runs also predate the current
seven-table hierarchy; the empty arena topology is Host Tested, Cross Compiled, and Runtime
Pending rather than inferred from the older five-table output. The live leaf mapper is likewise
Host Tested and Cross Compiled but has not been instantiated by a boot path.

## TODO

- execute a controlled guard access after final TTBR1 publication and confirm the expected EL1
  data abort without relying on a corrupted stack;
- automate a conservative early-stack footprint upper bound for supported build profiles;
- move bulky boot plans into explicitly owned scratch memory so the temporary stack can shrink;
- replace the provisional 16-slot/eight-page policy with measured thread and exception call-depth
  requirements before relying on it as production sizing;
- integrate the AArch64 mapper with one boot-owned stack allocation, keep exceptions unable to
  mutate the hierarchy, and validate publication/retirement on target before switching SP;
- define break-before-make separately before allowing any valid-to-valid descriptor replacement;
- define separate exception and emergency-stack sizing, nesting, and double-fault policy before
  asynchronous interrupts or recoverable current-EL faults;
- switch stacks safely during thread creation, context switching, teardown, and CPU-local idle;
- make overflow diagnostics identify the owning slot without touching guard memory;
- add stack canaries or high-water accounting only after their failure and concurrency semantics
  are specified.

## Skipped work

Phoenix reserves and owns the virtual pool metadata and implements a restricted AArch64 leaf
mapper, but does not yet allocate runtime-stack frames during boot, instantiate that mapper against
the active hierarchy, switch between kernel threads, recover from kernel stack faults, enter a
dedicated exception stack, maintain per-CPU emergency stacks, unwind stack traces, randomize stack
locations, or reclaim a mapping on target. The Host Tested mapping evidence must not be described
as a running kernel-stack subsystem.
