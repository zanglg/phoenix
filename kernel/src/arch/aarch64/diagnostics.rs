//! Small privileged observations and controlled runtime validation entry points.

use super::timer::{CounterFrequency, CounterFrequencyError};
use crate::time::TickInstant;
use core::arch::asm;

/// Current ordered physical counter without claiming the timer comparator.
pub fn counter() -> TickInstant {
    let raw: u64;
    // SAFETY: Bootstrap permits EL1 physical counter reads. ISB orders this
    // observation; the counter is read-only and has no exclusive owner.
    unsafe {
        asm!("isb", "mrs {}, CNTPCT_EL0", out(reg) raw, options(nomem, nostack, preserves_flags));
    }
    TickInstant::from_raw(raw)
}

/// Validate the platform's read-only counter frequency.
pub fn frequency() -> Result<CounterFrequency, CounterFrequencyError> {
    let raw: u64;
    // SAFETY: CNTFRQ_EL0 is readable at EL1 and does not modify the timer.
    unsafe {
        asm!("mrs {}, CNTFRQ_EL0", out(reg) raw, options(nomem, nostack, preserves_flags));
    }
    CounterFrequency::new(raw)
}

/// Current EL, SP and vector base for boot-contract validation.
pub fn boot_state() -> (usize, usize, usize) {
    let (el, sp, vectors): (usize, usize, usize);
    // SAFETY: Read-only privileged observations on the boot CPU.
    unsafe {
        asm!("mrs {el}, CurrentEL", "mov {sp}, sp", "mrs {vectors}, VBAR_EL1",
        el = out(reg) el, sp = out(reg) sp, vectors = out(reg) vectors,
        options(nomem, nostack, preserves_flags));
    }
    (el >> 2, sp, vectors)
}

/// Observe the caller's frame chain. Forced frame pointers are a target policy.
#[inline(always)]
pub fn frame_pointer() -> usize {
    let frame;
    // SAFETY: Reading x29 has no memory or privileged side effects.
    unsafe { asm!("mov {}, x29", out(reg) frame, options(nomem, nostack, preserves_flags)) };
    frame
}

/// Allow ordinary IRQ entry on this CPU.
///
/// # Safety
/// All vectors, stacks and dispatch owners must be published; every shared
/// resource must obey IRQ exclusion. No saved-mask token or lock may be live.
pub unsafe fn enable_irqs() {
    // SAFETY: The caller establishes handler readiness and owns this transition.
    unsafe {
        asm!("msr DAIFClr, #2", "isb", options(nostack, preserves_flags));
    }
}

/// Prevent new local IRQ entry before retiring an interrupt runtime.
pub fn disable_irqs() {
    // SAFETY: Masking IRQ entry is valid at EL1; the compiler barrier and ISB
    // order the removal boundary before device or dispatcher retirement.
    unsafe {
        asm!("msr DAIFSet, #2", "isb", options(nostack, preserves_flags));
    }
}

/// Exit an explicitly semihosting-enabled emulator test.
///
/// # Safety
/// Only an opt-in test harness enabling AArch64 semihosting may call this.
pub unsafe fn test_exit(code: usize) -> ! {
    let parameters = [0x20026usize, code];
    // SAFETY: The two-word SYS_EXIT_EXTENDED block stays live through the trap.
    unsafe {
        asm!("hlt #0xf000", in("x0") 0x20usize, in("x1") parameters.as_ptr(), options(nostack));
    }
    super::halt()
}

unsafe extern "C" {
    /// Controlled assembly register round trip. Requires an installed BRK handler.
    pub fn phoenix_register_probe() -> usize;
    /// Address of the exact BRK accepted by the register-probe handler.
    pub static phoenix_register_probe_trap: u8;
    /// Perform one deliberately faulting load, store or branch.
    pub fn phoenix_fault_probe(address: usize, operation: usize);
    /// Recovery point after a controlled fault.
    pub static phoenix_fault_probe_resume: u8;
    /// Deliberate unhandled BRK for the fatal-report harness.
    pub fn phoenix_fatal_probe();
    /// Address accepted as the intentional fatal trap.
    pub static phoenix_fatal_probe_trap: u8;
    /// Call a leaf callback on a privately owned aligned kernel stack.
    pub fn phoenix_stack_probe(top: usize) -> usize;
}

// SAFETY: These leaf probes preserve the AAPCS64 caller state. Their unsafe
// extern interfaces require exclusive stacks and an exact recoverable handler.
core::arch::global_asm!(include_str!("diagnostics.S"));

/// Validate the complete BSS before any runtime singleton is initialized.
///
/// # Safety
/// Call only at the very beginning of single-CPU Rust entry, before any BSS
/// object has been accessed and while interrupts remain masked.
pub unsafe fn validate_boot_bss() {
    unsafe extern "C" {
        static __bss_start: u64;
        static __bss_end: u64;
    }
    let mut cursor = (&raw const __bss_start) as usize;
    let end = (&raw const __bss_end) as usize;
    assert!(cursor <= end && cursor.is_multiple_of(8) && end.is_multiple_of(8));
    while cursor < end {
        // SAFETY: Linker-owned BSS is mapped, aligned and exclusively observed
        // before singleton writes; no IRQ or other CPU can race this volatile read.
        assert_eq!(
            unsafe { (cursor as *const u64).read_volatile() },
            0,
            "BSS not zero"
        );
        cursor += 8;
    }
}
