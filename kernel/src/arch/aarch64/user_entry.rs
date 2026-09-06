//! Ownership-gated AArch64 transition into a prepared EL0 address space.

use core::arch::asm;

use super::asid::{AddressSpaceId, ttbr0_value};
use crate::memory::PageFrame;
use crate::user::UserAddr;

const USER_PSTATE: usize = 0x3c0;

/// Replace TTBR0 with one private root and enter EL0t without returning.
///
/// This function is crate-private so a bare frame cannot bypass the public
/// `PreparedUserAddressSpace` ownership gate.
///
/// # Safety
///
/// `root` must name a complete, uniquely owned translation hierarchy whose
/// leaves remain owned and initialized. `entry` and `stack_pointer` must be
/// mapped with their documented executable and writable permissions. EL1
/// vectors and a valid kernel stack must be active, all table writes must be
/// visible to the current PE, and no concurrent PE may use `asid`.
pub(super) unsafe fn activate_and_enter(
    root: PageFrame,
    asid: AddressSpaceId,
    entry: UserAddr,
    stack_pointer: UserAddr,
) -> ! {
    let ttbr0 = ttbr0_value(root, asid).expect("a materialized TTBR0 root fits its address field");
    let entry = entry.as_usize();
    let stack = stack_pointer.as_usize();
    // SAFETY: the caller supplies a fully owned and initialized root, entry,
    // and stack. Barriers publish table writes before changing TTBR0 and make
    // the global stage-1 invalidation complete before the first EL0 fetch.
    unsafe {
        asm!(
            "dsb ishst",
            "msr TTBR0_EL1, {ttbr0}",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            "msr SP_EL0, {stack}",
            "msr ELR_EL1, {entry}",
            "msr SPSR_EL1, {pstate}",
            "eret",
            ttbr0 = in(reg) ttbr0,
            stack = in(reg) stack,
            entry = in(reg) entry,
            pstate = in(reg) USER_PSTATE,
            options(noreturn)
        );
    }
}
