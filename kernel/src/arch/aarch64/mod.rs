//! AArch64 CPU primitives.

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

pub mod paging;

/// Stop execution while allowing an event to wake the CPU transiently.
#[cfg(target_arch = "aarch64")]
pub fn halt() -> ! {
    loop {
        // SAFETY: `wfe` does not access memory or the stack. Interrupts remain
        // masked during early boot, so a wake event simply repeats the loop.
        unsafe {
            asm!("wfe", options(nomem, nostack, preserves_flags));
        }
    }
}
