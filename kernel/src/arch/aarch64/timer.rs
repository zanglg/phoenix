//! AArch64 physical generic-timer values and EL1 register access.

use core::num::NonZeroU32;

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

use crate::time::{MonotonicTimeError, TickDuration, TickInstant};

/// Number of nanoseconds in one second.
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

const CONTROL_ENABLE: u64 = 1 << 0;
const CONTROL_INTERRUPT_MASK: u64 = 1 << 1;
const CONTROL_CONDITION_MET: u64 = 1 << 2;
const CONTROL_DEFINED_MASK: u64 = CONTROL_ENABLE | CONTROL_INTERRUPT_MASK | CONTROL_CONDITION_MET;
#[cfg(target_arch = "aarch64")]
const CONTROL_DISABLED_MASKED: u64 = CONTROL_INTERRUPT_MASK;
#[cfg(target_arch = "aarch64")]
const CONTROL_ENABLED_MASKED: u64 = CONTROL_ENABLE | CONTROL_INTERRUPT_MASK;
#[cfg(target_arch = "aarch64")]
const CONTROL_ENABLED_UNMASKED: u64 = CONTROL_ENABLE;

/// Invalid `CNTFRQ_EL0` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CounterFrequencyError {
    /// The architectural frequency register reported zero ticks per second.
    Zero,
    /// Bits outside the 32-bit architectural frequency field were set.
    TooWide {
        /// Rejected raw register value.
        raw: u64,
    },
}

/// Failure while converting an external duration into counter ticks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerConversionError {
    /// The rounded-up tick count does not fit the supported `u64` timeline.
    TickCountOverflow {
        /// Requested duration.
        nanoseconds: u64,
        /// Counter frequency used for the conversion.
        ticks_per_second: u32,
    },
    /// Adding the converted duration would wrap the absolute counter deadline.
    DeadlineOverflow {
        /// Starting counter value.
        now: TickInstant,
        /// Successfully converted duration that could not be added.
        duration: TickDuration,
    },
}

/// Valid nonzero AArch64 generic-counter frequency.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CounterFrequency(NonZeroU32);

impl CounterFrequency {
    /// Validate the complete value read from `CNTFRQ_EL0`.
    pub const fn new(raw: u64) -> Result<Self, CounterFrequencyError> {
        if raw == 0 {
            return Err(CounterFrequencyError::Zero);
        }
        if raw > u32::MAX as u64 {
            return Err(CounterFrequencyError::TooWide { raw });
        }
        let Some(raw) = NonZeroU32::new(raw as u32) else {
            return Err(CounterFrequencyError::Zero);
        };
        Ok(Self(raw))
    }

    /// Return the exact hardware counter frequency in ticks per second.
    pub const fn ticks_per_second(self) -> u32 {
        self.0.get()
    }

    /// Convert nanoseconds to ticks, rounding every positive remainder up.
    ///
    /// Rounding up prevents a timer from being programmed earlier than the
    /// requested duration. Zero nanoseconds remains exactly zero ticks.
    pub fn ticks_for_nanoseconds_ceil(
        self,
        nanoseconds: u64,
    ) -> Result<TickDuration, TimerConversionError> {
        let product = u128::from(nanoseconds) * u128::from(self.ticks_per_second());
        let divisor = u128::from(NANOS_PER_SECOND);
        let ticks = if product == 0 {
            0
        } else {
            ((product - 1) / divisor) + 1
        };
        let Ok(ticks) = u64::try_from(ticks) else {
            return Err(TimerConversionError::TickCountOverflow {
                nanoseconds,
                ticks_per_second: self.ticks_per_second(),
            });
        };
        Ok(TickDuration::from_raw(ticks))
    }

    /// Form an absolute deadline from a current tick and relative nanoseconds.
    pub fn deadline_after_nanoseconds(
        self,
        now: TickInstant,
        nanoseconds: u64,
    ) -> Result<TickInstant, TimerConversionError> {
        let duration = self.ticks_for_nanoseconds_ceil(nanoseconds)?;
        match now.checked_add(duration) {
            Ok(deadline) => Ok(deadline),
            Err(MonotonicTimeError::Overflow) => {
                Err(TimerConversionError::DeadlineOverflow { now, duration })
            }
            Err(MonotonicTimeError::Regression { .. }) => {
                unreachable!("adding a duration cannot regress an instant")
            }
        }
    }
}

/// Decoded low architectural bits of `CNTP_CTL_EL0`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalTimerControl(u8);

impl PhysicalTimerControl {
    /// Decode the defined enable, mask, and condition bits.
    pub const fn from_raw(raw: u64) -> Self {
        Self((raw & CONTROL_DEFINED_MASK) as u8)
    }

    /// Return the normalized defined-bit value.
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// Return whether the physical timer is enabled.
    pub const fn enabled(self) -> bool {
        (self.0 as u64) & CONTROL_ENABLE != 0
    }

    /// Return whether timer interrupt signaling is masked.
    pub const fn interrupt_masked(self) -> bool {
        (self.0 as u64) & CONTROL_INTERRUPT_MASK != 0
    }

    /// Return the read-only timer condition status.
    pub const fn condition_met(self) -> bool {
        (self.0 as u64) & CONTROL_CONDITION_MET != 0
    }
}

/// Exclusive EL1 access to the AArch64 physical generic timer.
///
/// This owner is available only in AArch64 builds. It does not initialize or
/// acknowledge a GIC interrupt and does not change `DAIF` masking.
#[cfg(target_arch = "aarch64")]
#[derive(Debug)]
pub struct PhysicalTimer {
    _exclusive: (),
}

#[cfg(target_arch = "aarch64")]
impl PhysicalTimer {
    /// Claim the one physical timer comparator owned by the kernel.
    ///
    /// # Safety
    ///
    /// The caller must create at most one live owner for the current CPU and
    /// ensure execution is at EL1 with physical counter/timer access enabled.
    /// Before calling [`Self::unmask`], the caller must also establish its IRQ
    /// dispatch and synchronization invariants.
    pub const unsafe fn claim() -> Self {
        Self { _exclusive: () }
    }

    /// Read and validate `CNTFRQ_EL0`.
    pub fn frequency(&self) -> Result<CounterFrequency, CounterFrequencyError> {
        let raw: u64;
        // SAFETY: the exclusive owner contract requires EL1 counter access;
        // this read has no memory side effects.
        unsafe {
            asm!(
                "mrs {raw}, CNTFRQ_EL0",
                raw = out(reg) raw,
                options(nomem, nostack, preserves_flags)
            );
        }
        CounterFrequency::new(raw)
    }

    /// Read an ordered physical counter observation from `CNTPCT_EL0`.
    pub fn counter(&self) -> TickInstant {
        let raw: u64;
        // SAFETY: the exclusive owner contract requires EL1 counter access.
        // `isb` orders the observation after earlier instructions.
        unsafe {
            asm!(
                "isb",
                "mrs {raw}, CNTPCT_EL0",
                raw = out(reg) raw,
                options(nomem, nostack, preserves_flags)
            );
        }
        TickInstant::from_raw(raw)
    }

    /// Read the defined physical-timer control and status bits.
    pub fn control(&self) -> PhysicalTimerControl {
        let raw: u64;
        // SAFETY: the exclusive owner contract requires EL1 physical-timer
        // access; the register read does not modify timer state.
        unsafe {
            asm!(
                "mrs {raw}, CNTP_CTL_EL0",
                raw = out(reg) raw,
                options(nomem, nostack, preserves_flags)
            );
        }
        PhysicalTimerControl::from_raw(raw)
    }

    /// Program an absolute compare value and leave signaling enabled but masked.
    ///
    /// A past deadline is valid: the condition becomes asserted immediately,
    /// but cannot signal an IRQ until [`Self::unmask`] is called.
    pub fn program_masked(&mut self, deadline: TickInstant) {
        let deadline = deadline.raw();
        // SAFETY: the exclusive owner contract permits writes to this CPU's
        // physical timer. Exact control writes keep signaling masked across the
        // compare update, and each `isb` completes the preceding state change.
        unsafe {
            asm!(
                "msr CNTP_CTL_EL0, {disabled_masked}",
                "isb",
                "msr CNTP_CVAL_EL0, {deadline}",
                "isb",
                "msr CNTP_CTL_EL0, {enabled_masked}",
                "isb",
                disabled_masked = in(reg) CONTROL_DISABLED_MASKED,
                deadline = in(reg) deadline,
                enabled_masked = in(reg) CONTROL_ENABLED_MASKED,
                options(nomem, nostack, preserves_flags)
            );
        }
    }

    /// Mask interrupt signaling while preserving the current enable state.
    pub fn mask(&mut self) {
        let value = if self.control().enabled() {
            CONTROL_ENABLED_MASKED
        } else {
            CONTROL_DISABLED_MASKED
        };
        // SAFETY: the exclusive owner contract permits the exact timer-control
        // write. Masking cannot introduce a new interrupt signal.
        unsafe {
            asm!(
                "msr CNTP_CTL_EL0, {value}",
                "isb",
                value = in(reg) value,
                options(nomem, nostack, preserves_flags)
            );
        }
    }

    /// Disable the timer and mask interrupt signaling.
    pub fn disable(&mut self) {
        // SAFETY: the exclusive owner contract permits the exact timer-control
        // write. The resulting state cannot signal a timer interrupt.
        unsafe {
            asm!(
                "msr CNTP_CTL_EL0, {value}",
                "isb",
                value = in(reg) CONTROL_DISABLED_MASKED,
                options(nomem, nostack, preserves_flags)
            );
        }
    }

    /// Unmask signaling for an already programmed, enabled timer.
    ///
    /// # Safety
    ///
    /// The caller must have installed a correct exception vector and GIC
    /// route, made the handler's queue state IRQ-safe, and programmed a compare
    /// value. The timer condition may already be asserted, so an interrupt can
    /// become pending as soon as this write completes or IRQs are unmasked.
    pub unsafe fn unmask(&mut self) {
        // SAFETY: the caller establishes the IRQ delivery and shared-state
        // invariants required before this exact unmasking write.
        unsafe {
            asm!(
                "msr CNTP_CTL_EL0, {value}",
                "isb",
                value = in(reg) CONTROL_ENABLED_UNMASKED,
                options(nomem, nostack, preserves_flags)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CounterFrequency, CounterFrequencyError, NANOS_PER_SECOND, PhysicalTimerControl,
        TimerConversionError,
    };
    use crate::time::{TickDuration, TickInstant};

    #[test]
    fn counter_frequency_rejects_zero_and_values_wider_than_the_register() {
        assert_eq!(CounterFrequency::new(0), Err(CounterFrequencyError::Zero));
        assert_eq!(
            CounterFrequency::new(u64::from(u32::MAX) + 1),
            Err(CounterFrequencyError::TooWide {
                raw: u64::from(u32::MAX) + 1,
            })
        );
        assert_eq!(
            CounterFrequency::new(24_000_000)
                .unwrap()
                .ticks_per_second(),
            24_000_000
        );
    }

    #[test]
    fn nanosecond_conversion_rounds_up_without_early_deadlines() {
        let frequency = CounterFrequency::new(24_000_000).unwrap();
        assert_eq!(
            frequency.ticks_for_nanoseconds_ceil(0),
            Ok(TickDuration::from_raw(0))
        );
        assert_eq!(
            frequency.ticks_for_nanoseconds_ceil(1),
            Ok(TickDuration::from_raw(1))
        );
        assert_eq!(
            frequency.ticks_for_nanoseconds_ceil(41),
            Ok(TickDuration::from_raw(1))
        );
        assert_eq!(
            frequency.ticks_for_nanoseconds_ceil(42),
            Ok(TickDuration::from_raw(2))
        );
        assert_eq!(
            frequency.ticks_for_nanoseconds_ceil(NANOS_PER_SECOND),
            Ok(TickDuration::from_raw(24_000_000))
        );
    }

    #[test]
    fn conversion_and_deadline_overflow_are_distinct() {
        let fastest = CounterFrequency::new(u64::from(u32::MAX)).unwrap();
        assert_eq!(
            fastest.ticks_for_nanoseconds_ceil(u64::MAX),
            Err(TimerConversionError::TickCountOverflow {
                nanoseconds: u64::MAX,
                ticks_per_second: u32::MAX,
            })
        );

        let one_ghz = CounterFrequency::new(NANOS_PER_SECOND).unwrap();
        assert_eq!(
            one_ghz.deadline_after_nanoseconds(TickInstant::from_raw(u64::MAX), 1),
            Err(TimerConversionError::DeadlineOverflow {
                now: TickInstant::from_raw(u64::MAX),
                duration: TickDuration::from_raw(1),
            })
        );
    }

    #[test]
    fn control_decoding_ignores_reserved_bits() {
        let disabled = PhysicalTimerControl::from_raw(!0b111_u64);
        assert_eq!(disabled.raw(), 0);
        assert!(!disabled.enabled());
        assert!(!disabled.interrupt_masked());
        assert!(!disabled.condition_met());

        let active = PhysicalTimerControl::from_raw(u64::MAX);
        assert_eq!(active.raw(), 0b111);
        assert!(active.enabled());
        assert!(active.interrupt_masked());
        assert!(active.condition_met());
    }
}
