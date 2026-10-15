//! IRQ-safe transactional ownership around a fixed-capacity timer queue.

use crate::sync::{IrqSpinLock, LocalIrqControl};
use crate::time::{
    FiredTimer, MonotonicClock, MonotonicTimeError, TickInstant, TimerEvent, TimerId, TimerQueue,
    TimerQueueCheckpoint, TimerQueueError,
};
use core::marker::PhantomData;

/// Complete hardware programming state requested by the timer controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerProgramming {
    /// Enable interrupt signaling for one absolute deadline.
    Arm(TickInstant),
    /// Disable and mask timer interrupt signaling.
    Disarm,
}

/// Hardware operations required by [`LockedTimerQueue`].
pub trait TimerHardware {
    /// Backend-specific register or device failure.
    type Error;

    /// Read one ordered counter observation without changing timer state.
    fn counter(&mut self) -> Result<TickInstant, Self::Error>;

    /// Apply one complete comparator/signaling state.
    ///
    /// On success the requested state must be fully visible. On failure the
    /// hardware may be partially programmed; the caller will attempt a second
    /// explicit recovery command.
    ///
    /// # Safety
    ///
    /// Ordinary local IRQ delivery must be masked, the caller must uniquely
    /// own this backend, and any architecture-specific vector, interrupt-
    /// controller, and handler-publication invariants required before arming
    /// must already hold.
    unsafe fn program(&mut self, programming: TimerProgramming) -> Result<(), Self::Error>;
}

/// Non-transferable proof that timer interrupt programming is currently safe.
///
/// The architecture interrupt runtime creates this proof only after its
/// dispatcher is published and the matching interrupt-controller route is
/// active. Dropping the proof revokes permission to issue later programming
/// commands; it does not itself touch hardware.
pub struct TimerProgrammingPermit {
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl TimerProgrammingPermit {
    /// Create a current-CPU timer-programming proof.
    ///
    /// # Safety
    ///
    /// Every vector, interrupt-controller, handler-publication, and
    /// synchronization invariant required by the concrete [`TimerHardware`]
    /// backend must already hold and must remain valid until this value is
    /// dropped. The caller must create no competing permit for the same timer
    /// owner or CPU.
    pub(crate) const unsafe fn assume_ready() -> Self {
        Self {
            _not_send_or_sync: PhantomData,
        }
    }

    /// Consume the proof before its interrupt-controller route is closed.
    pub(crate) const fn revoke(self) {}
}

/// Failure while operating an IRQ-safe timer controller.
#[derive(Debug, Eq, PartialEq)]
pub enum TimerServiceError<E> {
    /// A previous programming failure and failed recovery left hardware state
    /// indeterminate. No later queue operation is permitted.
    Unavailable,
    /// Reading the counter failed without changing queue state.
    Counter(E),
    /// The hardware counter regressed; queue and programming state are kept.
    Monotonic(MonotonicTimeError),
    /// The queue rejected the requested operation before hardware changed.
    Queue(TimerQueueError),
    /// Deactivation was requested while timers remain queued.
    PendingTimers,
    /// Hardware programming failed after a tentative queue change.
    Programming {
        /// New state that failed.
        requested: TimerProgramming,
        /// Backend-specific primary failure.
        error: E,
        /// Prior state requested during recovery.
        recovery: TimerProgramming,
        /// Recovery failure. Its presence makes the controller unavailable.
        recovery_error: Option<E>,
    },
}

/// Fixed-capacity due-event batch removed by one timer interrupt.
#[derive(Debug, Eq, PartialEq)]
pub struct FiredTimerBatch<const CAPACITY: usize> {
    entries: [Option<FiredTimer>; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> FiredTimerBatch<CAPACITY> {
    fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, timer: FiredTimer) {
        debug_assert!(self.len < CAPACITY);
        self.entries[self.len] = Some(timer);
        self.len += 1;
    }

    /// Return the number of due events in this interrupt batch.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Return whether the interrupt found no due event.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate over due records in deadline-then-slot order.
    pub fn iter(&self) -> impl Iterator<Item = FiredTimer> + '_ {
        self.entries[..self.len].iter().copied().flatten()
    }
}

/// Observable state of a one-shot timer-event consumer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OneShotTimerConsumerState {
    /// No scheduled timer identity has been bound yet.
    Unbound,
    /// One exact scheduled identity and event value are expected.
    Bound {
        /// Generation-checked timer identity returned by the queue.
        id: TimerId,
        /// Opaque event value supplied with that timer.
        event: TimerEvent,
    },
    /// The exact bound timer was consumed once.
    Fired {
        /// Complete due record retained as completion evidence.
        timer: FiredTimer,
    },
}

/// Failure while binding or consuming a one-shot timer event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OneShotTimerConsumerError {
    /// The requested transition is invalid from the current state.
    State {
        /// Exact state that rejected the transition.
        actual: OneShotTimerConsumerState,
    },
    /// A one-shot consumer requires exactly one due record.
    BatchLength {
        /// Number of records supplied by the dispatcher.
        actual: usize,
    },
    /// The due record belongs to another slot generation.
    TimerMismatch {
        /// Identity bound before interrupt delivery was enabled.
        expected: TimerId,
        /// Identity returned by the timer queue.
        observed: TimerId,
    },
    /// The due record carries a different caller-defined event value.
    EventMismatch {
        /// Event value bound before interrupt delivery was enabled.
        expected: TimerEvent,
        /// Event value returned by the timer queue.
        observed: TimerEvent,
    },
}

/// IRQ-safe one-shot sink for an exact scheduled timer generation.
///
/// Binding is one-way and consumption accepts exactly one due record whose
/// generation-checked identity and opaque event value both match. Validation
/// failures leave the bound state unchanged, and successful consumption keeps
/// the complete fired record as permanent completion evidence. The caller is
/// responsible for invoking [`Self::consume`] only after the architecture
/// dispatcher has completed the interrupt-controller acknowledgement.
pub struct OneShotTimerConsumer {
    state: IrqSpinLock<OneShotTimerConsumerState>,
}

impl OneShotTimerConsumer {
    /// Create an unbound consumer without allocation.
    pub const fn new() -> Self {
        Self {
            state: IrqSpinLock::new(OneShotTimerConsumerState::Unbound),
        }
    }

    /// Bind the exact timer identity and event expected from one future IRQ.
    ///
    /// Callers must keep local IRQ delivery masked from successful scheduling
    /// through this binding so the timer cannot fire into an unbound consumer.
    pub fn bind<C: LocalIrqControl>(
        &self,
        irq: &C,
        id: TimerId,
        event: TimerEvent,
    ) -> Result<(), OneShotTimerConsumerError> {
        let mut state = self.state.lock(irq);
        if *state != OneShotTimerConsumerState::Unbound {
            return Err(OneShotTimerConsumerError::State { actual: *state });
        }
        *state = OneShotTimerConsumerState::Bound { id, event };
        Ok(())
    }

    /// Consume one exact due batch and retain its completion evidence.
    pub fn consume<C: LocalIrqControl, const CAPACITY: usize>(
        &self,
        irq: &C,
        batch: &FiredTimerBatch<CAPACITY>,
    ) -> Result<FiredTimer, OneShotTimerConsumerError> {
        if batch.len() != 1 {
            return Err(OneShotTimerConsumerError::BatchLength {
                actual: batch.len(),
            });
        }
        let observed = batch
            .iter()
            .next()
            .expect("a batch of length one must contain one timer");
        let mut state = self.state.lock(irq);
        let OneShotTimerConsumerState::Bound {
            id: expected_id,
            event: expected_event,
        } = *state
        else {
            return Err(OneShotTimerConsumerError::State { actual: *state });
        };
        if observed.id() != expected_id {
            return Err(OneShotTimerConsumerError::TimerMismatch {
                expected: expected_id,
                observed: observed.id(),
            });
        }
        if observed.event() != expected_event {
            return Err(OneShotTimerConsumerError::EventMismatch {
                expected: expected_event,
                observed: observed.event(),
            });
        }
        *state = OneShotTimerConsumerState::Fired { timer: observed };
        Ok(observed)
    }

    /// Return a stable snapshot while preserving the exact IRQ mask state.
    pub fn state<C: LocalIrqControl>(&self, irq: &C) -> OneShotTimerConsumerState {
        *self.state.lock(irq)
    }
}

impl Default for OneShotTimerConsumer {
    fn default() -> Self {
        Self::new()
    }
}

struct TimerState<const CAPACITY: usize, H> {
    clock: MonotonicClock,
    queue: TimerQueue<CAPACITY>,
    hardware: H,
    operational: bool,
}

/// Timer queue, monotonic clock, and comparator serialized by one IRQ-save lock.
///
/// Queue mutations and comparator updates form one transaction. A programming
/// failure restores the exact queue generation/slot checkpoint and attempts to
/// reapply the prior comparator state. Failed recovery permanently closes the
/// controller instead of allowing queue and hardware state to diverge.
pub struct LockedTimerQueue<const CAPACITY: usize, H> {
    state: IrqSpinLock<TimerState<CAPACITY, H>>,
}

impl<const CAPACITY: usize, H: TimerHardware> LockedTimerQueue<CAPACITY, H> {
    /// Construct a controller around disabled hardware and one initial counter
    /// observation.
    ///
    /// # Safety
    ///
    /// `hardware` must be uniquely owned with interrupt signaling disabled and
    /// masked. `initial` must be an observation from that hardware's fixed
    /// counter domain. Construction does not authorize hardware programming;
    /// every operation that can program the backend separately requires a
    /// live [`TimerProgrammingPermit`].
    pub const unsafe fn assume_disarmed(initial: TickInstant, hardware: H) -> Self {
        Self {
            state: IrqSpinLock::new(TimerState {
                clock: MonotonicClock::new(initial),
                queue: TimerQueue::new(),
                hardware,
                operational: true,
            }),
        }
    }

    /// Schedule one event against a fresh hardware-counter observation.
    pub fn schedule<C: LocalIrqControl>(
        &self,
        irq: &C,
        permit: &TimerProgrammingPermit,
        deadline: TickInstant,
        event: TimerEvent,
    ) -> Result<TimerId, TimerServiceError<H::Error>> {
        let mut state = self.state.lock(irq);
        ensure_operational(&state)?;
        let observed = state
            .hardware
            .counter()
            .map_err(TimerServiceError::Counter)?;
        let now = state
            .clock
            .observe(observed)
            .map_err(TimerServiceError::Monotonic)?;
        let checkpoint = state.queue.checkpoint();
        let recovery = desired_programming(&state.queue);
        let id = state
            .queue
            .schedule(now, deadline, event)
            .map_err(TimerServiceError::Queue)?;
        let requested = desired_programming(&state.queue);
        if requested != recovery {
            program_transaction(&mut state, permit, checkpoint, requested, recovery)?;
        }
        Ok(id)
    }

    /// Cancel one exact timer generation and update the comparator when needed.
    pub fn cancel<C: LocalIrqControl>(
        &self,
        irq: &C,
        permit: &TimerProgrammingPermit,
        id: TimerId,
    ) -> Result<TimerEvent, TimerServiceError<H::Error>> {
        let mut state = self.state.lock(irq);
        ensure_operational(&state)?;
        let checkpoint = state.queue.checkpoint();
        let recovery = desired_programming(&state.queue);
        let event = state.queue.cancel(id).map_err(TimerServiceError::Queue)?;
        let requested = desired_programming(&state.queue);
        if requested != recovery {
            program_transaction(&mut state, permit, checkpoint, requested, recovery)?;
        }
        Ok(event)
    }

    /// Remove every currently due timer and reprogram before returning.
    ///
    /// The IRQ-save guard is dropped before the batch becomes available to the
    /// caller. A real dispatcher must next complete its GIC acknowledgement,
    /// then invoke event callbacks outside both the timer lock and GIC active
    /// state.
    pub fn handle_interrupt<C: LocalIrqControl>(
        &self,
        irq: &C,
        permit: &TimerProgrammingPermit,
    ) -> Result<FiredTimerBatch<CAPACITY>, TimerServiceError<H::Error>> {
        let mut state = self.state.lock(irq);
        ensure_operational(&state)?;
        let observed = state
            .hardware
            .counter()
            .map_err(TimerServiceError::Counter)?;
        let now = state
            .clock
            .observe(observed)
            .map_err(TimerServiceError::Monotonic)?;
        let checkpoint = state.queue.checkpoint();
        let recovery = desired_programming(&state.queue);
        let mut batch = FiredTimerBatch::new();
        while let Some(timer) = state.queue.pop_due(now) {
            batch.push(timer);
        }
        let requested = desired_programming(&state.queue);
        program_transaction(&mut state, permit, checkpoint, requested, recovery)?;
        Ok(batch)
    }

    /// Disable timer signaling before the interrupt-controller route closes.
    ///
    /// A nonempty queue is rejected without touching hardware because closing
    /// delivery would strand owned wakeups. Successful quiescence keeps the
    /// empty queue and controller reusable for a later activation.
    pub fn quiesce<C: LocalIrqControl>(
        &self,
        irq: &C,
        permit: &TimerProgrammingPermit,
    ) -> Result<(), TimerServiceError<H::Error>> {
        let mut state = self.state.lock(irq);
        ensure_operational(&state)?;
        if !state.queue.is_empty() {
            return Err(TimerServiceError::PendingTimers);
        }
        let checkpoint = state.queue.checkpoint();
        program_transaction(
            &mut state,
            permit,
            checkpoint,
            TimerProgramming::Disarm,
            TimerProgramming::Disarm,
        )
    }

    /// Consume the controller and recover all state for diagnostics or a later
    /// ownership transition.
    pub fn into_parts(self) -> (MonotonicClock, TimerQueue<CAPACITY>, H, bool) {
        let state = self.state.into_inner();
        (state.clock, state.queue, state.hardware, state.operational)
    }
}

fn ensure_operational<const CAPACITY: usize, H: TimerHardware>(
    state: &TimerState<CAPACITY, H>,
) -> Result<(), TimerServiceError<H::Error>> {
    if state.operational {
        Ok(())
    } else {
        Err(TimerServiceError::Unavailable)
    }
}

fn desired_programming<const CAPACITY: usize>(queue: &TimerQueue<CAPACITY>) -> TimerProgramming {
    match queue.next_deadline() {
        Some(deadline) => TimerProgramming::Arm(deadline),
        None => TimerProgramming::Disarm,
    }
}

fn program_transaction<const CAPACITY: usize, H: TimerHardware>(
    state: &mut TimerState<CAPACITY, H>,
    _permit: &TimerProgrammingPermit,
    checkpoint: TimerQueueCheckpoint<CAPACITY>,
    requested: TimerProgramming,
    recovery: TimerProgramming,
) -> Result<(), TimerServiceError<H::Error>> {
    // SAFETY: the containing `IrqSpinLock` guard is live, so ordinary local
    // IRQ delivery is masked and `state` uniquely owns the hardware backend.
    // The live programming permit proves all backend-specific publication and
    // interrupt-controller invariants remain valid for this command.
    let result = unsafe { state.hardware.program(requested) };
    let Err(error) = result else {
        return Ok(());
    };

    state.queue.restore(checkpoint);
    // SAFETY: the same live guard, unique backend ownership, and construction-
    // time programming permit apply to the recovery command.
    let recovery_error = unsafe { state.hardware.program(recovery) }.err();
    if recovery_error.is_some() {
        state.operational = false;
    }
    Err(TimerServiceError::Programming {
        requested,
        error,
        recovery,
        recovery_error,
    })
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::cell::{Cell, RefCell};
    use std::vec;
    use std::vec::Vec;

    use crate::sync::LocalIrqControl;
    use crate::time::{TickInstant, TimerEvent, TimerQueueError};

    use super::{
        FiredTimerBatch, LockedTimerQueue, OneShotTimerConsumer, OneShotTimerConsumerError,
        OneShotTimerConsumerState, TimerHardware, TimerProgramming, TimerProgrammingPermit,
        TimerServiceError,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Mask(bool),
        Counter(TickInstant),
        Program(TimerProgramming),
        Restore(bool),
        Callback(TimerEvent),
    }

    struct MockIrq<'a> {
        masked: Cell<bool>,
        operations: &'a RefCell<Vec<Operation>>,
    }

    // SAFETY: this single-threaded mock saves, masks, and restores its exact
    // Boolean state on the one host thread that owns it.
    unsafe impl LocalIrqControl for MockIrq<'_> {
        type State = bool;

        fn save_and_mask(&self) -> Self::State {
            let previous = self.masked.replace(true);
            self.operations.borrow_mut().push(Operation::Mask(previous));
            previous
        }

        fn restore(&self, state: Self::State) {
            self.masked.set(state);
            self.operations.borrow_mut().push(Operation::Restore(state));
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum MockError {
        Injected,
    }

    struct MockHardware<'a> {
        now: &'a Cell<TickInstant>,
        operations: &'a RefCell<Vec<Operation>>,
        programming_count: usize,
        fail_programming: Vec<usize>,
    }

    impl TimerHardware for MockHardware<'_> {
        type Error = MockError;

        fn counter(&mut self) -> Result<TickInstant, Self::Error> {
            let now = self.now.get();
            self.operations.borrow_mut().push(Operation::Counter(now));
            Ok(now)
        }

        unsafe fn program(&mut self, programming: TimerProgramming) -> Result<(), Self::Error> {
            self.operations
                .borrow_mut()
                .push(Operation::Program(programming));
            let count = self.programming_count;
            self.programming_count += 1;
            if self.fail_programming.contains(&count) {
                Err(MockError::Injected)
            } else {
                Ok(())
            }
        }
    }

    fn service<'a, const CAPACITY: usize>(
        now: &'a Cell<TickInstant>,
        operations: &'a RefCell<Vec<Operation>>,
        fail_programming: Vec<usize>,
    ) -> LockedTimerQueue<CAPACITY, MockHardware<'a>> {
        let hardware = MockHardware {
            now,
            operations,
            programming_count: 0,
            fail_programming,
        };
        // SAFETY: the mock starts in the documented disarmed state and every
        // programming call occurs under the test IRQ controller's mask.
        unsafe { LockedTimerQueue::assume_disarmed(now.get(), hardware) }
    }

    fn events<const CAPACITY: usize>(batch: &FiredTimerBatch<CAPACITY>) -> Vec<TimerEvent> {
        batch.iter().map(|timer| timer.event()).collect()
    }

    fn programming_permit() -> TimerProgrammingPermit {
        // SAFETY: each test models one published, active interrupt route for
        // its uniquely owned timer and retains the proof on this host thread.
        unsafe { TimerProgrammingPermit::assume_ready() }
    }

    #[test]
    fn one_shot_consumer_accepts_only_its_exact_bound_generation() {
        let operations = RefCell::new(Vec::new());
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: Cell::new(false),
            operations: &operations,
        };
        let service = service::<1>(&now, &operations, Vec::new());
        let permit = programming_permit();
        let event = TimerEvent::new(0x5048_4f45_4e49_5801);
        let id = service
            .schedule(&irq, &permit, TickInstant::from_raw(10), event)
            .unwrap();
        let consumer = OneShotTimerConsumer::new();

        assert_eq!(consumer.bind(&irq, id, event), Ok(()));
        assert_eq!(
            consumer.bind(&irq, id, event),
            Err(OneShotTimerConsumerError::State {
                actual: OneShotTimerConsumerState::Bound { id, event },
            })
        );
        now.set(TickInstant::from_raw(10));
        let batch = service.handle_interrupt(&irq, &permit).unwrap();
        let fired = consumer.consume(&irq, &batch).unwrap();
        assert_eq!(fired.id(), id);
        assert_eq!(fired.deadline(), TickInstant::from_raw(10));
        assert_eq!(fired.event(), event);
        assert_eq!(
            consumer.state(&irq),
            OneShotTimerConsumerState::Fired { timer: fired }
        );
        assert_eq!(
            consumer.consume(&irq, &batch),
            Err(OneShotTimerConsumerError::State {
                actual: OneShotTimerConsumerState::Fired { timer: fired },
            })
        );
    }

    #[test]
    fn one_shot_consumer_rejects_empty_or_multiple_batches_without_completion() {
        let operations = RefCell::new(Vec::new());
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: Cell::new(false),
            operations: &operations,
        };
        let service = service::<2>(&now, &operations, Vec::new());
        let permit = programming_permit();
        let event = TimerEvent::new(1);
        let id = service
            .schedule(&irq, &permit, TickInstant::from_raw(10), event)
            .unwrap();
        service
            .schedule(&irq, &permit, TickInstant::from_raw(10), TimerEvent::new(2))
            .unwrap();
        let consumer = OneShotTimerConsumer::new();
        consumer.bind(&irq, id, event).unwrap();

        let empty = service.handle_interrupt(&irq, &permit).unwrap();
        assert_eq!(
            consumer.consume(&irq, &empty),
            Err(OneShotTimerConsumerError::BatchLength { actual: 0 })
        );
        now.set(TickInstant::from_raw(10));
        let multiple = service.handle_interrupt(&irq, &permit).unwrap();
        assert_eq!(
            consumer.consume(&irq, &multiple),
            Err(OneShotTimerConsumerError::BatchLength { actual: 2 })
        );
        assert_eq!(
            consumer.state(&irq),
            OneShotTimerConsumerState::Bound { id, event }
        );
    }

    #[test]
    fn one_shot_consumer_rejects_stale_identity_and_wrong_event_transactionally() {
        let operations = RefCell::new(Vec::new());
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: Cell::new(false),
            operations: &operations,
        };
        let service = service::<1>(&now, &operations, Vec::new());
        let permit = programming_permit();
        let stale_event = TimerEvent::new(10);
        let stale_id = service
            .schedule(&irq, &permit, TickInstant::from_raw(5), stale_event)
            .unwrap();
        assert_eq!(service.cancel(&irq, &permit, stale_id), Ok(stale_event));
        let live_event = TimerEvent::new(20);
        let live_id = service
            .schedule(&irq, &permit, TickInstant::from_raw(10), live_event)
            .unwrap();
        let stale_consumer = OneShotTimerConsumer::new();
        stale_consumer.bind(&irq, stale_id, live_event).unwrap();
        now.set(TickInstant::from_raw(10));
        let batch = service.handle_interrupt(&irq, &permit).unwrap();
        assert_eq!(
            stale_consumer.consume(&irq, &batch),
            Err(OneShotTimerConsumerError::TimerMismatch {
                expected: stale_id,
                observed: live_id,
            })
        );
        assert_eq!(
            stale_consumer.state(&irq),
            OneShotTimerConsumerState::Bound {
                id: stale_id,
                event: live_event,
            }
        );

        let wrong_event = TimerEvent::new(21);
        let event_consumer = OneShotTimerConsumer::new();
        event_consumer.bind(&irq, live_id, wrong_event).unwrap();
        assert_eq!(
            event_consumer.consume(&irq, &batch),
            Err(OneShotTimerConsumerError::EventMismatch {
                expected: wrong_event,
                observed: live_event,
            })
        );
        assert_eq!(
            event_consumer.state(&irq),
            OneShotTimerConsumerState::Bound {
                id: live_id,
                event: wrong_event,
            }
        );
    }

    #[test]
    fn scheduling_and_cancellation_reprogram_only_when_the_earliest_changes() {
        let operations = RefCell::new(Vec::new());
        let now = Cell::new(TickInstant::from_raw(5));
        let irq = MockIrq {
            masked: Cell::new(false),
            operations: &operations,
        };
        let service = service::<4>(&now, &operations, Vec::new());
        let permit = programming_permit();

        let later = service
            .schedule(
                &irq,
                &permit,
                TickInstant::from_raw(20),
                TimerEvent::new(20),
            )
            .unwrap();
        let non_earliest = service
            .schedule(
                &irq,
                &permit,
                TickInstant::from_raw(30),
                TimerEvent::new(30),
            )
            .unwrap();
        let earliest = service
            .schedule(
                &irq,
                &permit,
                TickInstant::from_raw(10),
                TimerEvent::new(10),
            )
            .unwrap();
        assert_eq!(
            service.cancel(&irq, &permit, non_earliest),
            Ok(TimerEvent::new(30))
        );
        assert_eq!(
            service.cancel(&irq, &permit, earliest),
            Ok(TimerEvent::new(10))
        );
        assert_eq!(
            service.cancel(&irq, &permit, later),
            Ok(TimerEvent::new(20))
        );

        let programs: Vec<_> = operations
            .borrow()
            .iter()
            .filter_map(|operation| match operation {
                Operation::Program(programming) => Some(*programming),
                _ => None,
            })
            .collect();
        assert_eq!(
            programs,
            vec![
                TimerProgramming::Arm(TickInstant::from_raw(20)),
                TimerProgramming::Arm(TickInstant::from_raw(10)),
                TimerProgramming::Arm(TickInstant::from_raw(20)),
                TimerProgramming::Disarm,
            ]
        );
        let (_, queue, _, operational) = service.into_parts();
        assert!(queue.is_empty());
        assert!(operational);
    }

    #[test]
    fn interrupt_rearms_before_unlock_and_callbacks_remain_external() {
        let operations = RefCell::new(Vec::new());
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: Cell::new(false),
            operations: &operations,
        };
        let service = service::<4>(&now, &operations, Vec::new());
        let permit = programming_permit();
        service
            .schedule(&irq, &permit, TickInstant::from_raw(10), TimerEvent::new(1))
            .unwrap();
        service
            .schedule(&irq, &permit, TickInstant::from_raw(10), TimerEvent::new(2))
            .unwrap();
        service
            .schedule(&irq, &permit, TickInstant::from_raw(20), TimerEvent::new(3))
            .unwrap();

        operations.borrow_mut().clear();
        now.set(TickInstant::from_raw(10));
        let batch = service.handle_interrupt(&irq, &permit).unwrap();
        assert_eq!(events(&batch), vec![TimerEvent::new(1), TimerEvent::new(2)]);
        for timer in batch.iter() {
            operations
                .borrow_mut()
                .push(Operation::Callback(timer.event()));
        }
        assert_eq!(
            *operations.borrow(),
            vec![
                Operation::Mask(false),
                Operation::Counter(TickInstant::from_raw(10)),
                Operation::Program(TimerProgramming::Arm(TickInstant::from_raw(20))),
                Operation::Restore(false),
                Operation::Callback(TimerEvent::new(1)),
                Operation::Callback(TimerEvent::new(2)),
            ]
        );

        operations.borrow_mut().clear();
        now.set(TickInstant::from_raw(20));
        let batch = service.handle_interrupt(&irq, &permit).unwrap();
        assert_eq!(events(&batch), vec![TimerEvent::new(3)]);
        assert!(
            operations
                .borrow()
                .contains(&Operation::Program(TimerProgramming::Disarm))
        );
    }

    #[test]
    fn programming_failure_restores_exact_queue_state_and_can_recover() {
        let operations = RefCell::new(Vec::new());
        let now = Cell::new(TickInstant::from_raw(5));
        let irq = MockIrq {
            masked: Cell::new(false),
            operations: &operations,
        };
        let service = service::<2>(&now, &operations, vec![0]);
        let permit = programming_permit();
        assert_eq!(
            service.schedule(&irq, &permit, TickInstant::from_raw(10), TimerEvent::new(1),),
            Err(TimerServiceError::Programming {
                requested: TimerProgramming::Arm(TickInstant::from_raw(10)),
                error: MockError::Injected,
                recovery: TimerProgramming::Disarm,
                recovery_error: None,
            })
        );
        let (_, queue, _, operational) = service.into_parts();
        assert!(queue.is_empty());
        assert!(operational);
    }

    #[test]
    fn failed_recovery_closes_controller_and_queue_errors_do_not_program() {
        let operations = RefCell::new(Vec::new());
        let now = Cell::new(TickInstant::from_raw(5));
        let irq = MockIrq {
            masked: Cell::new(false),
            operations: &operations,
        };
        let failed_service = service::<1>(&now, &operations, vec![0, 1]);
        let permit = programming_permit();
        assert!(matches!(
            failed_service.schedule(&irq, &permit, TickInstant::from_raw(10), TimerEvent::new(1),),
            Err(TimerServiceError::Programming {
                recovery_error: Some(MockError::Injected),
                ..
            })
        ));
        let operations_after_failure = operations.borrow().len();
        assert_eq!(
            failed_service.schedule(&irq, &permit, TickInstant::from_raw(11), TimerEvent::new(2),),
            Err(TimerServiceError::Unavailable)
        );
        assert_eq!(operations.borrow().len(), operations_after_failure + 2);
        let (_, queue, _, operational) = failed_service.into_parts();
        assert!(queue.is_empty());
        assert!(!operational);

        let clean_operations = RefCell::new(Vec::new());
        let clean = service::<0>(&now, &clean_operations, Vec::new());
        let clean_irq = MockIrq {
            masked: Cell::new(false),
            operations: &clean_operations,
        };
        assert_eq!(
            clean.schedule(
                &clean_irq,
                &permit,
                TickInstant::from_raw(10),
                TimerEvent::new(3),
            ),
            Err(TimerServiceError::Queue(TimerQueueError::CapacityExhausted))
        );
        assert!(
            !clean_operations
                .borrow()
                .iter()
                .any(|operation| matches!(operation, Operation::Program(_)))
        );
    }
}
