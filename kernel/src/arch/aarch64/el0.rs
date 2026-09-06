//! Static address-space setup for the opt-in first-EL0 conformance probe.

use core::ptr;

use crate::arch::aarch64::paging::{
    Descriptor, MappingAttributes, PagingError, TranslationIndices, TranslationLevel,
};
use crate::memory::{PAGE_SIZE, PageFrame, PhysAddr, VirtAddr};
use crate::user::{UserAddr, UserAddressError};

const KERNEL_VIRTUAL_OFFSET: usize = 0xffff_ff80_0000_0000;
const USER_CODE_BASE: usize = 0x0040_0000;
const USER_STACK_BASE: usize = 0x0080_1000;
const USER_STACK_TOP: usize = USER_STACK_BASE + PAGE_SIZE;

#[repr(C, align(4096))]
struct PageTable([u64; 512]);

#[unsafe(link_section = ".bss.user_probe_tables")]
static mut USER_L1: PageTable = PageTable([0; 512]);
#[unsafe(link_section = ".bss.user_probe_tables")]
static mut USER_L2: PageTable = PageTable([0; 512]);
#[unsafe(link_section = ".bss.user_probe_tables")]
static mut USER_CODE_L3: PageTable = PageTable([0; 512]);
#[unsafe(link_section = ".bss.user_probe_tables")]
static mut USER_STACK_L3: PageTable = PageTable([0; 512]);

unsafe extern "C" {
    static __user_probe_start: u8;
    static __user_probe_end: u8;
    static __user_probe_stack_bottom: u8;
    static __user_probe_stack_top: u8;
}

/// Failure while preparing the statically backed EL0 probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum El0ProbeError {
    /// A linker-provided section does not occupy exactly one aligned page.
    InvalidLinkedSection,
    /// A linked higher-half address cannot be converted to its physical load address.
    InvalidLinkedAddress,
    /// Page-table descriptor construction failed.
    Paging(PagingError),
    /// A fixed user virtual address violates the user-region contract.
    UserAddress(UserAddressError),
}

/// Fully populated static translation root plus EL0 entry state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedEl0Probe {
    root: PageFrame,
    entry: UserAddr,
    stack_pointer: UserAddr,
}

impl PreparedEl0Probe {
    /// Return the physical TTBR0 root frame.
    pub const fn root(self) -> PageFrame {
        self.root
    }

    /// Return the EL0 program entry.
    pub const fn entry(self) -> UserAddr {
        self.entry
    }

    /// Return the initial EL0 stack pointer.
    pub const fn stack_pointer(self) -> UserAddr {
        self.stack_pointer
    }
}

/// Clear and populate the static tables used only by the opt-in EL0 probe.
///
/// # Safety
///
/// Call exactly once on the boot CPU, before publishing the returned root to
/// `TTBR0_EL1`. No other code may access the four static table pages while
/// this function mutates them.
pub unsafe fn prepare_probe() -> Result<PreparedEl0Probe, El0ProbeError> {
    let l1 = &raw mut USER_L1;
    let l2 = &raw mut USER_L2;
    let code_l3 = &raw mut USER_CODE_L3;
    let stack_l3 = &raw mut USER_STACK_L3;
    for table in [l1, l2, code_l3, stack_l3] {
        // SAFETY: the caller guarantees unique boot-time ownership of all
        // table statics; each pointer names one valid `PageTable` object.
        unsafe {
            ptr::write_bytes(table, 0, 1);
        }
    }

    let code_start = &raw const __user_probe_start as usize;
    let code_end = &raw const __user_probe_end as usize;
    let stack_start = &raw const __user_probe_stack_bottom as usize;
    let stack_end = &raw const __user_probe_stack_top as usize;
    if code_start & (PAGE_SIZE - 1) != 0
        || stack_start & (PAGE_SIZE - 1) != 0
        || code_end.checked_sub(code_start) != Some(PAGE_SIZE)
        || stack_end.checked_sub(stack_start) != Some(PAGE_SIZE)
    {
        return Err(El0ProbeError::InvalidLinkedSection);
    }
    // SAFETY: the caller grants unique access to the linked probe resources,
    // and the layout checks above prove this is one writable stack page.
    unsafe {
        ptr::write_bytes(stack_start as *mut u8, 0, PAGE_SIZE);
    }

    let l1_frame = table_frame(l1)?;
    let l2_frame = table_frame(l2)?;
    let code_l3_frame = table_frame(code_l3)?;
    let stack_l3_frame = table_frame(stack_l3)?;
    let code_frame = linked_frame(code_start)?;
    let stack_frame = linked_frame(stack_start)?;
    let entry = UserAddr::new(USER_CODE_BASE).map_err(El0ProbeError::UserAddress)?;
    let stack_pointer = UserAddr::new(USER_STACK_TOP).map_err(El0ProbeError::UserAddress)?;
    let code_indices =
        TranslationIndices::new(VirtAddr::new(USER_CODE_BASE)).map_err(El0ProbeError::Paging)?;
    let stack_indices =
        TranslationIndices::new(VirtAddr::new(USER_STACK_BASE)).map_err(El0ProbeError::Paging)?;

    write_entry(
        l1,
        code_indices.l1(),
        Descriptor::table(l2_frame).map_err(El0ProbeError::Paging)?,
    );
    write_entry(
        l2,
        code_indices.l2(),
        Descriptor::table(code_l3_frame).map_err(El0ProbeError::Paging)?,
    );
    write_entry(
        l2,
        stack_indices.l2(),
        Descriptor::table(stack_l3_frame).map_err(El0ProbeError::Paging)?,
    );
    write_entry(
        code_l3,
        code_indices.l3(),
        Descriptor::leaf(
            TranslationLevel::L3,
            code_frame.start_address(),
            MappingAttributes::user_code(),
        )
        .map_err(El0ProbeError::Paging)?,
    );
    write_entry(
        stack_l3,
        stack_indices.l3(),
        Descriptor::leaf(
            TranslationLevel::L3,
            stack_frame.start_address(),
            MappingAttributes::user_data(),
        )
        .map_err(El0ProbeError::Paging)?,
    );

    Ok(PreparedEl0Probe {
        root: l1_frame,
        entry,
        stack_pointer,
    })
}

/// Replace the bootstrap `TTBR0_EL1` and enter the prepared program at EL0t.
///
/// # Safety
///
/// `probe` must have been returned by the only successful `prepare_probe`
/// call, its table frames must remain exclusively owned and mapped as normal
/// memory, `VBAR_EL1` and a valid EL1 stack must already be installed, and no
/// concurrent CPU may use ASID zero.
pub unsafe fn activate_and_enter(probe: PreparedEl0Probe) -> ! {
    // SAFETY: the caller establishes the static probe's ownership, mappings,
    // exception-vector, and single-ASID requirements.
    unsafe {
        super::user_entry::activate_and_enter(probe.root(), probe.entry(), probe.stack_pointer());
    }
}

fn table_frame(table: *const PageTable) -> Result<PageFrame, El0ProbeError> {
    linked_frame(table as usize)
}

fn linked_frame(virtual_address: usize) -> Result<PageFrame, El0ProbeError> {
    let physical_address = virtual_address
        .checked_sub(KERNEL_VIRTUAL_OFFSET)
        .ok_or(El0ProbeError::InvalidLinkedAddress)?;
    PageFrame::from_start(PhysAddr::new(physical_address))
        .map_err(|_| El0ProbeError::InvalidLinkedAddress)
}

fn write_entry(table: *mut PageTable, index: usize, descriptor: Descriptor) {
    debug_assert!(index < 512);
    // SAFETY: callers pass uniquely owned table pointers and architectural
    // indices in 0..512. Descriptor construction validates the written bits.
    unsafe {
        (*table).0[index] = descriptor.raw();
    }
}
