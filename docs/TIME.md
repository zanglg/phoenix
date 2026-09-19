# Monotonic Time and Timer Queue

Phoenix currently provides an architecture-neutral, allocation-free foundation for monotonic
time and deadline ordering. It does not yet read the AArch64 generic timer, initialize the GIC,
deliver timer interrupts, block a thread, or run callbacks.

## Current implementation

`kernel/src/time.rs` defines:

- `TickInstant` and `TickDuration` as exact, unscaled `u64` ticks in one externally selected
  fixed-frequency clock domain;
- `MonotonicClock`, which retains the last accepted observation and rejects a lower value;
- `TimerQueue<CAPACITY>`, a fixed-metadata queue of absolute deadlines and opaque `u64` events;
- generation-checked `TimerId` values for cancellation and `FiredTimer` values returned when a
  deadline is removed.

The queue performs no allocation and invokes no callback. Scheduling scans for the lowest reusable
slot. Removing a due timer chooses the earliest deadline and then the lowest slot, so equal-deadline
behavior is deterministic. A caller is expected to remove due records while holding the queue's
future synchronization and dispatch the returned event only after releasing that synchronization.

## Invariants and failure semantics

- A timeline is non-wrapping. Tick addition that exceeds `u64::MAX` returns `Overflow`; a lower
  counter observation returns `Regression` without changing the last accepted value.
- Instants, durations, queue deadlines, and the caller-supplied `now` must belong to the same
  fixed-frequency domain. The raw types deliberately perform no unit conversion.
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
single-threaded pre-interrupt construction path, but it is not an IRQ-safe synchronization policy.
Before an interrupt handler and normal kernel context share a queue, Phoenix must add a lock or
critical-section abstraction that:

- prevents same-core interrupt re-entry while queue metadata is borrowed;
- defines acquire/release ordering for producers, the interrupt handler, and future SMP users;
- never invokes an event callback while holding the queue protection; and
- coordinates timer reprogramming with changes to the earliest deadline.

## Validation

Host tests cover equal/increasing/regressing observations, arithmetic overflow, deadline ordering,
equal-deadline tie breaking, cancellation, stale-generation rejection, slot reuse and retirement,
past deadlines, zero and full capacity, and failure atomicity. Canonical CI also cross-compiles the
module as part of every AArch64 kernel variant; this only proves compilation and static artifact
contracts, not target execution.

## TODO

- Add an AArch64 generic-timer backend for counter frequency, counter reads, compare programming,
  enable/mask state, and acknowledged interrupt delivery.
- Define checked tick/unit conversion and rounding once the counter frequency is observed.
- Add GIC discovery and timer-PPI routing for the pinned QEMU `virt` board.
- Introduce the IRQ-safe synchronization and memory-ordering contract described above.
- Connect due events to sleeping threads and tickless scheduler wakeups.
- Decide whether a queue-instance namespace is worth the identifier and initialization cost.
- Add a bounded target probe for counter monotonicity, compare programming, interrupt delivery,
  equal-deadline ordering, and continued progress after the interrupt.

## Deliberately skipped

This checkpoint does not implement wall time, calendar time, periodic timers, callback storage,
dynamic queue growth, kernel-thread sleep, scheduler preemption, idle `WFI`, SMP, GIC access, or
generic-timer system-register access. It also does not claim a precision or duration unit because
no target counter frequency has been read.

## Runtime Pending

`RUN-TIME-001` in `docs/RUNTIME_VALIDATION.md` remains Runtime Pending. Host tests and AArch64
cross-compilation cannot establish that QEMU's counter advances monotonically, that the programmed
timer PPI is delivered through the GIC, or that interrupt-context ordering is correct.
