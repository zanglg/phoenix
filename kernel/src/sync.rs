//! Architecture-neutral synchronization primitives.

use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Per-CPU interrupt masking used by [`IrqSpinLock`].
///
/// # Safety
///
/// An implementation must mask ordinary local IRQ delivery before
/// `save_and_mask` returns and encode the exact preceding mask state in the
/// returned value. `restore` must consume a state produced on the same CPU and
/// restore only that preceding IRQ state. Neither method may migrate execution,
/// access the protected value, or weaken the lock's acquire/release ordering.
/// The caller must not change the local IRQ mask independently while a state is
/// outstanding.
pub unsafe trait LocalIrqControl {
    /// Opaque saved state that must be restored on the same CPU.
    type State;

    /// Save the current local IRQ state and mask ordinary IRQ delivery.
    fn save_and_mask(&self) -> Self::State;

    /// Restore the exact local IRQ state captured by `save_and_mask`.
    fn restore(&self, state: Self::State);
}

/// Spinlock that masks local IRQ delivery while its guard is held.
///
/// Acquisition masks local IRQs before attempting the atomic transition, then
/// uses acquire ordering. Guard drop publishes protected writes with release
/// ordering before restoring the caller's exact prior IRQ state. This prevents
/// same-CPU IRQ re-entry from deadlocking on a lock held by interrupted normal
/// context while retaining an SMP-capable mutual-exclusion boundary.
///
/// The lock is non-recursive, non-fair, and deliberately has no poisoning.
/// Callers must define a global lock order and must not sleep, schedule, or
/// invoke arbitrary callbacks while holding a guard.
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: moving the lock transfers its contained value. Safe access to that
// value still requires exclusive ownership or acquisition of the atomic lock.
unsafe impl<T: Send> Send for IrqSpinLock<T> {}

// SAFETY: acquire/release locking serializes every shared access to the value.
// `T: Send` is sufficient because a guard exposes the value to only one CPU at
// a time, and the guard itself cannot be sent or shared across CPUs.
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    /// Create an unlocked IRQ-safe spinlock.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Acquire the lock after saving and masking local IRQ delivery.
    ///
    /// This may spin indefinitely if another CPU never releases the lock. A
    /// recursive same-CPU acquisition is invalid and also cannot make progress.
    pub fn lock<'a, C: LocalIrqControl>(&'a self, irq: &'a C) -> IrqSpinLockGuard<'a, T, C> {
        let state = irq.save_and_mask();
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return IrqSpinLockGuard::new(self, irq, state);
            }
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
    }

    /// Try once to acquire the lock with local IRQ delivery masked.
    ///
    /// Contention restores the exact preceding IRQ state before returning
    /// `None`; it never leaves IRQs accidentally masked.
    pub fn try_lock<'a, C: LocalIrqControl>(
        &'a self,
        irq: &'a C,
    ) -> Option<IrqSpinLockGuard<'a, T, C>> {
        let state = irq.save_and_mask();
        match self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => Some(IrqSpinLockGuard::new(self, irq, state)),
            Err(_) => {
                irq.restore(state);
                None
            }
        }
    }

    /// Borrow the contained value without locking through an exclusive lock
    /// borrow.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    /// Consume the lock and return its contained value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

/// Exclusive access returned by [`IrqSpinLock::lock`] or
/// [`IrqSpinLock::try_lock`].
///
/// The guard is deliberately neither `Send` nor `Sync`: its saved IRQ state
/// belongs to the CPU and execution context that acquired it.
#[must_use = "dropping the guard releases the lock and restores local IRQ state"]
pub struct IrqSpinLockGuard<'a, T, C: LocalIrqControl> {
    lock: &'a IrqSpinLock<T>,
    irq: &'a C,
    state: Option<C::State>,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<'a, T, C: LocalIrqControl> IrqSpinLockGuard<'a, T, C> {
    fn new(lock: &'a IrqSpinLock<T>, irq: &'a C, state: C::State) -> Self {
        Self {
            lock,
            irq,
            state: Some(state),
            _not_send_or_sync: PhantomData,
        }
    }
}

impl<T, C: LocalIrqControl> Deref for IrqSpinLockGuard<'_, T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: the guard exists only after the atomic transition from
        // unlocked to locked and holds that unique logical access until Drop.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T, C: LocalIrqControl> DerefMut for IrqSpinLockGuard<'_, T, C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: only one guard can exist while the atomic state is locked,
        // and this mutable borrow is scoped to that guard.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T, C: LocalIrqControl> Drop for IrqSpinLockGuard<'_, T, C> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        let Some(state) = self.state.take() else {
            return;
        };
        self.irq.restore(state);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::cell::{Cell, RefCell};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::vec;
    use std::vec::Vec;

    use super::{IrqSpinLock, LocalIrqControl};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Mask { previously_masked: bool },
        Restore { masked: bool },
    }

    struct MockIrqControl {
        masked: Cell<bool>,
        events: RefCell<Vec<Event>>,
    }

    impl MockIrqControl {
        fn new(masked: bool) -> Self {
            Self {
                masked: Cell::new(masked),
                events: RefCell::new(Vec::new()),
            }
        }
    }

    // SAFETY: the single-threaded mock records and restores its exact Boolean
    // IRQ state and is used only on the host test thread that created it.
    unsafe impl LocalIrqControl for MockIrqControl {
        type State = bool;

        fn save_and_mask(&self) -> Self::State {
            let previously_masked = self.masked.replace(true);
            self.events
                .borrow_mut()
                .push(Event::Mask { previously_masked });
            previously_masked
        }

        fn restore(&self, state: Self::State) {
            self.masked.set(state);
            self.events
                .borrow_mut()
                .push(Event::Restore { masked: state });
        }
    }

    #[test]
    fn lock_masks_before_access_and_restores_the_exact_prior_state() {
        let irq = MockIrqControl::new(false);
        let mut lock = IrqSpinLock::new(7_u32);
        {
            let mut guard = lock.lock(&irq);
            assert!(irq.masked.get());
            assert_eq!(*guard, 7);
            *guard = 11;
        }
        assert!(!irq.masked.get());
        assert_eq!(*lock.get_mut(), 11);
        assert_eq!(
            *irq.events.borrow(),
            vec![
                Event::Mask {
                    previously_masked: false,
                },
                Event::Restore { masked: false },
            ]
        );
    }

    #[test]
    fn initially_masked_caller_stays_masked_after_unlock() {
        let irq = MockIrqControl::new(true);
        let lock = IrqSpinLock::new(());
        {
            let _guard = lock.lock(&irq);
            assert!(irq.masked.get());
        }
        assert!(irq.masked.get());
        assert_eq!(
            *irq.events.borrow(),
            vec![
                Event::Mask {
                    previously_masked: true,
                },
                Event::Restore { masked: true },
            ]
        );
    }

    #[test]
    fn nested_locks_preserve_the_outer_mask_and_restore_in_lifo_order() {
        let irq = MockIrqControl::new(false);
        let outer = IrqSpinLock::new(1_u8);
        let inner = IrqSpinLock::new(2_u8);
        {
            let _outer = outer.lock(&irq);
            assert!(irq.masked.get());
            {
                let _inner = inner.lock(&irq);
                assert!(irq.masked.get());
            }
            assert!(irq.masked.get());
        }
        assert!(!irq.masked.get());
        assert_eq!(
            *irq.events.borrow(),
            vec![
                Event::Mask {
                    previously_masked: false,
                },
                Event::Mask {
                    previously_masked: true,
                },
                Event::Restore { masked: true },
                Event::Restore { masked: false },
            ]
        );
    }

    #[test]
    fn try_lock_contention_restores_state_without_unlocking_the_owner() {
        let irq = MockIrqControl::new(false);
        let lock = IrqSpinLock::new(3_u8);
        let owner = lock.lock(&irq);
        assert!(lock.try_lock(&irq).is_none());
        assert!(irq.masked.get());
        assert_eq!(*owner, 3);
        drop(owner);
        assert!(!irq.masked.get());
        assert_eq!(
            *irq.events.borrow(),
            vec![
                Event::Mask {
                    previously_masked: false,
                },
                Event::Mask {
                    previously_masked: true,
                },
                Event::Restore { masked: true },
                Event::Restore { masked: false },
            ]
        );
    }

    struct ThreadIrqControl;

    // SAFETY: each host thread constructs its own controller. The Boolean
    // token models an initially unmasked local IRQ state and has no shared
    // state beyond the lock's atomic acquire/release protocol.
    unsafe impl LocalIrqControl for ThreadIrqControl {
        type State = bool;

        fn save_and_mask(&self) -> Self::State {
            false
        }

        fn restore(&self, _state: Self::State) {}
    }

    #[test]
    fn acquire_release_ordering_serializes_host_threads() {
        const THREADS: usize = 4;
        const INCREMENTS: usize = 2_000;

        let lock = Arc::new(IrqSpinLock::new(0_usize));
        let start = Arc::new(Barrier::new(THREADS));
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let lock = Arc::clone(&lock);
            let start = Arc::clone(&start);
            handles.push(thread::spawn(move || {
                let irq = ThreadIrqControl;
                start.wait();
                for _ in 0..INCREMENTS {
                    *lock.lock(&irq) += 1;
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let lock = match Arc::try_unwrap(lock) {
            Ok(lock) => lock,
            Err(_) => panic!("all worker references must be dropped"),
        };
        assert_eq!(lock.into_inner(), THREADS * INCREMENTS);
    }
}
