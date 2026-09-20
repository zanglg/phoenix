# Monotonic Time and Timer Queue

Phoenix currently provides an architecture-neutral, allocation-free foundation for monotonic
time and deadline ordering plus a deliberately unintegrated AArch64 physical generic-timer
register owner. A Host Tested GIC type state can activate and classify the timer PPI behind an
unsafe dispatcher-publication proof, but the final-map boot paths retain delivery closed and do not
construct or program the timer owner, deliver timer interrupts, block a thread, or run callbacks.

## Current implementation

`kernel/src/time.rs` defines:

- `TickInstant` and `TickDuration` as exact, unscaled `u64` ticks in one externally selected
  fixed-frequency clock domain;
- `MonotonicClock`, which retains the last accepted observation and rejects a lower value;
- `TimerQueue<CAPACITY>`, a fixed-metadata queue of absolute deadlines and opaque `u64` events;
- generation-checked `TimerId` values for cancellation and `FiredTimer` values returned when a
  deadline is removed.

`kernel/src/arch/aarch64/timer.rs` defines:

- a checked nonzero 32-bit `CNTFRQ_EL0` value and nanosecond-to-tick conversion that rounds up;
- decoded `CNTP_CTL_EL0` enable, interrupt-mask, and condition bits; and
- a target-only exclusive physical-timer owner for ordered `CNTFRQ_EL0`, `CNTPCT_EL0`,
  `CNTP_CVAL_EL0`, and `CNTP_CTL_EL0` access.

The QEMU `virt` platform layer also strictly discovers all four architected-timer PPIs from the
DTB and selects the second, non-secure physical entry for the `CNTP_*` contract. That descriptive
topology now feeds the boot GIC preparation transaction. The GIC layer can classify an acknowledged
INTID against that configured PPI, but is not yet connected to the physical-timer owner or a real
interrupt dispatcher.

The target owner programs an absolute compare value only while signaling is disabled and masked,
then leaves the timer enabled but masked. Unmasking is an unsafe operation whose caller must first
establish exception, GIC routing, timer ownership, and IRQ-safe shared-state invariants. Phoenix
does not currently construct this owner from a boot path or unmask it.

The queue performs no allocation and invokes no callback. Scheduling scans for the lowest reusable
slot. Removing a due timer chooses the earliest deadline and then the lowest slot, so equal-deadline
behavior is deterministic. A caller is expected to remove due records while holding the queue's
future synchronization and dispatch the returned event only after releasing that synchronization.

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

No unsafe code or architecture-specific state is present in this module.

## Synchronization boundary

The current API requires one exclusive mutable owner. This is sufficient for host tests and the
single-threaded pre-interrupt construction path, but it is not yet wrapped in an IRQ-safe
synchronization policy. Phoenix now has a Host Tested IRQ-save spinlock; before an interrupt
handler and normal kernel context share a queue, their owning wrapper must use it and:

- prevents same-core interrupt re-entry while queue metadata is borrowed;
- defines acquire/release ordering for producers, the interrupt handler, and future SMP users;
- never invokes an event callback while holding the queue protection; and
- coordinates timer reprogramming with changes to the earliest deadline.

## Validation

Host tests cover equal/increasing/regressing observations, arithmetic overflow, deadline ordering,
equal-deadline tie breaking, cancellation, stale-generation rejection, slot reuse and retirement,
past deadlines, zero and full capacity, and failure atomicity. They also cover invalid frequency,
round-up boundaries, conversion/deadline overflow, and control-bit decoding. Canonical CI
cross-compiles the register access as part of every AArch64 kernel variant; this only proves
compilation and static artifact contracts, not target execution.

## TODO

- Publish the interrupt dispatcher and its synchronization state, then consume the existing GIC
  activation proof without weakening its masked-transition contract.
- Connect the exclusive physical-timer owner to boot after GIC and IRQ state are ready.
- Wrap shared timer state in the existing IRQ-save lock and publish its place in the lock order.
- Connect due events to sleeping threads and tickless scheduler wakeups.
- Decide whether a queue-instance namespace is worth the identifier and initialization cost.
- Add a bounded target probe for counter monotonicity, compare programming, interrupt delivery,
  equal-deadline ordering, and continued progress after the interrupt.

## Deliberately skipped

This checkpoint does not implement wall time, calendar time, periodic timers, callback storage,
dynamic queue growth, kernel-thread sleep, scheduler preemption, idle `WFI`, SMP, boot-time GIC
delivery activation, target IRQ acknowledgement, or boot-time timer activation. GIC discovery,
closed-gate PPI preparation, and the unintegrated Host Tested delivery protocol belong to the
separate interrupt module. This checkpoint does not
claim a target frequency or precision because `CNTFRQ_EL0` has not been read during target
execution.

## Runtime Pending

`RUN-TIME-001` in `docs/RUNTIME_VALIDATION.md` remains Runtime Pending. Host tests and AArch64
cross-compilation cannot establish that QEMU's counter advances monotonically, that the programmed
timer PPI is delivered through the GIC, or that interrupt-context ordering is correct.
