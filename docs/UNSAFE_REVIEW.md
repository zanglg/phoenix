# Unsafe ownership and ordering review

This is a source review and test-supported invariant audit, not an independent
security assessment. It covers the AArch64/QEMU single-CPU configuration and the
new integration paths. Existing pure parser/allocator models remain subject to
canonical host tests. Every new unsafe block has an adjacent SAFETY explanation.

| Boundary | Retained invariant | Evidence |
| --- | --- | --- |
| Boot/BSS | Full linker BSS is inspected before singleton writes, with IRQs masked; x29 starts at zero | EL1 and EL2 state probes |
| Exception assembly | 288-byte aligned frame saves all GPRs/control fields before Rust; returned frame stays mapped; restoration follows borrow/lock end | 31-register/status/SP round trip and real thread preemption |
| Final TTBR1 | Private complete plan, permanent pinned table owner, barriers and exact MMIO mapping | Current final-map probe and RO/NX/guard faults |
| Heap alias | Six empty leaf slots preflighted before any writes; four backing frames retained permanently; TLB completion before access | Injected failure at every read, real alias read/write and both guards |
| Runtime stacks | Slot and frame ownership retained while SP/frame points into them; unpublish and TLB completion precede release | Two preempted workers, mapped guard probes, exact 16-frame restoration |
| IRQ runtime | Release publication precedes Acquire lookup/delivery; outer runtime before timer lock; callbacks after EOI/unlock; delivery closes before removal | Existing host failure injection plus timer/IRQ-lock probe |
| Singleton state | CPU 0 only, masked non-reentrant access, no borrowed reference across EL0 entry or context switch | Explicit accessor assertions and system/thread integration |
| User roots | All ELF, stack and table frames retained by process slot; switch reads back TTBR0; complete global invalidation before reap | Different programs at identical user VAs; normal and nine fault exits |
| ASID retirement | Old root no longer executable, translations invalidated, no stale owner; never-installed IDs can roll back | Host reuse beyond 255 allocations; 705 target child lifetimes and unused-ID exhaustion/recovery |
| User copies | Complete residency/permission proof; access through owned physical frames; no retained raw user pointer | Host page-boundary cases plus valid/invalid target cross-page reads/writes |
| Loader rollback | IRQ-masked private transaction; temporary owners die before restoring allocator snapshot; no publication on error | Target 0–9 frame budgets, all 21 injected backend operations, ASID exhaustion and exact free-set recovery |
| Diagnostics | Frame records checked inside current owned mapped stack before volatile reads; increasing chain and text bounds; maximum 16 | Corrupt/overflow/cycle host tests; real panic trace resolved offline |
| Semihosting | Only explicitly selected test boot arguments reach guest exit | Interactive system halts; new harnesses require exit status 0 and ordered proof |

The heap direct-map alias remains part of the kernel's privileged access model;
its backing owner is never released while the guarded allocation alias exists.
General callers must continue honoring unique frame ownership even when an address
is in the managed-RAM set. No new API grants EL0 direct access to these aliases.

Kernel thread SVC calls are lock-free scheduling boundaries. The worker probe has
no destructors/guards requiring cleanup when it exits. User syscalls remain masked
and synchronous, so VFS and process borrows end before the exception epilogue.
Read-commit tokens belong to their originating serialized file table and shared
filesystem owner; they are not cross-process capabilities.

Panic masks IRQs before reporting. Log snapshots use try-lock and release the log
before serial MMIO. Traces may truncate at assembly or corruption. There is no
emergency exception stack or stack-overflow recovery guarantee. Retained static
owners intentionally live until halt; exited child and worker resources are reaped.

SMP, nested interrupts, new scheduler contexts, DMA, FP/SIMD, dynamic mappings and
larger capacities require another invariant review and target validation.

## 0.2 cooperative scheduling extension

All user dispatch remains IRQ-masked and non-reentrant on CPU 0. The run queue
and process table publish/retire their identity binding together; every selection
checks the complete generation-checked binding. A running frame is copied into
its process owner before it becomes Ready or Waiting. No pointer into the shared
boot/exception stack and no suspended Rust continuation is retained by a process.
Selection installs/readbacks the next retained root and completes global TLBI,
then copies its owned frame onto the active exception stack. ERET occurs only
after dispatch returns and the unique State/frame borrows end.

Terminal children either retain all resources as an unschedulable zombie, or
have the parent's root installed before reap. Reap revokes process identity,
queue identity and their binding before reuse; exact allocator and ASID baselines
are asserted. Waiting results are written into the parent's owned frame before
waking it, and selection is a separate transition. Init exit ends the system;
remaining owners stay retained until halt. No orphan cleanup is claimed.

`test-scheduling` observes both completion/wait orderings, 257 child reaps and
128 faults, all preserved GPRs/NZCV, private stack contents and independent file
cursors. Timer preemption is a separate pending gate; this review does not yet
authorize EL0 IRQ unmasking.

The stress boot diagnostics run before first EL0 entry with IRQs masked. Withheld
frames and ASIDs remain private; failed image/table owners are destroyed before
allocator restoration, and diagnostic ASIDs are never installed in TTBR0. The
parent init root is retained throughout. No native syscall enables fault injection.
Successful retry and subsequent real child execution validate continued use of the
same allocator. Failure injection models backend operation errors, not arbitrary
hardware corruption or interrupted descriptor writes.
