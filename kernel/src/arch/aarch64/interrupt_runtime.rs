//! Published single-CPU ownership for GICv3 delivery and the physical timer.

use core::marker::{PhantomData, PhantomPinned};
use core::mem;
use core::pin::Pin;

use crate::arch::aarch64::gicv3::{
    ActiveGicV3, GicV3Acknowledge, GicV3Configuration, GicV3DeliveryError, GicV3InterruptSource,
    GicV3RegisterAccess, PreparedGicV3, PrivatePeripheralInterrupt, PublishedIrqDispatcher,
    activate,
};
use crate::sync::{IrqSpinLock, LocalIrqControl};
use crate::time::{TickInstant, TimerEvent, TimerId};
use crate::timer_service::{
    FiredTimerBatch, LockedTimerQueue, TimerHardware, TimerProgrammingPermit, TimerServiceError,
};

/// Observable lifecycle of a published interrupt runtime slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptRuntimePhase {
    /// Stable storage exists but no vector lookup has been published.
    Unpublished,
    /// Dispatcher lookup is published and GIC delivery remains closed.
    Prepared,
    /// GIC delivery and timer programming are available.
    Active,
    /// A hardware or protocol failure prevents all later operations.
    Unavailable,
    /// An in-place transition is currently protected by the runtime lock.
    Transitioning,
}

/// Failure while opening interrupt delivery from a published slot.
#[derive(Debug, Eq, PartialEq)]
pub enum InterruptRuntimeActivationError<E> {
    /// The slot is not in its published prepared phase.
    Phase(InterruptRuntimePhase),
    /// The timer service was published for a different PPI than the GIC route.
    TimerInterruptMismatch {
        /// PPI prepared in the GIC.
        configured: u16,
        /// PPI bound to the timer service.
        published: u16,
    },
    /// GIC activation failed and the slot was made permanently unavailable.
    Gic {
        /// Failure that stopped activation.
        error: GicV3DeliveryError<E>,
        /// First failure while trying to close delivery again.
        recovery_error: Option<GicV3DeliveryError<E>>,
    },
}

/// Failure while dispatching or mutating the active timer service.
#[derive(Debug, Eq, PartialEq)]
pub enum InterruptRuntimeError<GE, TE> {
    /// The requested operation is unavailable in the current slot phase.
    Phase(InterruptRuntimePhase),
    /// GIC acknowledgement failed; the active runtime is closed to later use.
    Acknowledge(GicV3DeliveryError<GE>),
    /// Timer state or programming failed.
    Timer(TimerServiceError<TE>),
    /// GIC completion failed and left the acknowledged INTID outstanding.
    Completion {
        /// Exact GIC completion failure.
        error: GicV3DeliveryError<GE>,
        /// Timer failure observed before the mandatory completion attempt.
        timer_error: Option<TimerServiceError<TE>>,
    },
    /// GIC shutdown failed after the timer was quiesced.
    Shutdown(GicV3DeliveryError<GE>),
}

/// Result of one complete Group-1 acknowledgement/dispatch/EOI cycle.
#[derive(Debug, Eq, PartialEq)]
pub enum InterruptDispatch<const CAPACITY: usize> {
    /// The GIC returned one of the architectural special INTIDs.
    Spurious {
        /// Exact special INTID, in `1020..=1023`.
        intid: u32,
    },
    /// The configured timer PPI fired and due records were removed.
    Timer(FiredTimerBatch<CAPACITY>),
    /// An unhandled real INTID was nevertheless completed.
    Unexpected {
        /// Exact completed INTID.
        intid: u32,
    },
}

struct RuntimeOwner<B, const CAPACITY: usize, H> {
    gic: PreparedGicV3<B>,
    timer: LockedTimerQueue<CAPACITY, H>,
    timer_interrupt: PrivatePeripheralInterrupt,
}

struct ActiveRuntime<B, const CAPACITY: usize, H> {
    gic: ActiveGicV3<B>,
    timer: LockedTimerQueue<CAPACITY, H>,
    timer_interrupt: PrivatePeripheralInterrupt,
    permit: TimerProgrammingPermit,
    operational: bool,
}

struct FailedGicOwner<B> {
    _backend: B,
    _configuration: GicV3Configuration,
    _dispatcher: PublishedIrqDispatcher,
}

enum RuntimeState<B, const CAPACITY: usize, H> {
    Unpublished(RuntimeOwner<B, CAPACITY, H>),
    Prepared(RuntimeOwner<B, CAPACITY, H>),
    Active(ActiveRuntime<B, CAPACITY, H>),
    Failed {
        _gic: FailedGicOwner<B>,
        _timer: LockedTimerQueue<CAPACITY, H>,
    },
    Transitioning,
}

impl<B, const CAPACITY: usize, H> RuntimeState<B, CAPACITY, H> {
    fn phase(&self) -> InterruptRuntimePhase {
        match self {
            Self::Unpublished(_) => InterruptRuntimePhase::Unpublished,
            Self::Prepared(_) => InterruptRuntimePhase::Prepared,
            Self::Active(active) if active.operational => InterruptRuntimePhase::Active,
            Self::Active(_) | Self::Failed { .. } => InterruptRuntimePhase::Unavailable,
            Self::Transitioning => InterruptRuntimePhase::Transitioning,
        }
    }
}

/// Stable, single-CPU slot jointly owning GICv3 and timer state.
///
/// The slot is pinned before publication so an EL1 IRQ vector can retain one
/// permanent lookup address. All state transitions occur under one outer
/// IRQ-save lock. Timer operations then acquire the inner timer lock, defining
/// the only permitted lock order: runtime before timer.
pub struct InterruptRuntime<B, const CAPACITY: usize, H> {
    state: IrqSpinLock<RuntimeState<B, CAPACITY, H>>,
    _pinned: PhantomPinned,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<B, const CAPACITY: usize, H> InterruptRuntime<B, CAPACITY, H> {
    /// Create an unpublished slot around closed GIC and disarmed timer owners.
    pub const fn new(
        gic: PreparedGicV3<B>,
        timer: LockedTimerQueue<CAPACITY, H>,
        timer_interrupt: PrivatePeripheralInterrupt,
    ) -> Self {
        Self {
            state: IrqSpinLock::new(RuntimeState::Unpublished(RuntimeOwner {
                gic,
                timer,
                timer_interrupt,
            })),
            _pinned: PhantomPinned,
            _not_send_or_sync: PhantomData,
        }
    }

    /// Publish the pinned slot as the current CPU's permanent IRQ lookup.
    ///
    /// # Safety
    ///
    /// `self` must have a stable address until it returns to `Prepared` after
    /// its final deactivation. Every EL1 IRQ vector for this CPU must be able
    /// to recover this exact slot and invoke [`PublishedInterruptRuntime::dispatch_one`]
    /// before [`PublishedInterruptRuntime::activate`] can be called. The
    /// exception stack and `irq` implementation must remain valid throughout
    /// that interval, and no second slot may own the same GIC or timer.
    pub unsafe fn publish<'a, C: LocalIrqControl>(
        self: Pin<&'a Self>,
        irq: &C,
    ) -> Result<PublishedInterruptRuntime<'a, B, CAPACITY, H>, InterruptRuntimePhase> {
        let runtime = self.get_ref();
        let mut state = runtime.state.lock(irq);
        let owner = match mem::replace(&mut *state, RuntimeState::Transitioning) {
            RuntimeState::Unpublished(owner) => owner,
            other => {
                let phase = other.phase();
                *state = other;
                return Err(phase);
            }
        };
        *state = RuntimeState::Prepared(owner);
        Ok(PublishedInterruptRuntime {
            runtime: self,
            _not_send_or_sync: PhantomData,
        })
    }
}

/// Non-transferable handle to one permanently published interrupt slot.
pub struct PublishedInterruptRuntime<'a, B, const CAPACITY: usize, H> {
    runtime: Pin<&'a InterruptRuntime<B, CAPACITY, H>>,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl<'a, B, const CAPACITY: usize, H> PublishedInterruptRuntime<'a, B, CAPACITY, H> {
    /// Observe the slot phase under IRQ-safe serialization.
    pub fn phase<C: LocalIrqControl>(&self, irq: &C) -> InterruptRuntimePhase {
        self.runtime.state.lock(irq).phase()
    }
}

impl<'a, B: GicV3RegisterAccess, const CAPACITY: usize, H: TimerHardware>
    PublishedInterruptRuntime<'a, B, CAPACITY, H>
{
    /// Open GIC delivery and create the timer-programming permission.
    ///
    /// This method leaves the caller's preceding IRQ mask state unchanged.
    /// The transition lock masks IRQs before either delivery gate opens.
    pub fn activate<C: LocalIrqControl>(
        &self,
        irq: &C,
    ) -> Result<(), InterruptRuntimeActivationError<B::Error>> {
        let mut state = self.runtime.state.lock(irq);
        let owner = match mem::replace(&mut *state, RuntimeState::Transitioning) {
            RuntimeState::Prepared(owner) => owner,
            other => {
                let phase = other.phase();
                *state = other;
                return Err(InterruptRuntimeActivationError::Phase(phase));
            }
        };
        let configured = owner.gic.configuration().timer();
        if configured != owner.timer_interrupt {
            let published = owner.timer_interrupt;
            *state = RuntimeState::Prepared(owner);
            return Err(InterruptRuntimeActivationError::TimerInterruptMismatch {
                configured: configured.intid(),
                published: published.intid(),
            });
        }

        // SAFETY: `publish` requires the pinned slot, vector lookup, stack, and
        // dispatcher entry to remain reachable. This live outer lock has
        // masked local IRQs and uniquely owns the prepared GIC transition.
        let dispatcher = unsafe { PublishedIrqDispatcher::assume_published() };
        // SAFETY: the same live lock proves DAIF.I-equivalent masking for the
        // supplied architecture backend and excludes all competing access.
        match unsafe { activate(owner.gic, dispatcher) } {
            Ok(gic) => {
                // SAFETY: successful GIC activation plus the published slot
                // establish the matching vector, route, and lock ordering.
                let permit = unsafe { TimerProgrammingPermit::assume_ready() };
                *state = RuntimeState::Active(ActiveRuntime {
                    gic,
                    timer: owner.timer,
                    timer_interrupt: owner.timer_interrupt,
                    permit,
                    operational: true,
                });
                Ok(())
            }
            Err(failure) => {
                let (backend, configuration, dispatcher, error, recovery_error) =
                    failure.into_parts();
                *state = RuntimeState::Failed {
                    _gic: FailedGicOwner {
                        _backend: backend,
                        _configuration: configuration,
                        _dispatcher: dispatcher,
                    },
                    _timer: owner.timer,
                };
                Err(InterruptRuntimeActivationError::Gic {
                    error,
                    recovery_error,
                })
            }
        }
    }

    /// Schedule one timer only while GIC delivery is active.
    pub fn schedule<C: LocalIrqControl>(
        &self,
        irq: &C,
        deadline: TickInstant,
        event: TimerEvent,
    ) -> Result<TimerId, InterruptRuntimeError<B::Error, H::Error>> {
        let mut state = self.runtime.state.lock(irq);
        let RuntimeState::Active(active) = &mut *state else {
            return Err(InterruptRuntimeError::Phase(state.phase()));
        };
        if !active.operational {
            return Err(InterruptRuntimeError::Phase(
                InterruptRuntimePhase::Unavailable,
            ));
        }
        match active.timer.schedule(irq, &active.permit, deadline, event) {
            Ok(id) => Ok(id),
            Err(error) => {
                if timer_failure_is_terminal(&error) {
                    active.operational = false;
                }
                Err(InterruptRuntimeError::Timer(error))
            }
        }
    }

    /// Cancel one exact timer generation while delivery is active.
    pub fn cancel<C: LocalIrqControl>(
        &self,
        irq: &C,
        id: TimerId,
    ) -> Result<TimerEvent, InterruptRuntimeError<B::Error, H::Error>> {
        let mut state = self.runtime.state.lock(irq);
        let RuntimeState::Active(active) = &mut *state else {
            return Err(InterruptRuntimeError::Phase(state.phase()));
        };
        if !active.operational {
            return Err(InterruptRuntimeError::Phase(
                InterruptRuntimePhase::Unavailable,
            ));
        }
        match active.timer.cancel(irq, &active.permit, id) {
            Ok(event) => Ok(event),
            Err(error) => {
                if timer_failure_is_terminal(&error) {
                    active.operational = false;
                }
                Err(InterruptRuntimeError::Timer(error))
            }
        }
    }

    /// Acknowledge, service, and complete at most one Group-1 interrupt.
    ///
    /// Timer hardware is reprogrammed before EOI. The returned due batch
    /// becomes visible only after both nested locks have dropped, so callbacks
    /// cannot run while either controller is borrowed.
    pub fn dispatch_one<C: LocalIrqControl>(
        &self,
        irq: &C,
    ) -> Result<InterruptDispatch<CAPACITY>, InterruptRuntimeError<B::Error, H::Error>> {
        let mut state = self.runtime.state.lock(irq);
        let RuntimeState::Active(active) = &mut *state else {
            return Err(InterruptRuntimeError::Phase(state.phase()));
        };
        if !active.operational {
            return Err(InterruptRuntimeError::Phase(
                InterruptRuntimePhase::Unavailable,
            ));
        }

        let acknowledgement = match active.gic.acknowledge() {
            Ok(acknowledgement) => acknowledgement,
            Err(error) => {
                active.operational = false;
                return Err(InterruptRuntimeError::Acknowledge(error));
            }
        };
        let acknowledgement = match acknowledgement {
            GicV3Acknowledge::Spurious { intid } => {
                return Ok(InterruptDispatch::Spurious { intid });
            }
            GicV3Acknowledge::Interrupt(acknowledgement) => acknowledgement,
        };
        let intid = acknowledgement.intid();
        let source = acknowledgement.source();
        let timer_result = match source {
            GicV3InterruptSource::Timer => Some(active.timer.handle_interrupt(irq, &active.permit)),
            GicV3InterruptSource::Unexpected => None,
        };

        match acknowledgement.complete() {
            Ok(()) => match (source, timer_result) {
                (GicV3InterruptSource::Timer, Some(Ok(batch))) => {
                    Ok(InterruptDispatch::Timer(batch))
                }
                (GicV3InterruptSource::Timer, Some(Err(error))) => {
                    if timer_failure_is_terminal(&error) {
                        active.operational = false;
                    }
                    Err(InterruptRuntimeError::Timer(error))
                }
                (GicV3InterruptSource::Unexpected, None) => {
                    Ok(InterruptDispatch::Unexpected { intid })
                }
                _ => unreachable!(),
            },
            Err(failure) => {
                let (outstanding, error) = failure.into_parts();
                drop(outstanding);
                active.operational = false;
                Err(InterruptRuntimeError::Completion {
                    error,
                    timer_error: timer_result.and_then(Result::err),
                })
            }
        }
    }

    /// Quiesce an empty timer service, then close GIC delivery in order.
    pub fn deactivate<C: LocalIrqControl>(
        &self,
        irq: &C,
    ) -> Result<(), InterruptRuntimeError<B::Error, H::Error>> {
        let mut state = self.runtime.state.lock(irq);
        {
            let RuntimeState::Active(active) = &mut *state else {
                return Err(InterruptRuntimeError::Phase(state.phase()));
            };
            if !active.operational {
                return Err(InterruptRuntimeError::Phase(
                    InterruptRuntimePhase::Unavailable,
                ));
            }
            if let Err(error) = active.timer.quiesce(irq, &active.permit) {
                if timer_failure_is_terminal(&error) {
                    active.operational = false;
                }
                return Err(InterruptRuntimeError::Timer(error));
            }
        }

        let RuntimeState::Active(active) = mem::replace(&mut *state, RuntimeState::Transitioning)
        else {
            unreachable!();
        };
        let ActiveRuntime {
            gic,
            timer,
            timer_interrupt,
            permit,
            operational: _,
        } = active;
        permit.revoke();
        // SAFETY: the live outer lock masks local IRQs, prevents new handler
        // entry, and the runtime never lets an acknowledgement escape. Timer
        // quiescence completed before the CPU/distributor gates are closed.
        match unsafe { gic.shutdown() } {
            Ok((gic, dispatcher)) => {
                dispatcher.retire();
                *state = RuntimeState::Prepared(RuntimeOwner {
                    gic,
                    timer,
                    timer_interrupt,
                });
                Ok(())
            }
            Err(failure) => {
                let (backend, configuration, dispatcher, error) = failure.into_parts();
                *state = RuntimeState::Failed {
                    _gic: FailedGicOwner {
                        _backend: backend,
                        _configuration: configuration,
                        _dispatcher: dispatcher,
                    },
                    _timer: timer,
                };
                Err(InterruptRuntimeError::Shutdown(error))
            }
        }
    }
}

fn timer_failure_is_terminal<E>(error: &TimerServiceError<E>) -> bool {
    matches!(
        error,
        TimerServiceError::Unavailable
            | TimerServiceError::Programming {
                recovery_error: Some(_),
                ..
            }
    )
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::cell::{Cell, RefCell};
    use core::num::NonZeroU32;
    use std::pin::pin;
    use std::vec;
    use std::vec::Vec;

    use super::{
        InterruptDispatch, InterruptRuntime, InterruptRuntimeActivationError,
        InterruptRuntimeError, InterruptRuntimePhase,
    };
    use crate::arch::aarch64::gicv3::{
        CpuInterfaceRegister, GicV3Configuration, GicV3DeliveryError, GicV3DeliveryStage,
        GicV3RegisterAccess, InterruptTrigger, PreparedGicV3, PrivatePeripheralInterrupt,
    };
    use crate::sync::LocalIrqControl;
    use crate::time::{TickInstant, TimerEvent};
    use crate::timer_service::{
        LockedTimerQueue, OneShotTimerConsumer, TimerHardware, TimerProgramming, TimerServiceError,
    };

    const AFFINITY_ROUTING: u32 = 1 << 4;
    const ENABLE_NONSECURE_GROUP1: u32 = 1 << 1;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Mask(bool),
        Restore(bool),
        Distributor(u32),
        CpuGroup(u64),
        Acknowledge(u32),
        Timer(TimerProgramming),
        Eoi(u32),
    }

    struct MockIrq<'a> {
        masked: &'a Cell<bool>,
        operations: &'a RefCell<Vec<Operation>>,
    }

    // SAFETY: this mock saves, masks, and restores its exact Boolean IRQ state
    // on the single host thread that owns all runtime state.
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
    enum MockGicError {
        Activate,
        Eoi,
    }

    struct MockGic<'a> {
        operations: &'a RefCell<Vec<Operation>>,
        distributor: u32,
        group_enable: u64,
        control: u64,
        acknowledgements: Vec<u32>,
        next_acknowledgement: usize,
        fail_activation_once: bool,
        fail_eoi: bool,
    }

    impl<'a> MockGic<'a> {
        fn new(operations: &'a RefCell<Vec<Operation>>, acknowledgements: Vec<u32>) -> Self {
            Self {
                operations,
                distributor: AFFINITY_ROUTING,
                group_enable: 0,
                control: 0,
                acknowledgements,
                next_acknowledgement: 0,
                fail_activation_once: false,
                fail_eoi: false,
            }
        }
    }

    // SAFETY: this single-threaded mock uniquely owns all modeled GIC state,
    // aliases no memory, and records each access in program order.
    unsafe impl GicV3RegisterAccess for MockGic<'_> {
        type Error = MockGicError;

        fn read_distributor(&mut self, _offset: usize) -> Result<u32, Self::Error> {
            Ok(self.distributor)
        }

        fn write_distributor(&mut self, _offset: usize, value: u32) -> Result<(), Self::Error> {
            self.distributor = value;
            self.operations
                .borrow_mut()
                .push(Operation::Distributor(value));
            Ok(())
        }

        fn read_redistributor(&mut self, _offset: usize) -> Result<u32, Self::Error> {
            Ok(0)
        }

        fn write_redistributor(&mut self, _offset: usize, _value: u32) -> Result<(), Self::Error> {
            Ok(())
        }

        fn read_cpu_interface(
            &mut self,
            register: CpuInterfaceRegister,
        ) -> Result<u64, Self::Error> {
            Ok(match register {
                CpuInterfaceRegister::GroupEnable1 => self.group_enable,
                CpuInterfaceRegister::Control => self.control,
                _ => 0,
            })
        }

        fn write_cpu_interface(
            &mut self,
            register: CpuInterfaceRegister,
            value: u64,
        ) -> Result<(), Self::Error> {
            if register == CpuInterfaceRegister::GroupEnable1 {
                if value == 1 && self.fail_activation_once {
                    self.fail_activation_once = false;
                    return Err(MockGicError::Activate);
                }
                self.group_enable = value;
                self.operations
                    .borrow_mut()
                    .push(Operation::CpuGroup(value));
            } else if register == CpuInterfaceRegister::Control {
                self.control = value;
            }
            Ok(())
        }

        fn acknowledge_group1(&mut self) -> Result<u32, Self::Error> {
            let intid = self
                .acknowledgements
                .get(self.next_acknowledgement)
                .copied()
                .unwrap_or(1023);
            self.next_acknowledgement += 1;
            self.operations
                .borrow_mut()
                .push(Operation::Acknowledge(intid));
            Ok(intid)
        }

        fn end_group1(&mut self, intid: u32) -> Result<(), Self::Error> {
            self.operations.borrow_mut().push(Operation::Eoi(intid));
            if self.fail_eoi {
                Err(MockGicError::Eoi)
            } else {
                Ok(())
            }
        }

        fn synchronize(&mut self) {}
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum MockTimerError {
        Program,
    }

    struct MockTimer<'a> {
        now: &'a Cell<TickInstant>,
        masked: &'a Cell<bool>,
        operations: &'a RefCell<Vec<Operation>>,
        programming_count: usize,
        fail_programming: Vec<usize>,
    }

    impl TimerHardware for MockTimer<'_> {
        type Error = MockTimerError;

        fn counter(&mut self) -> Result<TickInstant, Self::Error> {
            Ok(self.now.get())
        }

        unsafe fn program(&mut self, programming: TimerProgramming) -> Result<(), Self::Error> {
            assert!(self.masked.get(), "timer programming requires masked IRQs");
            self.operations
                .borrow_mut()
                .push(Operation::Timer(programming));
            let count = self.programming_count;
            self.programming_count += 1;
            if self.fail_programming.contains(&count) {
                Err(MockTimerError::Program)
            } else {
                Ok(())
            }
        }
    }

    fn timer_ppi(intid: u16) -> PrivatePeripheralInterrupt {
        PrivatePeripheralInterrupt::new(intid, InterruptTrigger::LevelHigh).unwrap()
    }

    fn prepared_gic<'a>(
        operations: &'a RefCell<Vec<Operation>>,
        acknowledgements: Vec<u32>,
    ) -> PreparedGicV3<MockGic<'a>> {
        let configuration = GicV3Configuration::new(timer_ppi(30), NonZeroU32::new(4).unwrap());
        // SAFETY: the mock starts with affinity routing set, both delivery
        // gates closed, combined EOI mode selected, and PPI 30 modeled ready.
        unsafe {
            PreparedGicV3::assume_prepared_for_test(
                MockGic::new(operations, acknowledgements),
                configuration,
            )
        }
    }

    fn timer_service<'a, const CAPACITY: usize>(
        now: &'a Cell<TickInstant>,
        masked: &'a Cell<bool>,
        operations: &'a RefCell<Vec<Operation>>,
        fail_programming: Vec<usize>,
    ) -> LockedTimerQueue<CAPACITY, MockTimer<'a>> {
        let timer = MockTimer {
            now,
            masked,
            operations,
            programming_count: 0,
            fail_programming,
        };
        // SAFETY: the mock timer starts disabled/masked and its counter value
        // belongs to the single fixed test domain.
        unsafe { LockedTimerQueue::assume_disarmed(now.get(), timer) }
    }

    #[test]
    fn publication_is_one_way_and_rejects_a_mismatched_timer_route() {
        let operations = RefCell::new(Vec::new());
        let masked = Cell::new(true);
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: &masked,
            operations: &operations,
        };
        let runtime = InterruptRuntime::<_, 1, _>::new(
            prepared_gic(&operations, Vec::new()),
            timer_service(&now, &masked, &operations, Vec::new()),
            timer_ppi(29),
        );
        let runtime = pin!(runtime);
        // SAFETY: the pinned host slot and direct handle model a permanent
        // vector lookup and dispatcher on this one test thread.
        let published = unsafe { runtime.as_ref().publish(&irq) }.unwrap();
        assert_eq!(published.phase(&irq), InterruptRuntimePhase::Prepared);
        assert_eq!(
            unsafe { runtime.as_ref().publish(&irq) }.err(),
            Some(InterruptRuntimePhase::Prepared)
        );
        assert_eq!(
            published.activate(&irq),
            Err(InterruptRuntimeActivationError::TimerInterruptMismatch {
                configured: 30,
                published: 29,
            })
        );
        assert_eq!(published.phase(&irq), InterruptRuntimePhase::Prepared);
    }

    #[test]
    fn active_runtime_orders_timer_reprogramming_eoi_and_shutdown() {
        let operations = RefCell::new(Vec::new());
        let masked = Cell::new(false);
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: &masked,
            operations: &operations,
        };
        let runtime = InterruptRuntime::<_, 2, _>::new(
            prepared_gic(&operations, vec![30]),
            timer_service(&now, &masked, &operations, Vec::new()),
            timer_ppi(30),
        );
        let runtime = pin!(runtime);
        // SAFETY: the pinned test slot is the only modeled dispatcher lookup.
        let published = unsafe { runtime.as_ref().publish(&irq) }.unwrap();
        published.activate(&irq).unwrap();
        assert_eq!(published.phase(&irq), InterruptRuntimePhase::Active);
        let event = TimerEvent::new(0x55);
        let id = published
            .schedule(&irq, TickInstant::from_raw(10), event)
            .unwrap();
        let consumer = OneShotTimerConsumer::new();
        consumer.bind(&irq, id, event).unwrap();

        now.set(TickInstant::from_raw(10));
        let dispatch = published.dispatch_one(&irq).unwrap();
        let InterruptDispatch::Timer(batch) = dispatch else {
            panic!("timer acknowledgement was not dispatched as a timer");
        };
        assert_eq!(
            batch.iter().map(|timer| timer.event()).collect::<Vec<_>>(),
            vec![event]
        );
        let consumer_lock = operations.borrow().len();
        let fired = consumer.consume(&irq, &batch).unwrap();
        assert_eq!(fired.id(), id);
        assert_eq!(operations.borrow()[consumer_lock], Operation::Mask(false));
        published.deactivate(&irq).unwrap();
        assert_eq!(published.phase(&irq), InterruptRuntimePhase::Prepared);
        assert!(!masked.get());

        let operations = operations.borrow();
        let distributor_open = operations
            .iter()
            .position(|entry| {
                *entry == Operation::Distributor(AFFINITY_ROUTING | ENABLE_NONSECURE_GROUP1)
            })
            .unwrap();
        let cpu_open = operations
            .iter()
            .position(|entry| *entry == Operation::CpuGroup(1))
            .unwrap();
        let timer_disarm = operations
            .iter()
            .position(|entry| *entry == Operation::Timer(TimerProgramming::Disarm))
            .unwrap();
        let eoi = operations
            .iter()
            .position(|entry| *entry == Operation::Eoi(30))
            .unwrap();
        let final_disarm = operations
            .iter()
            .rposition(|entry| *entry == Operation::Timer(TimerProgramming::Disarm))
            .unwrap();
        let cpu_close = operations
            .iter()
            .rposition(|entry| *entry == Operation::CpuGroup(0))
            .unwrap();
        let distributor_close = operations
            .iter()
            .rposition(|entry| *entry == Operation::Distributor(AFFINITY_ROUTING))
            .unwrap();
        assert!(distributor_open < cpu_open);
        assert!(timer_disarm < eoi);
        assert!(eoi < consumer_lock);
        assert!(final_disarm < cpu_close);
        assert!(cpu_close < distributor_close);
    }

    #[test]
    fn pending_timers_block_deactivation_without_closing_delivery() {
        let operations = RefCell::new(Vec::new());
        let masked = Cell::new(false);
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: &masked,
            operations: &operations,
        };
        let runtime = InterruptRuntime::<_, 1, _>::new(
            prepared_gic(&operations, Vec::new()),
            timer_service(&now, &masked, &operations, Vec::new()),
            timer_ppi(30),
        );
        let runtime = pin!(runtime);
        // SAFETY: the pinned test slot models the only permanent IRQ lookup.
        let published = unsafe { runtime.as_ref().publish(&irq) }.unwrap();
        published.activate(&irq).unwrap();
        let id = published
            .schedule(&irq, TickInstant::from_raw(10), TimerEvent::new(1))
            .unwrap();
        assert_eq!(
            published.deactivate(&irq),
            Err(InterruptRuntimeError::Timer(
                TimerServiceError::PendingTimers
            ))
        );
        assert_eq!(published.phase(&irq), InterruptRuntimePhase::Active);
        assert_eq!(published.cancel(&irq, id), Ok(TimerEvent::new(1)));
        published.deactivate(&irq).unwrap();
    }

    #[test]
    fn activation_and_eoi_failures_make_the_slot_unavailable() {
        let operations = RefCell::new(Vec::new());
        let masked = Cell::new(false);
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: &masked,
            operations: &operations,
        };
        let mut failed_gic = MockGic::new(&operations, Vec::new());
        failed_gic.fail_activation_once = true;
        let configuration = GicV3Configuration::new(timer_ppi(30), NonZeroU32::new(4).unwrap());
        // SAFETY: this mock satisfies prepared state before injecting one CPU
        // gate write failure during activation.
        let prepared =
            unsafe { PreparedGicV3::assume_prepared_for_test(failed_gic, configuration) };
        let runtime = InterruptRuntime::<_, 1, _>::new(
            prepared,
            timer_service(&now, &masked, &operations, Vec::new()),
            timer_ppi(30),
        );
        let runtime = pin!(runtime);
        // SAFETY: the pinned test slot models the only permanent IRQ lookup.
        let published = unsafe { runtime.as_ref().publish(&irq) }.unwrap();
        assert!(matches!(
            published.activate(&irq),
            Err(InterruptRuntimeActivationError::Gic {
                error: GicV3DeliveryError::Access {
                    stage: GicV3DeliveryStage::EnableCpuInterface,
                    error: MockGicError::Activate,
                },
                recovery_error: None,
            })
        ));
        assert_eq!(published.phase(&irq), InterruptRuntimePhase::Unavailable);

        let operations = RefCell::new(Vec::new());
        let masked = Cell::new(false);
        let now = Cell::new(TickInstant::from_raw(1));
        let irq = MockIrq {
            masked: &masked,
            operations: &operations,
        };
        let mut failed_gic = MockGic::new(&operations, vec![30]);
        failed_gic.fail_eoi = true;
        // SAFETY: the second mock also starts in the complete prepared state.
        let prepared =
            unsafe { PreparedGicV3::assume_prepared_for_test(failed_gic, configuration) };
        let runtime = InterruptRuntime::<_, 1, _>::new(
            prepared,
            timer_service(&now, &masked, &operations, Vec::new()),
            timer_ppi(30),
        );
        let runtime = pin!(runtime);
        // SAFETY: the pinned test slot models the only permanent IRQ lookup.
        let published = unsafe { runtime.as_ref().publish(&irq) }.unwrap();
        published.activate(&irq).unwrap();
        assert!(matches!(
            published.dispatch_one(&irq),
            Err(InterruptRuntimeError::Completion {
                error: GicV3DeliveryError::Access {
                    stage: GicV3DeliveryStage::EndOfInterrupt,
                    error: MockGicError::Eoi,
                },
                timer_error: None,
            })
        ));
        assert_eq!(published.phase(&irq), InterruptRuntimePhase::Unavailable);
        assert_eq!(
            published.dispatch_one(&irq),
            Err(InterruptRuntimeError::Phase(
                InterruptRuntimePhase::Unavailable
            ))
        );
    }
}
