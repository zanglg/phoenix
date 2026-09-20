//! AArch64 CPU primitives.

#[cfg(target_arch = "aarch64")]
use core::arch::asm;
#[cfg(target_arch = "aarch64")]
use core::marker::PhantomData;

#[cfg(target_arch = "aarch64")]
use crate::sync::LocalIrqControl;

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

/// Current-CPU AArch64 ordinary-IRQ masking backend.
#[cfg(target_arch = "aarch64")]
#[derive(Default)]
pub struct LocalIrq {
    _private: (),
}

#[cfg(target_arch = "aarch64")]
impl LocalIrq {
    /// Construct a handle to the current CPU's `DAIF.I` state.
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

/// Saved `DAIF.I` state that must be restored on the acquiring CPU.
#[cfg(target_arch = "aarch64")]
#[must_use = "dropping this token directly leaves the captured CPU's IRQ state unrestored"]
pub struct IrqMaskState {
    was_masked: bool,
    _not_send_or_sync: PhantomData<*mut ()>,
}

#[cfg(target_arch = "aarch64")]
// SAFETY: the implementation reads the current CPU's exact prior `DAIF.I`
// state, masks only ordinary IRQ delivery before returning, and restores only
// that bit. The non-transferable state token and lock guard keep restoration on
// the acquiring execution context under Phoenix's no-migration critical-section
// contract.
unsafe impl LocalIrqControl for LocalIrq {
    type State = IrqMaskState;

    fn save_and_mask(&self) -> Self::State {
        const DAIF_IRQ_MASK: u64 = 1 << 7;

        let daif: u64;
        // SAFETY: reading DAIF and setting its IRQ mask are privileged
        // current-CPU operations valid at Phoenix EL1. Omitting `nomem` makes
        // this a compiler barrier around the local masking transition; `isb`
        // makes the changed interrupt-recognition context visible before the
        // caller attempts lock acquisition.
        unsafe {
            asm!(
                "mrs {daif}, DAIF",
                "msr DAIFSet, #2",
                "isb",
                daif = out(reg) daif,
                options(nostack, preserves_flags)
            );
        }
        IrqMaskState {
            was_masked: daif & DAIF_IRQ_MASK != 0,
            _not_send_or_sync: PhantomData,
        }
    }

    fn restore(&self, state: Self::State) {
        // SAFETY: `state` was produced on this execution context before the
        // matching lock acquisition. Guard drop publishes protected writes
        // before this instruction. Each branch changes only `DAIF.I`; `isb`
        // makes the restored recognition state visible before later code.
        unsafe {
            if state.was_masked {
                asm!("msr DAIFSet, #2", "isb", options(nostack, preserves_flags));
            } else {
                asm!("msr DAIFClr, #2", "isb", options(nostack, preserves_flags));
            }
        }
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
