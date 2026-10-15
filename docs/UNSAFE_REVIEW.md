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

## 0.2 EL0 timer extension

The separately observed preemption path enables I only in saved EL0 frames.
The exception vectors mask all DAIF bits before Rust; EL1 never unmasks them.
Thus State access remains CPU-0-only, masked and non-reentrant even when its
caller was interrupted in EL0. `dispatch_one` completes EOI and releases both
runtime/timer locks before the process frame is saved and another root selected.
No lock is held at ERET. Returned initial and saved frames preserve EL0t and its
IRQ policy; all roots are retained until retirement with completed global TLBI.
The periodic TimerId/event binding is checked on every delivery, and root exit
cancels/revokes delivery before halt or the explicit guest-driven test exit.

The non-yielding target loops check all 31 GPRs/NZCV/SP/private stack words across
real timer handoffs; the host also requires both contexts to receive multiple
ticks and the infinite child to remain Ready. A no-timer negative control proves
that printed success alone is insufficient. Kernel syscall preemption, SMP,
nested IRQs and FP/SIMD are still outside this invariant.

## 0.2 sleep/idle extension

Sleep validates the bounded duration and checked deadline before changing frame,
process or queue state. A success return is saved in the process-owned frame;
sleeping resources remain retained. Queue and lifecycle wake all deadlines due at
the same sampled counter instant before selecting a frame. No early wake is allowed.

If no user is Ready, dispatch overwrites only the active exception frame with an
EL1h frame targeting an eight-byte naked WFI/branch loop. That loop has no prologue,
stack allocation, destructor or borrow. The permanent construction/exception stack
remains owned and mapped. The last user root stays retained, with no idle access
to user memory. Every idle entry and IRQ checks the same SP top; idle IRQs must
come from CurrentSpX/EL1h with I clear and PC inside the exact two-instruction range.
Selection of a woken user installs its retained root and owned frame before ERET.
Thus repeated idle/user transitions neither accumulate suspended kernel frames nor
reuse a released stack. Rust borrows still end before every exception return.
At child completion, the parent must be Waiting, Sleeping or Ready. A Sleeping
parent keeps its deadline and is never implicitly woken by child exit; the child
remains terminal until that parent resumes and waits. Alternating sleep durations
observe both Waiting and Sleeping completion branches, eight times each.

The stress boot diagnostics run before first EL0 entry with IRQs masked. Withheld
frames and ASIDs remain private; failed image/table owners are destroyed before
allocator restoration, and diagnostic ASIDs are never installed in TTBR0. The
parent init root is retained throughout. No native syscall enables fault injection.
Successful retry and subsequent real child execution validate continued use of the
same allocator. Failure injection models backend operation errors, not arbitrary
hardware corruption or interrupted descriptor writes.
