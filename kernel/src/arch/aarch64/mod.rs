//! AArch64 CPU primitives.

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

pub mod exception;
pub mod paging;
pub mod syscall;

#[cfg(all(target_arch = "aarch64", feature = "el0-probe"))]
pub mod el0;

#[cfg(target_arch = "aarch64")]
unsafe extern "C" {
    static __exception_vectors: u8;
}

/// Install Phoenix's statically linked EL1 exception vector table.
///
/// # Safety
///
/// The caller must ensure that the table's linked virtual range is mapped as
/// readable executable normal memory and that no exception can observe an
/// incomplete stack or handler environment during installation.
#[cfg(target_arch = "aarch64")]
pub unsafe fn install_exception_vectors() {
    let table = &raw const __exception_vectors;
    // SAFETY: the caller establishes the mapping and exception-environment
    // contract. The linker and host inspection enforce 2 KiB alignment.
    unsafe {
        asm!(
            "msr VBAR_EL1, {table}",
            "isb",
            table = in(reg) table,
            options(nostack, preserves_flags)
        );
    }
}

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
