//! Allocation-free monotonic time values and a deterministic timer queue.

/// One raw tick from a monotonic counter with a fixed, externally owned rate.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct TickInstant(u64);

impl TickInstant {
    /// Construct an instant from an unscaled hardware or test counter value.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the exact unscaled tick value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Add a duration without permitting counter wrap.
    pub const fn checked_add(self, duration: TickDuration) -> Result<Self, MonotonicTimeError> {
        match self.0.checked_add(duration.0) {
            Some(raw) => Ok(Self(raw)),
            None => Err(MonotonicTimeError::Overflow),
        }
    }

    /// Measure elapsed ticks, rejecting reversed observations.
    pub const fn duration_since(self, earlier: Self) -> Result<TickDuration, MonotonicTimeError> {
        match self.0.checked_sub(earlier.0) {
            Some(raw) => Ok(TickDuration(raw)),
            None => Err(MonotonicTimeError::Regression {
                previous: earlier,
                observed: self,
            }),
        }
    }
}

/// An exact number of ticks in the same fixed-rate domain as [`TickInstant`].
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct TickDuration(u64);

impl TickDuration {
    /// Construct a duration from an exact unscaled tick count.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the exact unscaled tick count.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Failure while extending or observing a monotonic tick timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonotonicTimeError {
    /// Adding a duration exceeded the supported non-wrapping timeline.
    Overflow,
    /// A newly observed counter value was lower than the prior observation.
    Regression {
        /// Last accepted value.
        previous: TickInstant,
        /// Rejected value.
        observed: TickInstant,
    },
}

/// Single-owner monotonic observation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonotonicClock {
    last: TickInstant,
}

impl MonotonicClock {
    /// Start a timeline at one already observed counter value.
    pub const fn new(initial: TickInstant) -> Self {
        Self { last: initial }
    }

    /// Return the latest accepted observation.
    pub const fn now(&self) -> TickInstant {
        self.last
    }

    /// Accept an equal or increasing counter value and reject regression.
    pub fn observe(&mut self, observed: TickInstant) -> Result<TickInstant, MonotonicTimeError> {
        if observed < self.last {
            return Err(MonotonicTimeError::Regression {
                previous: self.last,
                observed,
            });
        }
        self.last = observed;
        Ok(observed)
    }
}

/// Stable identity for one scheduled timer slot generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerId {
    slot: usize,
    generation: u64,
}

impl TimerId {
    /// Return the fixed queue slot index for diagnostics.
    pub const fn slot(self) -> usize {
        self.slot
    }

    /// Return the nonzero slot generation for diagnostics.
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

/// Opaque value delivered when a timer becomes due or is cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct TimerEvent(u64);

impl TimerEvent {
    /// Construct a caller-defined event value.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the caller-defined event value.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// One due timer removed from the queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FiredTimer {
    id: TimerId,
    deadline: TickInstant,
    event: TimerEvent,
}

impl FiredTimer {
    /// Return the identity that was valid while the timer was scheduled.
    pub const fn id(self) -> TimerId {
        self.id
    }

    /// Return the requested absolute deadline.
    pub const fn deadline(self) -> TickInstant {
        self.deadline
    }

    /// Return the caller-defined event value.
    pub const fn event(self) -> TimerEvent {
        self.event
    }
}

/// Failure while changing a fixed-capacity timer queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerQueueError {
    /// No reusable slot remains.
    CapacityExhausted,
    /// The requested deadline precedes the supplied current time.
    DeadlineInPast {
        /// Current monotonic time.
        now: TickInstant,
        /// Rejected deadline.
        deadline: TickInstant,
    },
    /// A timer identity is stale, already fired, already cancelled, or out of range.
    InvalidTimer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimerSlot {
    Free {
        generation: u64,
    },
    Scheduled {
        generation: u64,
        deadline: TickInstant,
        event: TimerEvent,
    },
    Retired,
}

/// Fixed-capacity absolute-deadline queue with generation-checked identities.
///
/// The queue invokes no callbacks. Callers repeatedly remove due events and
/// dispatch them after releasing whatever synchronization protects the queue.
/// Equal deadlines fire in ascending slot order for deterministic behavior.
/// A [`TimerId`] is valid only for the queue instance that created it; compact
/// IDs do not carry a queue-instance namespace.
#[derive(Debug, Eq, PartialEq)]
pub struct TimerQueue<const CAPACITY: usize> {
    slots: [TimerSlot; CAPACITY],
    scheduled: usize,
}

impl<const CAPACITY: usize> TimerQueue<CAPACITY> {
    /// Create an empty queue without dynamic allocation.
    pub const fn new() -> Self {
        Self {
            slots: [TimerSlot::Free { generation: 1 }; CAPACITY],
            scheduled: 0,
        }
    }

    /// Return the compile-time slot capacity, including retired slots.
    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    /// Return the number of currently scheduled timers.
    pub const fn len(&self) -> usize {
        self.scheduled
    }

    /// Return whether no timer is currently scheduled.
    pub const fn is_empty(&self) -> bool {
        self.scheduled == 0
    }

    /// Schedule one absolute deadline transactionally.
    pub fn schedule(
        &mut self,
        now: TickInstant,
        deadline: TickInstant,
        event: TimerEvent,
    ) -> Result<TimerId, TimerQueueError> {
        if deadline < now {
            return Err(TimerQueueError::DeadlineInPast { now, deadline });
        }
        let Some((slot, generation)) = self.slots.iter().enumerate().find_map(|(slot, state)| {
            if let TimerSlot::Free { generation } = state {
                Some((slot, *generation))
            } else {
                None
            }
        }) else {
            return Err(TimerQueueError::CapacityExhausted);
        };
        self.slots[slot] = TimerSlot::Scheduled {
            generation,
            deadline,
            event,
        };
        self.scheduled += 1;
        Ok(TimerId { slot, generation })
    }

    /// Cancel one exact scheduled generation and return its event.
    pub fn cancel(&mut self, id: TimerId) -> Result<TimerEvent, TimerQueueError> {
        let Some(TimerSlot::Scheduled {
            generation, event, ..
        }) = self.slots.get(id.slot).copied()
        else {
            return Err(TimerQueueError::InvalidTimer);
        };
        if generation != id.generation {
            return Err(TimerQueueError::InvalidTimer);
        }
        self.recycle(id.slot, generation);
        Ok(event)
    }

    /// Return the earliest scheduled absolute deadline.
    pub fn next_deadline(&self) -> Option<TickInstant> {
        self.slots
            .iter()
            .filter_map(|slot| match slot {
                TimerSlot::Scheduled { deadline, .. } => Some(*deadline),
                TimerSlot::Free { .. } | TimerSlot::Retired => None,
            })
            .min()
    }

    /// Remove the earliest due event, using slot order to break deadline ties.
    pub fn pop_due(&mut self, now: TickInstant) -> Option<FiredTimer> {
        let (slot, generation, deadline, event) = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, state)| match state {
                TimerSlot::Scheduled {
                    generation,
                    deadline,
                    event,
                } if *deadline <= now => Some((slot, *generation, *deadline, *event)),
                TimerSlot::Free { .. } | TimerSlot::Scheduled { .. } | TimerSlot::Retired => None,
            })
            .min_by_key(|(slot, _, deadline, _)| (*deadline, *slot))?;
        self.recycle(slot, generation);
        Some(FiredTimer {
            id: TimerId { slot, generation },
            deadline,
            event,
        })
    }

    fn recycle(&mut self, slot: usize, generation: u64) {
        self.slots[slot] = match generation.checked_add(1) {
            Some(generation) => TimerSlot::Free { generation },
            None => TimerSlot::Retired,
        };
        self.scheduled -= 1;
    }
}

impl<const CAPACITY: usize> Default for TimerQueue<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MonotonicClock, MonotonicTimeError, TickDuration, TickInstant, TimerEvent, TimerQueue,
        TimerQueueError, TimerSlot,
    };

    #[test]
    fn monotonic_clock_rejects_regression_and_arithmetic_wrap() {
        let start = TickInstant::from_raw(10);
        let mut clock = MonotonicClock::new(start);
        assert_eq!(clock.observe(TickInstant::from_raw(10)), Ok(start));
        assert_eq!(
            clock.observe(TickInstant::from_raw(12)),
            Ok(TickInstant::from_raw(12))
        );
        assert_eq!(
            clock.observe(TickInstant::from_raw(11)),
            Err(MonotonicTimeError::Regression {
                previous: TickInstant::from_raw(12),
                observed: TickInstant::from_raw(11),
            })
        );
        assert_eq!(clock.now(), TickInstant::from_raw(12));
        assert_eq!(
            TickInstant::from_raw(u64::MAX).checked_add(TickDuration::from_raw(1)),
            Err(MonotonicTimeError::Overflow)
        );
        assert_eq!(
            TickInstant::from_raw(15).duration_since(TickInstant::from_raw(12)),
            Ok(TickDuration::from_raw(3))
        );
    }

    #[test]
    fn schedule_cancel_and_reuse_are_generation_checked() {
        let mut queue = TimerQueue::<2>::new();
        let first = queue
            .schedule(
                TickInstant::from_raw(5),
                TickInstant::from_raw(8),
                TimerEvent::new(11),
            )
            .unwrap();
        assert_eq!(first.slot(), 0);
        assert_eq!(first.generation(), 1);
        assert_eq!(queue.cancel(first), Ok(TimerEvent::new(11)));
        assert_eq!(queue.cancel(first), Err(TimerQueueError::InvalidTimer));

        let reused = queue
            .schedule(
                TickInstant::from_raw(5),
                TickInstant::from_raw(9),
                TimerEvent::new(12),
            )
            .unwrap();
        assert_eq!(reused.slot(), first.slot());
        assert_eq!(reused.generation(), 2);
        assert_eq!(queue.cancel(first), Err(TimerQueueError::InvalidTimer));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn due_order_is_deadline_then_slot_and_callbacks_stay_external() {
        let mut queue = TimerQueue::<3>::new();
        let now = TickInstant::from_raw(100);
        queue
            .schedule(now, TickInstant::from_raw(110), TimerEvent::new(0))
            .unwrap();
        queue
            .schedule(now, TickInstant::from_raw(105), TimerEvent::new(1))
            .unwrap();
        queue
            .schedule(now, TickInstant::from_raw(105), TimerEvent::new(2))
            .unwrap();
        assert_eq!(queue.next_deadline(), Some(TickInstant::from_raw(105)));
        assert_eq!(queue.pop_due(TickInstant::from_raw(104)), None);

        let first = queue.pop_due(TickInstant::from_raw(105)).unwrap();
        let second = queue.pop_due(TickInstant::from_raw(105)).unwrap();
        assert_eq!(first.event(), TimerEvent::new(1));
        assert_eq!(second.event(), TimerEvent::new(2));
        assert_eq!(first.deadline(), TickInstant::from_raw(105));
        assert_eq!(first.id().slot(), 1);
        assert_eq!(second.id().slot(), 2);
        assert_eq!(queue.pop_due(TickInstant::from_raw(105)), None);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.next_deadline(), Some(TickInstant::from_raw(110)));
    }

    #[test]
    fn failures_are_explicit_and_leave_queue_unchanged() {
        let mut queue = TimerQueue::<1>::new();
        assert_eq!(queue.capacity(), 1);
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
        assert_eq!(
            queue.schedule(
                TickInstant::from_raw(10),
                TickInstant::from_raw(9),
                TimerEvent::new(1),
            ),
            Err(TimerQueueError::DeadlineInPast {
                now: TickInstant::from_raw(10),
                deadline: TickInstant::from_raw(9),
            })
        );
        assert!(queue.is_empty());
        queue
            .schedule(
                TickInstant::from_raw(10),
                TickInstant::from_raw(10),
                TimerEvent::new(2),
            )
            .unwrap();
        let before = TimerQueue::<1> {
            slots: queue.slots,
            scheduled: queue.scheduled,
        };
        assert_eq!(
            queue.schedule(
                TickInstant::from_raw(10),
                TickInstant::from_raw(11),
                TimerEvent::new(3),
            ),
            Err(TimerQueueError::CapacityExhausted)
        );
        assert_eq!(queue, before);
        assert_eq!(
            TimerQueue::<0>::new().schedule(
                TickInstant::from_raw(0),
                TickInstant::from_raw(0),
                TimerEvent::new(0),
            ),
            Err(TimerQueueError::CapacityExhausted)
        );
    }

    #[test]
    fn generation_overflow_retires_a_slot_instead_of_aliasing() {
        let mut queue = TimerQueue::<1> {
            slots: [TimerSlot::Free {
                generation: u64::MAX,
            }],
            scheduled: 0,
        };
        let id = queue
            .schedule(
                TickInstant::from_raw(0),
                TickInstant::from_raw(1),
                TimerEvent::new(7),
            )
            .unwrap();
        assert_eq!(id.generation(), u64::MAX);
        assert_eq!(queue.cancel(id), Ok(TimerEvent::new(7)));
        assert_eq!(queue.slots, [TimerSlot::Retired]);
        assert_eq!(
            queue.schedule(
                TickInstant::from_raw(0),
                TickInstant::from_raw(1),
                TimerEvent::new(8),
            ),
            Err(TimerQueueError::CapacityExhausted)
        );
    }
}
