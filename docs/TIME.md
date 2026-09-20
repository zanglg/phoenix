# Monotonic Time and Timer Queue

Phoenix currently provides an architecture-neutral, allocation-free foundation for monotonic
time and deadline ordering plus an AArch64 physical generic-timer adapter and a Host Tested pinned
GIC/timer publication runtime. The final-map boot paths still retain delivery closed and do not
construct or program the timer owner, deliver timer interrupts, block a thread, or run callbacks.

## Current implementation

`kernel/src/time.rs` defines:

- `TickInstant` and `TickDuration` as exact, unscaled `u64` ticks in one externally selected
  fixed-frequency clock domain;
- `MonotonicClock`, which retains the last accepted observation and rejects a lower value;
- `TimerQueue<CAPACITY>`, a fixed-metadata queue of absolute deadlines and opaque `u64` events;
- generation-checked `TimerId` values for cancellation and `FiredTimer` values returned when a
  deadline is removed.

`kernel/src/timer_service.rs` now composes that queue with the IRQ-save lock:

- `TimerHardware` separates ordered counter reads from unsafe, IRQ-masked application of a complete
  `Arm(deadline)` or `Disarm` state;
- `LockedTimerQueue<CAPACITY, H>` owns one monotonic clock, queue, hardware backend, and operational
  flag under `IrqSpinLock`;
- command-capable operations require a non-transferable `TimerProgrammingPermit` created only by
  the active architecture runtime; initially disarmed construction no longer assumes GIC delivery;
- schedule and cancel reprogram only when the earliest deadline changes;
- interrupt handling removes every due record, re-arms the next deadline or disarms an empty queue,
  and returns a fixed-capacity batch only after its lock guard drops; and
- a programming failure restores the exact queue slot/generation checkpoint, attempts the prior
  hardware command, and permanently closes the controller if that recovery also fails; and
- `quiesce` refuses to strand queued events and explicitly disarms an empty service before its
  architecture permit is revoked.

`kernel/src/arch/aarch64/timer.rs` defines:

- a checked nonzero 32-bit `CNTFRQ_EL0` value and nanosecond-to-tick conversion that rounds up;
- decoded `CNTP_CTL_EL0` enable, interrupt-mask, and condition bits; and
- a target-only, non-transferable physical-timer owner for ordered `CNTFRQ_EL0`, `CNTPCT_EL0`,
  `CNTP_CVAL_EL0`, and `CNTP_CTL_EL0` access; and
- the concrete `TimerHardware` adapter that programs masked, then unmasks only behind a live
  runtime permit, or disables/masks for `Disarm`.

The QEMU `virt` platform layer also strictly discovers all four architected-timer PPIs from the
DTB and selects the second, non-secure physical entry for the `CNTP_*` contract. That descriptive
topology now feeds the boot GIC preparation transaction. The unintegrated pinned interrupt runtime
binds that exact configured PPI to a locked timer service, rejects mismatches before activation,
and models a complete acknowledge/reprogram/EOI cycle. Boot does not construct this owner or route
an IRQ vector to it.

The target owner programs an absolute compare value only while signaling is disabled and masked.
Its `TimerHardware` implementation then unmasks signaling only through the unsafe backend call;
the locked service requires the active runtime's private permit before reaching that call. Phoenix
does not currently construct this owner from a boot path or issue a real command.

The queue performs no allocation and invokes no callback. Scheduling scans for the lowest reusable
slot. Removing a due timer chooses the earliest deadline and then the lowest slot, so equal-deadline
behavior is deterministic. The locked controller enforces queue/hardware serialization and returns
due records only after releasing synchronization. The Host Tested interrupt runtime completes the
GIC acknowledgement after timer reprogramming and returns events only after both locks release, so
callbacks remain outside timer and GIC ownership.

## Invariants and failure semantics

- A timeline is non-wrapping. Tick addition that exceeds `u64::MAX` returns `Overflow`; a lower
  counter observation returns `Regression` without changing the last accepted value.
- Instants, durations, queue deadlines, and the caller-supplied `now` must belong to the same
  fixed-frequency domain. The raw types deliberately perform no unit conversion.
- `CNTFRQ_EL0` must be nonzero and fit its architectural 32-bit field. Nanosecond conversion uses
  an intermediate `u128`, rounds every positive fractional tick up, and reports tick-count or
  absolute-deadline overflow separately.
- A deadline equal to `now` is accepted and immediately due. A deadline below `now` returns
  `DeadlineInPast` without changing the queue.
- Capacity exhaustion, including a zero-capacity queue or slots permanently retired after
  generation exhaustion, returns `CapacityExhausted` without changing the queue.
- Cancelling a stale, already cancelled, or already fired generation returns `InvalidTimer`.
- A `TimerId` is scoped to the queue instance that created it. The current compact identifier does
  not contain a queue-instance namespace, so passing an ID to another queue is a caller contract
  violation and is not guaranteed to be detected.
- Reuse increments a nonzero generation. A slot whose generation reaches `u64::MAX` is retired
  after its final event instead of wrapping and allowing an old identity to alias a new timer.
- `pop_due` removes exactly one record before returning it. Event dispatch and any retry policy are
  owned by the caller; the queue cannot leave a callback half-executed.
- locked scheduling and cancellation update the comparator only when the earliest deadline changes;
  interrupt handling always reapplies the resulting command before return so a level-triggered
  source is cleared or deliberately left armed;
- a primary programming failure never publishes a tentative queue mutation. Successful recovery
  restores service; failed recovery makes every later operation return `Unavailable` without
  reading the counter or touching hardware;
- counter failure and monotonic regression change neither queue nor comparator, while queue
  capacity, stale-ID, and past-deadline errors occur before hardware programming; and
- the controller constructor is unsafe only for unique, initially disabled/masked hardware and a
  matching counter domain; each later command additionally requires the active runtime's private
  programming permit; and
- quiescence rejects a nonempty queue without hardware mutation and explicitly disarms an empty
  queue before interrupt delivery can close.

The raw time and queue module contains no unsafe code or architecture-specific state. The service
uses narrow unsafe calls only to apply a hardware command while its IRQ-save guard proves local
masking and unique backend ownership.

## Synchronization boundary

The raw queue still requires one exclusive mutable owner. `LockedTimerQueue` is the first owning
wrapper to use the Host Tested IRQ-save spinlock and it:

- prevents same-core interrupt re-entry while queue metadata and hardware are borrowed;
- uses acquire/release ordering for producers, the interrupt handler, and future SMP users;
- returns due data without invoking an event callback while protected; and
- coordinates comparator programming transactionally with every earliest-deadline change.

The Host Tested AArch64 interrupt runtime supplies the first nested production order: acquire its
outer GIC/publication lock before the inner timer lock. Its pinned slot is published while GIC
delivery is closed, and it creates the timer permit only after activation. The concrete register
adapter and state machine Cross Compile, but no static boot owner or vector lookup exists yet.

## Validation

Host tests cover equal/increasing/regressing observations, arithmetic overflow, deadline ordering,
equal-deadline tie breaking, cancellation, stale-generation rejection, slot reuse and retirement,
past deadlines, zero and full capacity, and failure atomicity. Service tests cover earliest-only
reprogramming, non-earliest cancellation, equal-deadline batch order, rearm/disarm, callbacks after
IRQ-state restoration, exact queue rollback, successful programming recovery, permanent closure
after failed recovery, queue failures that never program hardware, permit-gated activation, PPI
mismatch, pending-timer deactivation rejection, timer-before-EOI ordering, and terminal GIC
failures. They also cover invalid frequency, round-up boundaries, conversion/deadline overflow,
and control-bit decoding. Canonical CI cross-compiles the concrete adapter as part of every AArch64
kernel variant; this proves compilation and static artifact contracts, not target execution.

## TODO

- Install the pinned runtime plus a permanent dispatcher lookup in boot, then connect the exclusive
  physical-timer owner after the initial counter observation.
- Add the explicit final `DAIF.I` unmask/removal transition after every runtime invariant is live.
- Connect due events to sleeping threads and tickless scheduler wakeups.
- Decide whether a queue-instance namespace is worth the identifier and initialization cost.
- Add a bounded target probe for counter monotonicity, compare programming, interrupt delivery,
  equal-deadline ordering, and continued progress after the interrupt.

## Deliberately skipped

This checkpoint does not implement wall time, calendar time, periodic timers, callback storage,
dynamic queue growth, kernel-thread sleep, scheduler preemption, idle `WFI`, SMP, boot-time GIC
delivery activation, target IRQ acknowledgement, or
boot-time timer activation. GIC discovery, closed-gate PPI preparation, and the unintegrated Host
Tested delivery protocol belong to the separate interrupt module. This checkpoint does not
claim a target frequency or precision because `CNTFRQ_EL0` has not been read during target
execution.

## Runtime Pending

`RUN-TIME-001` in `docs/RUNTIME_VALIDATION.md` remains Runtime Pending. Host tests and AArch64
cross-compilation cannot establish that QEMU's counter advances monotonically, that the abstract
transaction matches real `CNTP_*` signaling, that the timer PPI is delivered through the GIC, or
that interrupt-context ordering is correct.
