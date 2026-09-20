# IRQ-Safe Synchronization

This document defines Phoenix's first synchronization primitive: an architecture-neutral
IRQ-save spinlock with an AArch64 `DAIF.I` backend. The lock algorithm is Host Tested and the
AArch64 masking path is Cross Compiled. An architecture-neutral timer controller and a Host Tested
pinned AArch64 interrupt runtime now adopt it. Both final-map boot paths publish that owner and
lookup while IRQ delivery remains closed; activation, target lock behavior, and delivery are
Runtime Pending.

## Current implementation

`kernel/src/sync.rs` provides:

- the unsafe `LocalIrqControl` contract, whose implementation must save the current CPU's exact
  ordinary-IRQ state, mask delivery before returning, and restore that state on the same CPU;
- `IrqSpinLock<T>`, which stores one `AtomicBool` ownership word and one `UnsafeCell<T>` without
  allocation or poisoning;
- blocking `lock`, which masks local IRQs before beginning acquisition and spins until one
  compare-exchange succeeds;
- non-blocking `try_lock`, which restores the saved IRQ state immediately on contention; and
- `IrqSpinLockGuard`, which exposes the protected value and releases the ownership word before it
  restores the exact prior IRQ state.

The guard and its saved state are deliberately neither `Send` nor `Sync`. `get_mut` is available
only through an exclusive borrow of the complete lock, and `into_inner` consumes the lock; neither
bypasses synchronization through a shared reference.

`kernel/src/arch/aarch64/mod.rs` implements `LocalIrqControl` with `LocalIrq`. Acquisition reads
`DAIF`, executes `MSR DAIFSet, #2`, and then `ISB`. Restoration selects `MSR DAIFSet, #2` or
`MSR DAIFClr, #2` from the saved `DAIF.I` value and executes another `ISB`. The assembly omits
the `nomem` option so the compiler cannot move critical-section memory accesses across the local
masking transition. Debug, SError, and FIQ masks are not changed.

`kernel/src/timer_service.rs` is the first consumer. It keeps a monotonic clock, timer queue, and
abstract hardware owner under one lock, performs no callback while guarded, and transactionally
restores queue state if programming fails. `kernel/src/arch/aarch64/interrupt_runtime.rs` adds an
outer GIC/publication lock and publishes the first subsystem order: runtime lock before timer lock.
Activation, scheduling, dispatch, cancellation, and deactivation all follow that direction; no
timer-guarded path attempts to acquire the runtime lock.

## Invariants and memory ordering

- Local IRQ delivery is masked before the first lock acquisition attempt and remains masked until
  after the lock word has been released.
- A successful compare-exchange uses `Acquire`; guard drop stores the unlocked state with
  `Release`. Protected writes therefore happen before a later successful acquisition on another
  CPU. Relaxed loads are used only while waiting for an opportunity to retry the acquire operation.
- `try_lock` failure changes neither the protected value nor the lock word and restores the exact
  state observed before its attempt. An already-masked caller remains masked.
- Nested acquisition of different locks restores IRQ state in LIFO order. The inner guard cannot
  accidentally unmask an outer critical section.
- One lock is non-recursive. Attempting to reacquire it on the same CPU cannot make progress and
  violates the API contract.
- The guard and AArch64 mask token belong to the acquiring CPU and execution context. Future
  scheduler code must prohibit migration while either exists; the current kernel has no migration.
- Local IRQ masking prevents an ordinary same-CPU IRQ handler from interrupting normal context and
  deadlocking on the same lock. It does not mask FIQ, SError, debug exceptions, or synchronous
  faults, and those contexts must not access a protected value unless a separate policy permits it.
- Acquire/Release orders normal memory shared through this lock. It is not a device-MMIO barrier,
  cache-maintenance primitive, TLB shootdown, or substitute for the barriers required by GIC and
  page-table protocols.
- Safe shared access exists only through a live guard. `T: Send` is required before the lock itself
  can be moved or shared across Rust concurrency domains.
- Lock holders must not sleep, schedule, invoke arbitrary callbacks, or enter an allocator whose
  lock order is not explicitly below the current lock. The current nested order is runtime/GIC
  before timer; allocator, scheduler, console, and process locks remain outside that graph.

The unsafe architecture contract is necessary because a backend that reports masking without
actually preventing same-CPU IRQ re-entry could permit two mutable accesses to the same protected
value and violate Rust aliasing rules.

## Validation

Host tests verify mask-before-access ordering, mutable access, exact unmasked restoration, nested
masked-state preservation, LIFO restoration, contended `try_lock` rollback, and four host threads
performing 8,000 serialized updates through the Acquire/Release path. Formatting, checks, Clippy,
and all existing host tests run through canonical CI. Timer-controller tests additionally verify
that comparator operations occur while masked and returned callbacks occur after restoration.
Interrupt-runtime tests cover nested mask preservation, the runtime-before-timer order,
timer-before-EOI service, pending-timer deactivation rejection, and terminal controller failures.

Every AArch64 kernel variant Cross Compiles the real `MRS DAIF`, `MSR DAIFSet`, `MSR DAIFClr`, and
`ISB` backend. Existing ELF inspection continues to validate each kernel artifact, but static
evidence cannot prove target interrupt recognition, restoration timing, or absence of a lock-order
cycle in a future dispatcher.

## TODO

- Preserve the published runtime-before-timer order when boot activation and the first event
  consumer are added; keep console, allocator, and scheduler work outside both guards.
- Decide the kernel-heap and runtime-stack policies for normal-context versus interrupt-context
  access before wrapping either owner.
- Add preemption-count integration before a scheduler can migrate or switch a lock holder.
- Add lock ownership diagnostics, bounded debug-time recursion detection, and contention metrics
  without allocating in the lock path.
- Add a target probe that enters from an IRQ while normal context repeatedly acquires/releases the
  same lock and verifies continued progress plus exact `DAIF.I` restoration.

## Deliberately skipped

This checkpoint does not add a sleeping mutex, reader/writer lock, condition variable, seqlock,
RCU, priority inheritance, fairness guarantee, lock dependency graph, timeout, NMI/FIQ-safe lock,
per-CPU storage, scheduler preemption control, or SMP bring-up. It does not wrap the heap, frame
allocator, runtime-stack pool, process table, file table, or console. Boot now uses it for pinned
interrupt-runtime publication and lookup, but does not activate target Group-1 delivery, program a
timer deadline, or unmask boot IRQs.

## Runtime Pending

`RUN-SYNC-001` in `docs/RUNTIME_VALIDATION.md` is Runtime Pending. Host contention tests and
AArch64 cross-compilation do not show that a real target IRQ is excluded from a critical section,
that a previously unmasked `DAIF.I` is restored after unlock, or that future cross-subsystem lock
ordering is deadlock-free.
