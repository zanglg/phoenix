//! Fixed-capacity single-CPU round-robin policy, independent of saved CPU state.
//!
//! The owner serializes every transition with local IRQs masked. No switch may
//! occur while a kernel lock is held. Hardware contexts and stacks remain owned
//! outside this policy until their Exited slots are reaped.
use crate::time::TickInstant;

/// Generation-checked slot identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadId {
    slot: usize,
    generation: u64,
}
impl ThreadId {
    /// Index into the owner's context table.
    pub const fn slot(self) -> usize {
        self.slot
    }
}

/// Scheduling state. Sleeping deadlines use the monotonic counter domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadState {
    /// Eligible for the next round-robin selection.
    Ready,
    /// The sole currently executing thread.
    Running,
    /// Ineligible until its absolute deadline is reached.
    Sleeping(TickInstant),
    /// Ineligible until the owner explicitly publishes a completed event.
    Blocked,
    /// Terminal state, retained until explicit reaping.
    Exited,
}

/// Scheduling failures leave the policy unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    /// No slot is available.
    Full,
    /// A stale identity or invalid slot was supplied.
    Stale,
    /// A transition is invalid for the current state.
    State,
    /// A slot exhausted its non-wrapping generation space.
    GenerationExhausted,
}

#[derive(Clone, Copy)]
struct Thread {
    generation: u64,
    state: Option<ThreadState>,
}

/// Bounded scheduler with deterministic selection and no allocation.
pub struct Scheduler<const CAPACITY: usize> {
    threads: [Thread; CAPACITY],
    current: Option<ThreadId>,
    cursor: usize,
}
impl<const CAPACITY: usize> Default for Scheduler<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const CAPACITY: usize> Scheduler<CAPACITY> {
    /// Construct an empty run queue.
    pub const fn new() -> Self {
        Self {
            threads: [Thread {
                generation: 0,
                state: None,
            }; CAPACITY],
            current: None,
            cursor: 0,
        }
    }
    /// Reserve a ready slot. Stack/context publication must precede scheduling.
    pub fn spawn(&mut self) -> Result<ThreadId, ScheduleError> {
        let slot = self
            .threads
            .iter()
            .position(|thread| thread.state.is_none())
            .ok_or(ScheduleError::Full)?;
        let thread = &mut self.threads[slot];
        let generation = thread
            .generation
            .checked_add(1)
            .ok_or(ScheduleError::GenerationExhausted)?;
        thread.generation = generation;
        thread.state = Some(ThreadState::Ready);
        Ok(ThreadId { slot, generation })
    }
    /// Read a live thread's state, rejecting stale generations.
    pub fn state(&self, id: ThreadId) -> Result<ThreadState, ScheduleError> {
        let thread = self.threads.get(id.slot).ok_or(ScheduleError::Stale)?;
        if thread.generation != id.generation {
            return Err(ScheduleError::Stale);
        }
        thread.state.ok_or(ScheduleError::Stale)
    }
    /// Current thread, if one was selected.
    pub const fn current(&self) -> Option<ThreadId> {
        self.current
    }
    /// Suspend the current thread until an absolute deadline.
    pub fn sleep(&mut self, until: TickInstant) -> Result<(), ScheduleError> {
        let id = self.current.ok_or(ScheduleError::State)?;
        self.threads[id.slot].state = Some(ThreadState::Sleeping(until));
        self.current = None;
        Ok(())
    }
    /// Block the current thread at a lock-free scheduling boundary.
    pub fn block(&mut self) -> Result<(), ScheduleError> {
        let id = self.current.ok_or(ScheduleError::State)?;
        self.threads[id.slot].state = Some(ThreadState::Blocked);
        self.current = None;
        Ok(())
    }
    /// Publish a completed event before making its blocked owner runnable.
    /// Duplicate wakes and stale generations leave the queue unchanged.
    pub fn wake(&mut self, id: ThreadId) -> Result<(), ScheduleError> {
        if self.state(id)? != ThreadState::Blocked {
            return Err(ScheduleError::State);
        }
        self.threads[id.slot].state = Some(ThreadState::Ready);
        Ok(())
    }
    /// Mark the current thread terminal without freeing its active stack.
    pub fn exit(&mut self) -> Result<(), ScheduleError> {
        let id = self.current.ok_or(ScheduleError::State)?;
        self.threads[id.slot].state = Some(ThreadState::Exited);
        self.current = None;
        Ok(())
    }
    /// Wake expired sleepers and preempt to the next ready slot.
    ///
    /// Returns None only when no thread can run; the architecture must then use
    /// a permanently owned idle context. The previous context must be saved
    /// before acting on the result.
    pub fn select(&mut self, now: TickInstant) -> Option<ThreadId> {
        if let Some(id) = self.current.take() {
            self.threads[id.slot].state = Some(ThreadState::Ready);
        }
        for thread in &mut self.threads {
            if matches!(thread.state, Some(ThreadState::Sleeping(until)) if until <= now) {
                thread.state = Some(ThreadState::Ready);
            }
        }
        for offset in 0..CAPACITY {
            let slot = (self.cursor + offset) % CAPACITY;
            if self.threads[slot].state == Some(ThreadState::Ready) {
                self.threads[slot].state = Some(ThreadState::Running);
                let id = ThreadId {
                    slot,
                    generation: self.threads[slot].generation,
                };
                self.current = Some(id);
                self.cursor = (slot + 1) % CAPACITY;
                return Some(id);
            }
        }
        None
    }
    /// Free only an exited slot after the hardware owner retires its context.
    pub fn reap(&mut self, id: ThreadId) -> Result<(), ScheduleError> {
        if self.state(id)? != ThreadState::Exited {
            return Err(ScheduleError::State);
        }
        self.threads[id.slot].state = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocked_threads_require_one_generation_checked_wake() {
        let mut queue = Scheduler::<2>::new();
        let parent = queue.spawn().unwrap();
        let child = queue.spawn().unwrap();
        let now = TickInstant::from_raw(0);
        assert_eq!(queue.wake(parent), Err(ScheduleError::State));
        assert_eq!(queue.block(), Err(ScheduleError::State));
        assert_eq!(queue.select(now), Some(parent));
        queue.block().unwrap();
        assert_eq!(queue.state(parent), Ok(ThreadState::Blocked));
        // Advancing time must never wake an event waiter.
        assert_eq!(queue.select(TickInstant::from_raw(u64::MAX)), Some(child));
        queue.block().unwrap();
        assert_eq!(queue.select(now), None);
        queue.wake(parent).unwrap();
        assert_eq!(queue.wake(parent), Err(ScheduleError::State));
        assert_eq!(queue.select(now), Some(parent));
        queue.exit().unwrap();
        assert_eq!(queue.wake(parent), Err(ScheduleError::State));
        queue.reap(parent).unwrap();
        let replacement = queue.spawn().unwrap();
        assert_eq!(queue.wake(parent), Err(ScheduleError::Stale));
        assert_eq!(queue.select(now), Some(replacement));
        assert_eq!(queue.state(child), Ok(ThreadState::Blocked));
    }
    #[test]
    fn round_robin_sleep_exit_and_generation_reuse() {
        let mut scheduler = Scheduler::<3>::new();
        let a = scheduler.spawn().unwrap();
        let b = scheduler.spawn().unwrap();
        let c = scheduler.spawn().unwrap();
        assert_eq!(scheduler.spawn(), Err(ScheduleError::Full));
        assert_eq!(scheduler.select(TickInstant::from_raw(0)), Some(a));
        scheduler.sleep(TickInstant::from_raw(10)).unwrap();
        assert_eq!(scheduler.select(TickInstant::from_raw(1)), Some(b));
        assert_eq!(scheduler.select(TickInstant::from_raw(2)), Some(c));
        assert_eq!(scheduler.select(TickInstant::from_raw(9)), Some(b));
        assert_eq!(scheduler.select(TickInstant::from_raw(10)), Some(c));
        assert_eq!(scheduler.select(TickInstant::from_raw(11)), Some(a));
        scheduler.exit().unwrap();
        assert_eq!(scheduler.reap(b), Err(ScheduleError::State));
        scheduler.reap(a).unwrap();
        let replacement = scheduler.spawn().unwrap();
        assert_eq!(replacement.slot(), a.slot());
        assert_ne!(replacement, a);
        assert_eq!(scheduler.reap(a), Err(ScheduleError::Stale));
        assert_eq!(scheduler.state(replacement), Ok(ThreadState::Ready));
    }
    #[test]
    fn idle_and_zero_capacity_do_not_invent_runnable_threads() {
        assert_eq!(Scheduler::<0>::new().select(TickInstant::from_raw(0)), None);
        let mut scheduler = Scheduler::<1>::new();
        let id = scheduler.spawn().unwrap();
        assert_eq!(scheduler.select(TickInstant::from_raw(0)), Some(id));
        scheduler.sleep(TickInstant::from_raw(5)).unwrap();
        assert_eq!(scheduler.select(TickInstant::from_raw(4)), None);
        assert_eq!(scheduler.exit(), Err(ScheduleError::State));
        assert_eq!(scheduler.select(TickInstant::from_raw(5)), Some(id));
    }
}
