//! AArch64 CPU primitives.

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

pub mod asid;
pub mod exception;
pub mod gicv3;
pub mod kernel_page_table;
pub mod paging;
pub mod syscall;
pub mod timer;
pub mod user_page_table;

#[cfg(target_arch = "aarch64")]
mod user_entry;

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

/// Return whether ordinary IRQ exceptions are masked in the current PSTATE.
#[cfg(target_arch = "aarch64")]
pub fn irqs_masked() -> bool {
    const DAIF_IRQ_MASK: u64 = 1 << 7;

    let daif: u64;
    // SAFETY: reading `DAIF` has no memory side effects and is available at
    // EL1 in Phoenix's AArch64 execution environment.
    unsafe {
        asm!(
            "mrs {daif}, DAIF",
            daif = out(reg) daif,
            options(nomem, nostack, preserves_flags)
        );
    }
    daif & DAIF_IRQ_MASK != 0
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
