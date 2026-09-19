# Kernel Stacks

This document describes the stack protection that Phoenix currently implements and the larger
kernel-stack policy that remains to be built. The linker-owned boot guard becomes effective only
after the final AArch64 `TTBR1_EL1` hierarchy is installed; the owned runtime-stack mechanism is
currently a Host Tested allocation and offline-mapping foundation.

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
does not choose an AArch64 address; the future platform integration must reserve a non-overlapping
TTBR1 arena and validate it against the complete kernel layout. Constructing two pools over the
same virtual arena is a caller error until that platform reservation owner exists.

Allocation clones the physical allocator, acquires all usable frames, then clears every frame
through `KernelStackMemory`. Only complete success commits the allocator and marks the virtual
slot allocated. The resulting non-copyable `OwnedKernelStack` retains the slot generation,
intended `Thread` or `Exception` role, guarded layout, and frames in increasing virtual order.
Unpublished release validates the exact pool and generation and returns every frame as one
transaction before recycling the slot. Generation overflow retires a slot instead of aliasing an
old owner. Failed allocation can have cleared candidate frames, but it does not publish their
ownership and leaves the original physical allocator unchanged.

The owned stack can supply page-by-page pairs to an offline architecture page-table plan. Host
tests use the real AArch64 planner to map only usable pages with `kernel_data` attributes and prove
both guards remain absent. No live table mutation, stack-pointer switch, or exception entry uses
these owners yet.

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
- an allocated owner uniquely retains every usable physical frame and its virtual slot generation;
- physical OOM, slot exhaustion, allocator failure, and injected clear failure leave the pool and
  caller's physical allocator unchanged;
- every usable frame is cleared before an owner can be returned;
- unpublished release either restores every frame and recycles the slot or changes neither owner;
- guards never appear in the owner's mapping iterator, while every usable page appears exactly
  once in increasing virtual order;
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
Compiles this mechanism in all AArch64 variants and retains the existing ELF inspections.

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
and do not close either negative or dynamic-stack boundary.

## TODO

- execute a controlled guard access after final TTBR1 publication and confirm the expected EL1
  data abort without relying on a corrupted stack;
- automate a conservative early-stack footprint upper bound for supported build profiles;
- move bulky boot plans into explicitly owned scratch memory so the temporary stack can shrink;
- choose the AArch64 TTBR1 runtime-stack arena, slot count, and usable page count from measured
  call depth and the complete virtual-address layout;
- add an ownership-preserving mapped-stack type that couples frames to installed descriptors;
- define break-before-make, TLB maintenance, and rollback for live stack map/unmap operations;
- define separate exception and emergency-stack sizing, nesting, and double-fault policy before
  asynchronous interrupts or recoverable current-EL faults;
- switch stacks safely during thread creation, context switching, teardown, and CPU-local idle;
- make overflow diagnostics identify the owning slot without touching guard memory;
- add stack canaries or high-water accounting only after their failure and concurrency semantics
  are specified.

## Skipped work

Phoenix does not yet publish owned runtime stacks into live tables, switch between kernel threads,
recover from kernel stack faults, enter a dedicated exception stack, maintain per-CPU emergency
stacks, unwind stack traces, randomize stack locations, or reclaim live stack mappings. The pool
and offline mapping evidence must not be described as a running kernel-stack subsystem.
