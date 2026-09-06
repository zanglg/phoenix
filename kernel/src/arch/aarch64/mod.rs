//! AArch64 CPU primitives.

use core::arch::asm;

/// Stop execution while allowing an event to wake the CPU transiently.
pub fn halt() -> ! {
    loop {
        // SAFETY: `wfe` does not access memory or the stack. Interrupts remain
        // masked during early boot, so a wake event simply repeats the loop.
        unsafe {
            asm!("wfe", options(nomem, nostack, preserves_flags));
        }
    }
}
