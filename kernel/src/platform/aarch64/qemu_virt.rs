//! Early devices on QEMU's AArch64 `virt` machine.

pub mod interrupts;

use core::ptr::{self, read_volatile, write_volatile};

#[cfg(target_arch = "aarch64")]
use crate::arch::aarch64::gicv3::{
    CpuAffinity, GicRedistributorDiscoveryAccess, RedistributorSelectionError, select_redistributor,
};
use crate::arch::aarch64::gicv3::{GicV3MmioError, GicV3MmioLayout, SelectedRedistributor};
#[cfg(any(target_arch = "aarch64", test))]
use crate::arch::aarch64::kernel_page_table::DirectMapOverride;
use crate::arch::aarch64::kernel_page_table::{
    ActiveKernelTableMemory, KernelPageTableError, KernelPageTablePlan,
    MaterializedKernelPageTables,
};
#[cfg(any(target_arch = "aarch64", test))]
use crate::arch::aarch64::paging::TranslationLevel;
use crate::arch::aarch64::paging::{Descriptor, MappingAttributes};
use crate::arch::aarch64::user_page_table::TranslationTableMemory;
use crate::console::ByteSink;
#[cfg(target_arch = "aarch64")]
use crate::dtb::BootInfo;
use crate::dtb::{DeviceTree, Error as DeviceTreeError};
use crate::memory::{
    AddressRange, FrameRange, HeapBackingMemory, KernelStackMemory, KernelStackPool,
    KernelStackPoolError, ManagedFrameSet, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr,
};
use crate::process_image::ProcessImageMemory;
use crate::user_copy::{UserMemoryReader, UserMemoryWriter};

use self::interrupts::InterruptTopology;
#[cfg(target_arch = "aarch64")]
use self::interrupts::InterruptTopologyError;

/// Physical base of QEMU `virt`'s first PL011 UART.
pub const PL011_PHYSICAL_BASE: usize = 0x0900_0000;
/// Final higher-half virtual address of the first PL011 UART.
pub const PL011_VIRTUAL_BASE: usize = 0xffff_ff80_0900_0000;
/// Physical start of the RAM window reachable through the bootstrap alias.
pub const BOOTSTRAP_RAM_PHYSICAL_START: usize = 0x4000_0000;
/// Exclusive physical end of the RAM window reachable through the bootstrap alias.
pub const BOOTSTRAP_RAM_PHYSICAL_END: usize = 0x8000_0000;
/// Exclusive end of the temporary low Device-nGnRnE identity window.
pub const BOOTSTRAP_DEVICE_PHYSICAL_END: usize = 0x4000_0000;
/// Fixed physical-to-virtual offset of the QEMU `virt` kernel direct map.
pub const KERNEL_VIRTUAL_OFFSET: usize = 0xffff_ff80_0000_0000;
/// First byte of the platform-owned runtime kernel-stack virtual arena.
pub const KERNEL_STACK_ARENA_START: usize = 0xffff_ff90_0000_0000;
/// Number of guarded runtime-stack slots reserved by the initial platform policy.
pub const KERNEL_STACK_ARENA_SLOTS: usize = 16;
/// Number of usable 4 KiB pages in each initial runtime-stack slot.
pub const KERNEL_STACK_USABLE_PAGES: usize = 8;
/// Maximum number of page-table frames in the pinned single-region layout.
pub const FINAL_KERNEL_TABLE_CAPACITY: usize = 8;
/// Maximum number of leaf mappings in the pinned 512 MiB layout.
pub const FINAL_KERNEL_MAPPING_CAPACITY: usize = 1024;
const UART_DR: usize = 0x00;
const UART_FR: usize = 0x18;
const UART_FR_BUSY: u32 = 1 << 3;
const UART_FR_TXFF: u32 = 1 << 5;

/// Failure while reading GIC discovery registers through the boot device map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapDeviceAccessError {
    /// The requested access is not naturally aligned.
    MisalignedAccess {
        /// Physical register address.
        address: usize,
        /// Required byte alignment.
        alignment: usize,
    },
    /// Address arithmetic overflowed.
    AddressOverflow,
    /// The access is outside the temporary low device identity mapping.
    OutsideTemporaryDeviceWindow {
        /// Physical register address.
        address: usize,
        /// Access width in bytes.
        width: usize,
    },
}

/// Exclusive early-boot reader for GIC redistributor discovery registers.
#[cfg(target_arch = "aarch64")]
pub struct BootstrapGicDiscoveryAccess {
    _private: (),
}

#[cfg(target_arch = "aarch64")]
impl BootstrapGicDiscoveryAccess {
    /// Assert that the temporary low Device-nGnRnE identity map is active.
    ///
    /// # Safety
    ///
    /// The caller must run on the single boot CPU before replacing TTBR0 and
    /// must uniquely own GIC discovery access. The bootstrap mapping of
    /// `0..BOOTSTRAP_DEVICE_PHYSICAL_END` must remain active for this value's
    /// lifetime, and no concurrent observer may mutate redistributor topology.
    pub const unsafe fn assume_bootstrap_mapping() -> Self {
        Self { _private: () }
    }
}

#[cfg(target_arch = "aarch64")]
impl GicRedistributorDiscoveryAccess for BootstrapGicDiscoveryAccess {
    type Error = BootstrapDeviceAccessError;

    fn read_u32(&mut self, address: PhysAddr) -> Result<u32, Self::Error> {
        let address = checked_bootstrap_device_access(address, size_of::<u32>())? as *const u32;
        // SAFETY: construction proves the low Device identity mapping and
        // unique discovery access; validation proves a naturally aligned
        // complete register access inside that mapping.
        Ok(unsafe { read_volatile(address) })
    }

    fn read_u64(&mut self, address: PhysAddr) -> Result<u64, Self::Error> {
        let address = checked_bootstrap_device_access(address, size_of::<u64>())? as *const u64;
        // SAFETY: construction proves the low Device identity mapping and
        // unique discovery access; validation proves a naturally aligned
        // complete register access inside that mapping.
        Ok(unsafe { read_volatile(address) })
    }
}

#[cfg(any(target_arch = "aarch64", test))]
fn checked_bootstrap_device_access(
    address: PhysAddr,
    width: usize,
) -> Result<usize, BootstrapDeviceAccessError> {
    let address = address.as_usize();
    if !address.is_multiple_of(width) {
        return Err(BootstrapDeviceAccessError::MisalignedAccess {
            address,
            alignment: width,
        });
    }
    let end = address
        .checked_add(width)
        .ok_or(BootstrapDeviceAccessError::AddressOverflow)?;
    if end > BOOTSTRAP_DEVICE_PHYSICAL_END {
        return Err(BootstrapDeviceAccessError::OutsideTemporaryDeviceWindow { address, width });
    }
    Ok(address)
}

/// Polled PL011 transmitter used before the driver subsystem exists.
pub struct EarlyPl011 {
    base: usize,
}

impl EarlyPl011 {
    /// Construct the early console for QEMU `virt`.
    pub const fn new() -> Self {
        Self {
            base: PL011_VIRTUAL_BASE,
        }
    }

    /// Wait until every submitted byte has left the transmitter.
    pub fn flush(&mut self) {
        while self.flags() & UART_FR_BUSY != 0 {
            core::hint::spin_loop();
        }
    }

    fn flags(&self) -> u32 {
        let register = (self.base + UART_FR) as *const u32;
        // SAFETY: the platform contract maps the QEMU `virt` PL011 register
        // page at `PL011_VIRTUAL_BASE` as device memory before this is called.
        unsafe { read_volatile(register) }
    }
}

impl Default for EarlyPl011 {
    fn default() -> Self {
        Self::new()
    }
}

impl ByteSink for EarlyPl011 {
    fn write_byte(&mut self, byte: u8) {
        while self.flags() & UART_FR_TXFF != 0 {
            core::hint::spin_loop();
        }

        let register = (self.base + UART_DR) as *mut u32;
        // SAFETY: the platform contract maps the QEMU `virt` PL011 register
        // page at `PL011_VIRTUAL_BASE` as device memory before this is called.
        unsafe {
            write_volatile(register, u32::from(byte));
        }
    }
}

/// Final page-table plan for the pinned QEMU `virt` memory envelope.
pub type QemuVirtKernelPageTablePlan =
    KernelPageTablePlan<FINAL_KERNEL_TABLE_CAPACITY, FINAL_KERNEL_MAPPING_CAPACITY>;
/// Platform pool type for the initially reserved guarded runtime-stack arena.
pub type QemuVirtKernelStackPool =
    KernelStackPool<KERNEL_STACK_ARENA_SLOTS, KERNEL_STACK_USABLE_PAGES>;

/// Exact physical and final virtual ownership for the boot CPU's GICv3 path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QemuVirtGicV3Layout {
    distributor_physical: AddressRange<PhysAddr>,
    redistributor_physical: AddressRange<PhysAddr>,
    registers: GicV3MmioLayout,
}

impl QemuVirtGicV3Layout {
    /// Return the complete DTB-declared GICD physical range.
    pub const fn distributor_physical(self) -> AddressRange<PhysAddr> {
        self.distributor_physical
    }

    /// Return the selected boot CPU's exact 128 KiB RD/SGI physical pair.
    pub const fn redistributor_physical(self) -> AddressRange<PhysAddr> {
        self.redistributor_physical
    }

    /// Return the checked final virtual register ranges.
    pub const fn registers(self) -> GicV3MmioLayout {
        self.registers
    }
}

/// Failure while binding discovered GIC topology to final virtual ranges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QemuVirtGicV3LayoutError {
    /// The selected redistributor is not inside the DTB-declared region.
    RedistributorOutsideDiscoveredRegion,
    /// Physical-to-virtual address conversion overflowed.
    AddressOverflow,
    /// The architecture-side MMIO contract rejected the ranges.
    Mmio(GicV3MmioError),
}

/// Copied boot-CPU GIC discovery evidence retained beyond the borrowed DTB.
#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootGicV3Discovery {
    topology: InterruptTopology,
    affinity: CpuAffinity,
    selected: SelectedRedistributor,
    layout: QemuVirtGicV3Layout,
}

#[cfg(target_arch = "aarch64")]
impl BootGicV3Discovery {
    /// Return the validated DTB interrupt topology.
    pub const fn topology(self) -> InterruptTopology {
        self.topology
    }

    /// Return the current CPU affinity used during selection.
    pub const fn affinity(self) -> CpuAffinity {
        self.affinity
    }

    /// Return the exact selected redistributor evidence.
    pub const fn selected(self) -> SelectedRedistributor {
        self.selected
    }

    /// Return the final GIC device layout used by the TTBR1 builder.
    pub const fn layout(self) -> QemuVirtGicV3Layout {
        self.layout
    }
}

/// Failure while discovering and selecting the boot CPU GIC path.
#[cfg(target_arch = "aarch64")]
#[derive(Debug, Eq, PartialEq)]
pub enum BootGicV3DiscoveryError {
    /// The DTB GIC/timer topology is invalid.
    Topology(InterruptTopologyError),
    /// The current CPU redistributor could not be selected safely.
    Redistributor(RedistributorSelectionError<BootstrapDeviceAccessError>),
    /// The selected physical ranges cannot form the final MMIO layout.
    Layout(QemuVirtGicV3LayoutError),
}

/// Discover, affinity-select, and bind the boot CPU's GICv3 device ranges.
///
/// # Safety
///
/// The temporary low Device-nGnRnE identity window must still be active, the
/// caller must execute on the sole boot CPU with IRQs masked, and no other
/// observer may access or mutate the GIC redistributor discovery registers.
#[cfg(target_arch = "aarch64")]
pub unsafe fn discover_boot_gicv3(
    tree: DeviceTree<'_>,
) -> Result<BootGicV3Discovery, BootGicV3DiscoveryError> {
    let topology = interrupts::discover(tree).map_err(BootGicV3DiscoveryError::Topology)?;
    let affinity = CpuAffinity::current();
    // SAFETY: the caller establishes the low device mapping, single-CPU, and
    // exclusive discovery conditions required by the access capability.
    let mut access = unsafe { BootstrapGicDiscoveryAccess::assume_bootstrap_mapping() };
    let selected = select_redistributor(topology.redistributor(), affinity, &mut access)
        .map_err(BootGicV3DiscoveryError::Redistributor)?;
    let layout = final_gicv3_layout(topology, selected).map_err(BootGicV3DiscoveryError::Layout)?;
    Ok(BootGicV3Discovery {
        topology,
        affinity,
        selected,
        layout,
    })
}

/// Bind discovered topology and one affinity-selected redistributor to TTBR1.
pub fn final_gicv3_layout(
    topology: InterruptTopology,
    selected: SelectedRedistributor,
) -> Result<QemuVirtGicV3Layout, QemuVirtGicV3LayoutError> {
    let distributor_physical = topology.distributor();
    let redistributor_physical = selected.range();
    if !topology
        .redistributor()
        .contains_range(redistributor_physical)
    {
        return Err(QemuVirtGicV3LayoutError::RedistributorOutsideDiscoveredRegion);
    }
    let distributor_virtual = final_device_virtual_range(distributor_physical)?;
    let redistributor_virtual = final_device_virtual_range(redistributor_physical)?;
    let registers = GicV3MmioLayout::new(distributor_virtual, redistributor_virtual)
        .map_err(QemuVirtGicV3LayoutError::Mmio)?;
    Ok(QemuVirtGicV3Layout {
        distributor_physical,
        redistributor_physical,
        registers,
    })
}

fn final_device_virtual_range(
    physical: AddressRange<PhysAddr>,
) -> Result<AddressRange<VirtAddr>, QemuVirtGicV3LayoutError> {
    let start = physical
        .start()
        .as_usize()
        .checked_add(KERNEL_VIRTUAL_OFFSET)
        .ok_or(QemuVirtGicV3LayoutError::AddressOverflow)?;
    let end = physical
        .end()
        .as_usize()
        .checked_add(KERNEL_VIRTUAL_OFFSET)
        .ok_or(QemuVirtGicV3LayoutError::AddressOverflow)?;
    AddressRange::new(VirtAddr::new(start), VirtAddr::new(end))
        .map_err(|_| QemuVirtGicV3LayoutError::AddressOverflow)
}

/// Failure while deriving the final QEMU `virt` higher-half address space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalKernelMapError {
    /// A validated memory entry could not be decoded on a later traversal.
    DeviceTree(DeviceTreeError),
    /// A linked kernel section is empty, unordered, outside the image, or unaligned.
    InvalidKernelLayout,
    /// A DTB memory region is outside the RAM window reachable during bootstrap.
    RamOutsideBootstrap {
        /// Rejected physical start.
        start: usize,
        /// Rejected exclusive physical end.
        end: usize,
    },
    /// No complete RAM frame was present in the DTB memory nodes.
    MissingRam,
    /// A required kernel section is absent from the resulting direct map.
    MissingKernelSectionMapping,
    /// A required GIC device page is absent or mapped with wrong attributes.
    MissingGicDeviceMapping,
    /// The fixed runtime-stack arena could not be represented.
    InvalidRuntimeStackArena(KernelStackPoolError),
    /// A final mapping unexpectedly occupies the reserved runtime-stack arena.
    RuntimeStackArenaMapped {
        /// First conflicting virtual page.
        address: usize,
    },
    /// The fixed table topology or mapping metadata cannot represent the layout.
    PageTable(KernelPageTableError),
}

/// Construct the sole owner for the QEMU `virt` runtime-stack virtual arena.
pub fn runtime_kernel_stack_pool() -> Result<QemuVirtKernelStackPool, KernelStackPoolError> {
    QemuVirtKernelStackPool::new(VirtAddr::new(KERNEL_STACK_ARENA_START))
}

/// Failure while proving allocator-managed RAM is present in a final TTBR1 plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalDirectMapLayoutError {
    /// The allocator has no managed frames to authorize.
    NoManagedFrames,
    /// Physical or virtual range arithmetic overflowed.
    AddressOverflow,
    /// A managed frame has no final direct-map translation.
    MissingMapping {
        /// First frame number not covered by a valid translation.
        frame_number: usize,
    },
    /// A translation did not preserve the fixed physical-to-virtual offset.
    WrongPhysicalAddress {
        /// Frame whose translation was rejected.
        frame_number: usize,
        /// Physical address returned by the offline translation model.
        translated_address: usize,
    },
    /// A managed frame is not mapped as EL1 read/write, non-executable normal memory.
    WrongAttributes {
        /// Frame whose translation was rejected.
        frame_number: usize,
        /// Attributes returned by the offline translation model.
        attributes: MappingAttributes,
    },
    /// The offline translation model rejected an address.
    PageTable(KernelPageTableError),
}

/// Failure while translating allocator-managed bytes through the active final direct map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalDirectMapAccessError {
    /// A frame is outside the reservation-filtered allocator domain.
    UnmanagedFrame {
        /// Rejected physical frame start.
        address: usize,
    },
    /// A contiguous range is not wholly inside one managed region.
    UnmanagedRange {
        /// Inclusive first frame number.
        start_frame: usize,
        /// Exclusive final frame number.
        end_frame: usize,
    },
    /// A requested byte range extends beyond one base page.
    AccessOutOfBounds {
        /// Offset within the frame.
        offset: usize,
        /// Requested byte count.
        length: usize,
    },
    /// Physical or virtual address arithmetic overflowed.
    AddressOverflow,
}

/// Offline proof that every allocator-managed frame has the final RAM mapping.
///
/// This copyable value authorizes address translation only. It does not prove
/// that a caller owns any currently allocated frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinalDirectMapLayout<const RANGES: usize> {
    managed: ManagedFrameSet<RANGES>,
}

impl<const RANGES: usize> FinalDirectMapLayout<RANGES> {
    /// Return the reservation-filtered frame domain covered by this proof.
    pub const fn managed_frames(self) -> ManagedFrameSet<RANGES> {
        self.managed
    }

    /// Translate a checked range within one managed frame.
    pub fn translated_address(
        self,
        frame: PageFrame,
        offset: usize,
        length: usize,
    ) -> Result<usize, FinalDirectMapAccessError> {
        if !self.managed.contains(frame) {
            return Err(FinalDirectMapAccessError::UnmanagedFrame {
                address: frame.start_address().as_usize(),
            });
        }
        let end = offset
            .checked_add(length)
            .ok_or(FinalDirectMapAccessError::AddressOverflow)?;
        if offset > PAGE_SIZE || end > PAGE_SIZE {
            return Err(FinalDirectMapAccessError::AccessOutOfBounds { offset, length });
        }
        frame
            .start_address()
            .as_usize()
            .checked_add(offset)
            .and_then(|physical| physical.checked_add(KERNEL_VIRTUAL_OFFSET))
            .ok_or(FinalDirectMapAccessError::AddressOverflow)
    }

    /// Return the virtual start of one wholly managed contiguous range.
    pub fn range_virtual_start(
        self,
        frames: FrameRange,
    ) -> Result<VirtAddr, FinalDirectMapAccessError> {
        if !self.managed.contains_range(frames) {
            return Err(FinalDirectMapAccessError::UnmanagedRange {
                start_frame: frames.start_frame_number(),
                end_frame: frames.end_frame_number(),
            });
        }
        let physical = frames
            .start_frame_number()
            .checked_mul(PAGE_SIZE)
            .ok_or(FinalDirectMapAccessError::AddressOverflow)?;
        let virtual_start = physical
            .checked_add(KERNEL_VIRTUAL_OFFSET)
            .ok_or(FinalDirectMapAccessError::AddressOverflow)?;
        Ok(VirtAddr::new(virtual_start))
    }
}

/// RAM access capability tied to a permanently retained active TTBR1 owner.
pub struct FinalDirectMapMemory<'a, const TABLES: usize, const RANGES: usize> {
    _tables: &'a MaterializedKernelPageTables<TABLES>,
    layout: FinalDirectMapLayout<RANGES>,
}

impl<'a, const TABLES: usize, const RANGES: usize> FinalDirectMapMemory<'a, TABLES, RANGES> {
    /// Bind an offline direct-map proof to the exact retained active hierarchy.
    ///
    /// # Safety
    ///
    /// `tables` must have been materialized from the plan that produced
    /// `layout`, must already be active in `TTBR1_EL1`, and must outlive this
    /// capability. The caller must separately prove unique ownership before
    /// mutating any accepted frame. No concurrent CPU may access those bytes.
    pub const unsafe fn assume_active(
        tables: &'a MaterializedKernelPageTables<TABLES>,
        layout: FinalDirectMapLayout<RANGES>,
    ) -> Self {
        Self {
            _tables: tables,
            layout,
        }
    }

    /// Copy bytes from one caller-owned managed frame.
    pub fn read_frame(
        &self,
        frame: PageFrame,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), FinalDirectMapAccessError> {
        let source = self
            .layout
            .translated_address(frame, offset, output.len())? as *const u8;
        if output.is_empty() {
            return Ok(());
        }
        // SAFETY: capability construction proves the final translation and
        // the caller-facing contract requires readable ownership. Range
        // validation keeps the copy inside one mapped frame.
        unsafe {
            ptr::copy(source, output.as_mut_ptr(), output.len());
        }
        Ok(())
    }

    fn write_frame_bytes(
        &mut self,
        frame: PageFrame,
        offset: usize,
        input: &[u8],
    ) -> Result<(), FinalDirectMapAccessError> {
        let destination = self.layout.translated_address(frame, offset, input.len())? as *mut u8;
        if input.is_empty() {
            return Ok(());
        }
        // SAFETY: capability construction proves the final writable mapping;
        // the caller-facing trait contracts require unique frame ownership,
        // and validation keeps the copy inside one frame.
        unsafe {
            ptr::copy(input.as_ptr(), destination, input.len());
        }
        Ok(())
    }

    fn clear_owned_frame(&mut self, frame: PageFrame) -> Result<(), FinalDirectMapAccessError> {
        let destination = self.layout.translated_address(frame, 0, PAGE_SIZE)? as *mut u8;
        // SAFETY: capability construction proves the complete writable final
        // mapping, while each implementing trait requires private ownership.
        unsafe {
            ptr::write_bytes(destination, 0, PAGE_SIZE);
        }
        Ok(())
    }
}

/// Page-aligned physical boundaries of the linked kernel permission domains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinalKernelImageLayout {
    image: AddressRange<PhysAddr>,
    text: FrameRange,
    rodata: FrameRange,
    data: FrameRange,
    boot_stack_guard: FrameRange,
}

impl FinalKernelImageLayout {
    /// Return the complete loaded kernel byte range.
    pub const fn image(self) -> AddressRange<PhysAddr> {
        self.image
    }

    /// Return executable, read-only kernel frames.
    pub const fn text(self) -> FrameRange {
        self.text
    }

    /// Return non-executable, read-only kernel frames.
    pub const fn rodata(self) -> FrameRange {
        self.rodata
    }

    /// Return non-executable, writable kernel frames.
    pub const fn data(self) -> FrameRange {
        self.data
    }

    /// Return the physical frame omitted below the temporary boot stack.
    pub const fn boot_stack_guard(self) -> FrameRange {
        self.boot_stack_guard
    }
}

/// Build the final permission-separated TTBR1 plan from validated boot data.
#[cfg(target_arch = "aarch64")]
pub fn final_kernel_page_table_plan(
    info: BootInfo<'_>,
    gic: QemuVirtGicV3Layout,
) -> Result<QemuVirtKernelPageTablePlan, FinalKernelMapError> {
    let layout = kernel_image_layout()?;
    final_kernel_page_table_plan_from_regions(
        layout,
        info.memory_regions()
            .map(|region| region.map_err(FinalKernelMapError::DeviceTree)),
        gic,
    )
}

#[cfg(any(target_arch = "aarch64", test))]
fn final_kernel_page_table_plan_from_regions<I>(
    layout: FinalKernelImageLayout,
    regions: I,
    gic: QemuVirtGicV3Layout,
) -> Result<QemuVirtKernelPageTablePlan, FinalKernelMapError>
where
    I: IntoIterator<Item = Result<AddressRange<PhysAddr>, FinalKernelMapError>>,
{
    let overrides = [
        DirectMapOverride::new(layout.text, MappingAttributes::kernel_code())
            .map_err(FinalKernelMapError::PageTable)?,
        DirectMapOverride::new(layout.rodata, MappingAttributes::kernel_rodata())
            .map_err(FinalKernelMapError::PageTable)?,
        DirectMapOverride::unmapped(layout.boot_stack_guard)
            .map_err(FinalKernelMapError::PageTable)?,
    ];
    let mut plan = QemuVirtKernelPageTablePlan::new().map_err(FinalKernelMapError::PageTable)?;
    let mut mapped_ram = false;
    for region in regions {
        let region = region?;
        if region.is_empty() {
            continue;
        }
        let start = region.start().as_usize();
        let end = region.end().as_usize();
        if start < BOOTSTRAP_RAM_PHYSICAL_START || end > BOOTSTRAP_RAM_PHYSICAL_END {
            return Err(FinalKernelMapError::RamOutsideBootstrap { start, end });
        }
        let frames = FrameRange::from_usable_bytes(region);
        if frames.is_empty() {
            continue;
        }
        plan.map_direct_frames(
            frames,
            KERNEL_VIRTUAL_OFFSET,
            MappingAttributes::kernel_data(),
            &overrides,
        )
        .map_err(FinalKernelMapError::PageTable)?;
        mapped_ram = true;
    }
    if !mapped_ram {
        return Err(FinalKernelMapError::MissingRam);
    }

    plan = map_final_device_range(plan, gic.distributor_physical, gic.registers.distributor())?;
    plan = map_final_device_range(
        plan,
        gic.redistributor_physical,
        gic.registers.redistributor(),
    )?;
    plan = plan
        .with_mapping(
            VirtAddr::new(PL011_VIRTUAL_BASE),
            PhysAddr::new(PL011_PHYSICAL_BASE),
            TranslationLevel::L3,
            MappingAttributes::kernel_device(),
        )
        .map_err(FinalKernelMapError::PageTable)?;
    validate_critical_mappings(&plan, layout, gic)?;
    validate_runtime_stack_arena(&plan)?;
    let stack_pool =
        runtime_kernel_stack_pool().map_err(FinalKernelMapError::InvalidRuntimeStackArena)?;
    let stack_arena = stack_pool.arena();
    plan = plan
        .with_l3_range_reservation(stack_arena.start(), stack_arena.len())
        .map_err(FinalKernelMapError::PageTable)?;
    validate_runtime_stack_arena(&plan)?;
    Ok(plan)
}

#[cfg(any(target_arch = "aarch64", test))]
fn map_final_device_range(
    mut plan: QemuVirtKernelPageTablePlan,
    physical: AddressRange<PhysAddr>,
    virtual_range: AddressRange<VirtAddr>,
) -> Result<QemuVirtKernelPageTablePlan, FinalKernelMapError> {
    debug_assert_eq!(physical.len(), virtual_range.len());
    let mut offset = 0_usize;
    while offset < physical.len() {
        let virtual_address = virtual_range.start().as_usize().checked_add(offset).ok_or(
            FinalKernelMapError::PageTable(KernelPageTableError::Paging(
                crate::arch::aarch64::paging::PagingError::AddressOverflow,
            )),
        )?;
        let physical_address = physical.start().as_usize().checked_add(offset).ok_or(
            FinalKernelMapError::PageTable(KernelPageTableError::Paging(
                crate::arch::aarch64::paging::PagingError::AddressOverflow,
            )),
        )?;
        plan = plan
            .with_mapping(
                VirtAddr::new(virtual_address),
                PhysAddr::new(physical_address),
                TranslationLevel::L3,
                MappingAttributes::kernel_device(),
            )
            .map_err(FinalKernelMapError::PageTable)?;
        offset = offset
            .checked_add(PAGE_SIZE)
            .ok_or(FinalKernelMapError::PageTable(
                KernelPageTableError::Paging(
                    crate::arch::aarch64::paging::PagingError::AddressOverflow,
                ),
            ))?;
    }
    Ok(plan)
}

#[cfg(any(target_arch = "aarch64", test))]
fn validate_runtime_stack_arena(
    plan: &QemuVirtKernelPageTablePlan,
) -> Result<(), FinalKernelMapError> {
    let pool =
        runtime_kernel_stack_pool().map_err(FinalKernelMapError::InvalidRuntimeStackArena)?;
    let arena = pool.arena();
    let mut address = arena.start().as_usize();
    while address < arena.end().as_usize() {
        if plan
            .translate(VirtAddr::new(address))
            .map_err(KernelPageTableError::Paging)
            .map_err(FinalKernelMapError::PageTable)?
            .is_some()
        {
            return Err(FinalKernelMapError::RuntimeStackArenaMapped { address });
        }
        address =
            address
                .checked_add(PAGE_SIZE)
                .ok_or(FinalKernelMapError::InvalidRuntimeStackArena(
                    KernelStackPoolError::AddressOverflow,
                ))?;
    }
    Ok(())
}

#[cfg(any(target_arch = "aarch64", test))]
fn validate_critical_mappings(
    plan: &QemuVirtKernelPageTablePlan,
    layout: FinalKernelImageLayout,
    gic: QemuVirtGicV3Layout,
) -> Result<(), FinalKernelMapError> {
    for (frames, attributes) in [
        (layout.text, MappingAttributes::kernel_code()),
        (layout.rodata, MappingAttributes::kernel_rodata()),
        (layout.data, MappingAttributes::kernel_data()),
    ] {
        for frame_number in [frames.start_frame_number(), frames.end_frame_number() - 1] {
            let physical = frame_number
                .checked_mul(PAGE_SIZE)
                .ok_or(FinalKernelMapError::InvalidKernelLayout)?;
            let virtual_address = physical
                .checked_add(KERNEL_VIRTUAL_OFFSET)
                .ok_or(FinalKernelMapError::InvalidKernelLayout)?;
            let translation = plan
                .translate(VirtAddr::new(virtual_address))
                .map_err(KernelPageTableError::Paging)
                .map_err(FinalKernelMapError::PageTable)?
                .ok_or(FinalKernelMapError::MissingKernelSectionMapping)?;
            if translation.physical_address != PhysAddr::new(physical)
                || translation.attributes != attributes
            {
                return Err(FinalKernelMapError::MissingKernelSectionMapping);
            }
        }
    }
    let guard_physical = layout
        .boot_stack_guard
        .start_frame_number()
        .checked_mul(PAGE_SIZE)
        .ok_or(FinalKernelMapError::InvalidKernelLayout)?;
    let guard_virtual = guard_physical
        .checked_add(KERNEL_VIRTUAL_OFFSET)
        .ok_or(FinalKernelMapError::InvalidKernelLayout)?;
    if plan
        .translate(VirtAddr::new(guard_virtual))
        .map_err(KernelPageTableError::Paging)
        .map_err(FinalKernelMapError::PageTable)?
        .is_some()
    {
        return Err(FinalKernelMapError::MissingKernelSectionMapping);
    }
    let uart = plan
        .translate(VirtAddr::new(PL011_VIRTUAL_BASE))
        .map_err(KernelPageTableError::Paging)
        .map_err(FinalKernelMapError::PageTable)?
        .ok_or(FinalKernelMapError::MissingKernelSectionMapping)?;
    if uart.physical_address != PhysAddr::new(PL011_PHYSICAL_BASE)
        || uart.attributes != MappingAttributes::kernel_device()
        || uart.level != TranslationLevel::L3
    {
        return Err(FinalKernelMapError::MissingKernelSectionMapping);
    }
    validate_gic_device_mapping(plan, gic.distributor_physical, gic.registers.distributor())?;
    validate_gic_device_mapping(
        plan,
        gic.redistributor_physical,
        gic.registers.redistributor(),
    )?;
    Ok(())
}

#[cfg(any(target_arch = "aarch64", test))]
fn validate_gic_device_mapping(
    plan: &QemuVirtKernelPageTablePlan,
    physical: AddressRange<PhysAddr>,
    virtual_range: AddressRange<VirtAddr>,
) -> Result<(), FinalKernelMapError> {
    let mut offset = 0_usize;
    while offset < physical.len() {
        let virtual_address = virtual_range
            .start()
            .as_usize()
            .checked_add(offset)
            .ok_or(FinalKernelMapError::MissingGicDeviceMapping)?;
        let expected_physical = physical
            .start()
            .as_usize()
            .checked_add(offset)
            .ok_or(FinalKernelMapError::MissingGicDeviceMapping)?;
        let translation = plan
            .translate(VirtAddr::new(virtual_address))
            .map_err(KernelPageTableError::Paging)
            .map_err(FinalKernelMapError::PageTable)?
            .ok_or(FinalKernelMapError::MissingGicDeviceMapping)?;
        if translation.physical_address != PhysAddr::new(expected_physical)
            || translation.attributes != MappingAttributes::kernel_device()
            || translation.level != TranslationLevel::L3
        {
            return Err(FinalKernelMapError::MissingGicDeviceMapping);
        }
        offset = offset
            .checked_add(PAGE_SIZE)
            .ok_or(FinalKernelMapError::MissingGicDeviceMapping)?;
    }
    Ok(())
}

/// Prove that every allocator-managed frame is present in the final RAM direct map.
#[cfg(any(target_arch = "aarch64", test))]
pub fn final_direct_map_layout<const RANGES: usize>(
    plan: &QemuVirtKernelPageTablePlan,
    managed: ManagedFrameSet<RANGES>,
) -> Result<FinalDirectMapLayout<RANGES>, FinalDirectMapLayoutError> {
    if managed.ranges().is_empty() {
        return Err(FinalDirectMapLayoutError::NoManagedFrames);
    }
    for range in managed.ranges().iter().copied() {
        let mut frame_number = range.start_frame_number();
        while frame_number < range.end_frame_number() {
            let physical = frame_number
                .checked_mul(PAGE_SIZE)
                .ok_or(FinalDirectMapLayoutError::AddressOverflow)?;
            let virtual_address = physical
                .checked_add(KERNEL_VIRTUAL_OFFSET)
                .ok_or(FinalDirectMapLayoutError::AddressOverflow)?;
            let translation = plan
                .translate(VirtAddr::new(virtual_address))
                .map_err(KernelPageTableError::Paging)
                .map_err(FinalDirectMapLayoutError::PageTable)?
                .ok_or(FinalDirectMapLayoutError::MissingMapping { frame_number })?;
            if translation.physical_address != PhysAddr::new(physical) {
                return Err(FinalDirectMapLayoutError::WrongPhysicalAddress {
                    frame_number,
                    translated_address: translation.physical_address.as_usize(),
                });
            }
            if translation.attributes != MappingAttributes::kernel_data() {
                return Err(FinalDirectMapLayoutError::WrongAttributes {
                    frame_number,
                    attributes: translation.attributes,
                });
            }

            let leaf_size = translation.level.mapping_size();
            let bytes_to_leaf_end = leaf_size - virtual_address % leaf_size;
            let frames_to_leaf_end = bytes_to_leaf_end / PAGE_SIZE;
            let remaining = range.end_frame_number() - frame_number;
            frame_number = frame_number
                .checked_add(frames_to_leaf_end.min(remaining))
                .ok_or(FinalDirectMapLayoutError::AddressOverflow)?;
        }
    }
    Ok(FinalDirectMapLayout { managed })
}

/// Failure while accessing a private RAM frame through the temporary TTBR1 alias.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapMemoryError {
    /// The frame is not completely covered by the temporary 1 GiB RAM block.
    OutsideTemporaryRam {
        /// Rejected physical frame start.
        address: usize,
    },
    /// A requested byte range extends beyond one 4 KiB frame.
    AccessOutOfBounds {
        /// Byte offset within the frame.
        offset: usize,
        /// Requested byte count.
        length: usize,
    },
    /// Physical-to-virtual address conversion overflowed.
    AddressOverflow,
    /// The linked kernel bounds do not form a non-empty physical range.
    InvalidKernelImage,
    /// The bytes at the boot argument do not contain a valid device tree.
    DeviceTree(DeviceTreeError),
}

/// Checked access to private RAM through the bootstrap higher-half block mapping.
///
/// This handle is deliberately platform- and bootstrap-specific. It must be
/// replaced by the final physical-memory mapping before the coarse TTBR1 block
/// is retired.
pub struct BootstrapPhysicalMemory {
    _private: (),
}

impl Drop for BootstrapPhysicalMemory {
    fn drop(&mut self) {
        // This explicit destructor makes capability revocation visible at the
        // final TTBR1 publication boundary even though no storage is released.
    }
}

impl BootstrapPhysicalMemory {
    /// Assert that the QEMU `virt` bootstrap higher-half RAM alias is active.
    ///
    /// # Safety
    ///
    /// The caller must ensure the current EL1 translation regime maps physical
    /// `0x40000000..0x80000000` at `KERNEL_VIRTUAL_OFFSET` as writable normal
    /// memory for the entire lifetime of this handle. Every supplied frame must
    /// be privately owned, and no concurrent CPU may access it while a trait
    /// method mutates it.
    pub const unsafe fn assume_bootstrap_mapping() -> Self {
        Self { _private: () }
    }

    /// Validate and borrow the DTB passed in the physical boot argument.
    ///
    /// The returned borrow cannot outlive this handle. Its physical pages must
    /// also remain reserved until every derived borrow has ended.
    pub fn device_tree(
        &self,
        physical_start: PhysAddr,
    ) -> Result<DeviceTree<'_>, BootstrapMemoryError> {
        const HEADER_SIZE: usize = 40;
        let header_address = Self::translated_physical_range(physical_start, HEADER_SIZE)?;
        // SAFETY: the constructor guarantees the temporary high RAM mapping;
        // range validation proves all header bytes are readable through it.
        let header =
            unsafe { core::slice::from_raw_parts(header_address as *const u8, HEADER_SIZE) };
        let total_size = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let blob_address = Self::translated_physical_range(physical_start, total_size)?;
        // SAFETY: the complete declared byte range was checked inside the
        // temporary readable RAM alias and its lifetime is tied to `self`.
        let blob = unsafe { core::slice::from_raw_parts(blob_address as *const u8, total_size) };
        DeviceTree::from_bytes(blob).map_err(BootstrapMemoryError::DeviceTree)
    }

    /// Copy bytes from one privately owned frame into a caller buffer.
    pub fn read_frame(
        &self,
        frame: PageFrame,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), BootstrapMemoryError> {
        // SAFETY: construction guarantees that every frame in the complete
        // bootstrap window is mapped as readable normal memory.
        unsafe { read_direct_mapped_frame(frame, offset, output) }
    }

    fn translated_address(
        frame: PageFrame,
        offset: usize,
        length: usize,
    ) -> Result<usize, BootstrapMemoryError> {
        let frame_start = frame.start_address().as_usize();
        if !(BOOTSTRAP_RAM_PHYSICAL_START..=BOOTSTRAP_RAM_PHYSICAL_END - PAGE_SIZE)
            .contains(&frame_start)
        {
            return Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: frame_start,
            });
        }
        let end = offset
            .checked_add(length)
            .ok_or(BootstrapMemoryError::AddressOverflow)?;
        if offset > PAGE_SIZE || end > PAGE_SIZE {
            return Err(BootstrapMemoryError::AccessOutOfBounds { offset, length });
        }
        frame_start
            .checked_add(offset)
            .and_then(|physical| physical.checked_add(KERNEL_VIRTUAL_OFFSET))
            .ok_or(BootstrapMemoryError::AddressOverflow)
    }

    fn translated_physical_range(
        physical_start: PhysAddr,
        length: usize,
    ) -> Result<usize, BootstrapMemoryError> {
        let start = physical_start.as_usize();
        let end = start
            .checked_add(length)
            .ok_or(BootstrapMemoryError::AddressOverflow)?;
        if start < BOOTSTRAP_RAM_PHYSICAL_START || end > BOOTSTRAP_RAM_PHYSICAL_END {
            return Err(BootstrapMemoryError::OutsideTemporaryRam { address: start });
        }
        start
            .checked_add(KERNEL_VIRTUAL_OFFSET)
            .ok_or(BootstrapMemoryError::AddressOverflow)
    }
}

/// Copy bytes from one frame through the QEMU kernel's fixed higher-half offset.
///
/// # Safety
///
/// The complete frame must currently be mapped as readable normal memory at
/// `frame.start_address() + KERNEL_VIRTUAL_OFFSET`. Its contents must be valid
/// to read, and no concurrent mutation may race with this copy.
pub unsafe fn read_direct_mapped_frame(
    frame: PageFrame,
    offset: usize,
    output: &mut [u8],
) -> Result<(), BootstrapMemoryError> {
    let source =
        BootstrapPhysicalMemory::translated_address(frame, offset, output.len())? as *const u8;
    if output.is_empty() {
        return Ok(());
    }
    // SAFETY: the caller guarantees the validated source frame is mapped and
    // readable; the mutable destination slice is valid. `ptr::copy` permits
    // defensive overlap with a destination already inside the direct map.
    unsafe {
        ptr::copy(source, output.as_mut_ptr(), output.len());
    }
    Ok(())
}

/// Copy bytes into one frame through the QEMU kernel's fixed higher-half offset.
///
/// # Safety
///
/// The complete frame must currently be mapped as writable normal memory at
/// `frame.start_address() + KERNEL_VIRTUAL_OFFSET`. The caller must uniquely
/// own its contents for the duration of this copy.
pub unsafe fn write_direct_mapped_frame(
    frame: PageFrame,
    offset: usize,
    input: &[u8],
) -> Result<(), BootstrapMemoryError> {
    let destination =
        BootstrapPhysicalMemory::translated_address(frame, offset, input.len())? as *mut u8;
    if input.is_empty() {
        return Ok(());
    }
    // SAFETY: the caller guarantees unique writable mapped destination bytes;
    // range validation keeps the operation within one frame. `ptr::copy`
    // permits overlap with a source already inside the direct map.
    unsafe {
        ptr::copy(input.as_ptr(), destination, input.len());
    }
    Ok(())
}

#[cfg(target_arch = "aarch64")]
unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __boot_stack_guard_start: u8;
    static __boot_stack_guard_end: u8;
}

/// Return the complete physical range occupied by the linked kernel image.
#[cfg(target_arch = "aarch64")]
pub fn kernel_physical_range() -> Result<AddressRange<PhysAddr>, BootstrapMemoryError> {
    physical_kernel_range(
        &raw const __kernel_start as usize,
        &raw const __kernel_end as usize,
    )
}

/// Return the page-aligned physical permission domains of the linked image.
#[cfg(target_arch = "aarch64")]
pub fn kernel_image_layout() -> Result<FinalKernelImageLayout, FinalKernelMapError> {
    linked_kernel_image_layout([
        &raw const __kernel_start as usize,
        &raw const __kernel_end as usize,
        &raw const __text_start as usize,
        &raw const __text_end as usize,
        &raw const __rodata_start as usize,
        &raw const __rodata_end as usize,
        &raw const __data_start as usize,
        &raw const __data_end as usize,
        &raw const __boot_stack_guard_start as usize,
        &raw const __boot_stack_guard_end as usize,
    ])
}

#[cfg(any(target_arch = "aarch64", test))]
fn linked_kernel_image_layout(
    bounds: [usize; 10],
) -> Result<FinalKernelImageLayout, FinalKernelMapError> {
    let [
        kernel_start,
        kernel_end,
        text_start,
        text_end,
        rodata_start,
        rodata_end,
        data_start,
        data_end,
        boot_stack_guard_start,
        boot_stack_guard_end,
    ] = bounds;
    if kernel_start != text_start
        || !(text_start < text_end
            && text_end <= rodata_start
            && rodata_start < rodata_end
            && rodata_end <= data_start
            && data_start < data_end
            && data_end == kernel_end
            && data_start < boot_stack_guard_start
            && boot_stack_guard_start < boot_stack_guard_end
            && boot_stack_guard_end < data_end)
        || bounds.iter().any(|bound| !bound.is_multiple_of(PAGE_SIZE))
        || boot_stack_guard_end - boot_stack_guard_start != PAGE_SIZE
    {
        return Err(FinalKernelMapError::InvalidKernelLayout);
    }
    let image = physical_kernel_range(kernel_start, kernel_end)
        .map_err(|_| FinalKernelMapError::InvalidKernelLayout)?;
    let text = linked_frame_range(text_start, text_end)?;
    let rodata = linked_frame_range(rodata_start, rodata_end)?;
    let data = linked_frame_range(data_start, data_end)?;
    let boot_stack_guard = linked_frame_range(boot_stack_guard_start, boot_stack_guard_end)?;
    Ok(FinalKernelImageLayout {
        image,
        text,
        rodata,
        data,
        boot_stack_guard,
    })
}

#[cfg(any(target_arch = "aarch64", test))]
fn linked_frame_range(start: usize, end: usize) -> Result<FrameRange, FinalKernelMapError> {
    let physical_start = start
        .checked_sub(KERNEL_VIRTUAL_OFFSET)
        .ok_or(FinalKernelMapError::InvalidKernelLayout)?;
    let physical_end = end
        .checked_sub(KERNEL_VIRTUAL_OFFSET)
        .ok_or(FinalKernelMapError::InvalidKernelLayout)?;
    if physical_start >= physical_end
        || !physical_start.is_multiple_of(PAGE_SIZE)
        || !physical_end.is_multiple_of(PAGE_SIZE)
    {
        return Err(FinalKernelMapError::InvalidKernelLayout);
    }
    let start_frame = PageFrame::from_start(PhysAddr::new(physical_start))
        .map_err(|_| FinalKernelMapError::InvalidKernelLayout)?;
    FrameRange::new(start_frame, (physical_end - physical_start) / PAGE_SIZE)
        .map_err(|_| FinalKernelMapError::InvalidKernelLayout)
}

#[cfg(any(target_arch = "aarch64", test))]
fn physical_kernel_range(
    linked_start: usize,
    linked_end: usize,
) -> Result<AddressRange<PhysAddr>, BootstrapMemoryError> {
    let start = linked_start
        .checked_sub(KERNEL_VIRTUAL_OFFSET)
        .ok_or(BootstrapMemoryError::InvalidKernelImage)?;
    let end = linked_end
        .checked_sub(KERNEL_VIRTUAL_OFFSET)
        .ok_or(BootstrapMemoryError::InvalidKernelImage)?;
    if start < BOOTSTRAP_RAM_PHYSICAL_START || start >= end || end > BOOTSTRAP_RAM_PHYSICAL_END {
        return Err(BootstrapMemoryError::InvalidKernelImage);
    }
    AddressRange::new(PhysAddr::new(start), PhysAddr::new(end))
        .map_err(|_| BootstrapMemoryError::InvalidKernelImage)
}

impl ProcessImageMemory for BootstrapPhysicalMemory {
    type Error = BootstrapMemoryError;

    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        let destination = Self::translated_address(frame, 0, PAGE_SIZE)? as *mut u8;
        // SAFETY: construction guarantees the complete higher-half frame is
        // writable and privately owned. The checked range is exactly one page.
        unsafe {
            ptr::write_bytes(destination, 0, PAGE_SIZE);
        }
        Ok(())
    }

    fn write_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), Self::Error> {
        let destination = Self::translated_address(frame, offset, bytes.len())? as *mut u8;
        if bytes.is_empty() {
            return Ok(());
        }
        // SAFETY: construction guarantees unique writable destination memory;
        // the checked range lies within one frame. `ptr::copy` also permits a
        // source slice that aliases the destination during bootstrap loading.
        unsafe {
            ptr::copy(bytes.as_ptr(), destination, bytes.len());
        }
        Ok(())
    }
}

impl UserMemoryReader for BootstrapPhysicalMemory {
    type Error = BootstrapMemoryError;

    fn read_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), Self::Error> {
        BootstrapPhysicalMemory::read_frame(self, frame, offset, output)
    }
}

impl UserMemoryWriter for BootstrapPhysicalMemory {
    type Error = BootstrapMemoryError;

    fn write_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        input: &[u8],
    ) -> Result<(), Self::Error> {
        ProcessImageMemory::write_frame(self, frame, offset, input)
    }
}

impl TranslationTableMemory for BootstrapPhysicalMemory {
    type Error = BootstrapMemoryError;

    fn clear_table(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        ProcessImageMemory::clear_frame(self, frame)
    }

    fn write_descriptor(
        &mut self,
        frame: PageFrame,
        index: usize,
        descriptor: Descriptor,
    ) -> Result<(), Self::Error> {
        let offset = index
            .checked_mul(core::mem::size_of::<u64>())
            .ok_or(BootstrapMemoryError::AddressOverflow)?;
        let destination = Self::translated_address(frame, offset, size_of::<u64>())? as *mut u64;
        // SAFETY: the frame is private writable normal memory, its base is page
        // aligned, and the checked descriptor offset preserves u64 alignment.
        unsafe {
            ptr::write(destination, descriptor.raw());
        }
        Ok(())
    }
}

impl<const TABLES: usize, const RANGES: usize> HeapBackingMemory
    for FinalDirectMapMemory<'_, TABLES, RANGES>
{
    type Error = FinalDirectMapAccessError;

    fn virtual_start(&mut self, frames: FrameRange) -> Result<VirtAddr, Self::Error> {
        self.layout.range_virtual_start(frames)
    }

    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        self.clear_owned_frame(frame)
    }
}

impl<const TABLES: usize, const RANGES: usize> KernelStackMemory
    for FinalDirectMapMemory<'_, TABLES, RANGES>
{
    type Error = FinalDirectMapAccessError;

    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        self.clear_owned_frame(frame)
    }
}

impl<const TABLES: usize, const RANGES: usize> ProcessImageMemory
    for FinalDirectMapMemory<'_, TABLES, RANGES>
{
    type Error = FinalDirectMapAccessError;

    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        self.clear_owned_frame(frame)
    }

    fn write_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), Self::Error> {
        self.write_frame_bytes(frame, offset, bytes)
    }
}

impl<const TABLES: usize, const RANGES: usize> UserMemoryReader
    for FinalDirectMapMemory<'_, TABLES, RANGES>
{
    type Error = FinalDirectMapAccessError;

    fn read_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), Self::Error> {
        FinalDirectMapMemory::read_frame(self, frame, offset, output)
    }
}

impl<const TABLES: usize, const RANGES: usize> UserMemoryWriter
    for FinalDirectMapMemory<'_, TABLES, RANGES>
{
    type Error = FinalDirectMapAccessError;

    fn write_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        input: &[u8],
    ) -> Result<(), Self::Error> {
        self.write_frame_bytes(frame, offset, input)
    }
}

impl<const TABLES: usize, const RANGES: usize> TranslationTableMemory
    for FinalDirectMapMemory<'_, TABLES, RANGES>
{
    type Error = FinalDirectMapAccessError;

    fn clear_table(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        self.clear_owned_frame(frame)
    }

    fn write_descriptor(
        &mut self,
        frame: PageFrame,
        index: usize,
        descriptor: Descriptor,
    ) -> Result<(), Self::Error> {
        let offset = index
            .checked_mul(core::mem::size_of::<u64>())
            .ok_or(FinalDirectMapAccessError::AddressOverflow)?;
        let destination =
            self.layout
                .translated_address(frame, offset, size_of::<u64>())? as *mut u64;
        // SAFETY: the capability proves the active writable final mapping;
        // the trait contract requires a private table frame, and the checked
        // descriptor index preserves u64 alignment inside the frame.
        unsafe {
            ptr::write(destination, descriptor.raw());
        }
        Ok(())
    }
}

// SAFETY: reads and writes use the retained final direct map for exact
// allocator-managed table frames. A successful read validates the complete
// aligned u64 slot; while the caller preserves exclusive access, the matching
// volatile write cannot fail or target unrelated memory.
unsafe impl<const TABLES: usize, const RANGES: usize> ActiveKernelTableMemory
    for FinalDirectMapMemory<'_, TABLES, RANGES>
{
    type Error = FinalDirectMapAccessError;

    fn read_descriptor(&mut self, frame: PageFrame, index: usize) -> Result<u64, Self::Error> {
        let offset = index
            .checked_mul(core::mem::size_of::<u64>())
            .ok_or(FinalDirectMapAccessError::AddressOverflow)?;
        let source =
            self.layout
                .translated_address(frame, offset, size_of::<u64>())? as *const u64;
        // SAFETY: checked translation proves an aligned complete u64 inside
        // one retained, writable normal-memory table frame.
        Ok(unsafe { ptr::read_volatile(source) })
    }

    unsafe fn write_descriptor(&mut self, frame: PageFrame, index: usize, descriptor: Descriptor) {
        let offset = index
            .checked_mul(core::mem::size_of::<u64>())
            .expect("preflighted descriptor offset");
        let destination = self
            .layout
            .translated_address(frame, offset, size_of::<u64>())
            .expect("preflighted active table descriptor") as *mut u64;
        // SAFETY: the trait caller successfully preflighted this exact slot
        // and retains exclusive active-table access through the write.
        unsafe {
            ptr::write_volatile(destination, descriptor.raw());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BOOTSTRAP_DEVICE_PHYSICAL_END, BOOTSTRAP_RAM_PHYSICAL_END, BOOTSTRAP_RAM_PHYSICAL_START,
        BootstrapDeviceAccessError, BootstrapMemoryError, BootstrapPhysicalMemory,
        FinalDirectMapAccessError, FinalDirectMapLayoutError, FinalKernelMapError,
        KERNEL_STACK_ARENA_SLOTS, KERNEL_STACK_ARENA_START, KERNEL_STACK_USABLE_PAGES,
        KERNEL_VIRTUAL_OFFSET, PL011_VIRTUAL_BASE, QemuVirtGicV3Layout,
        QemuVirtKernelPageTablePlan, checked_bootstrap_device_access, final_direct_map_layout,
        final_kernel_page_table_plan_from_regions, linked_kernel_image_layout,
        physical_kernel_range, runtime_kernel_stack_pool, validate_runtime_stack_arena,
    };
    use crate::arch::aarch64::gicv3::GicV3MmioLayout;
    use crate::arch::aarch64::kernel_page_table::{DirectMapOverride, KernelTranslationTableRole};
    use crate::arch::aarch64::paging::{MappingAttributes, TranslationIndices, TranslationLevel};
    use crate::memory::{
        AddressRange, FrameRange, MemoryMap, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr,
    };

    fn gic_layout() -> QemuVirtGicV3Layout {
        let distributor_physical =
            AddressRange::new(PhysAddr::new(0x0800_0000), PhysAddr::new(0x0801_0000)).unwrap();
        let redistributor_physical =
            AddressRange::new(PhysAddr::new(0x080a_0000), PhysAddr::new(0x080c_0000)).unwrap();
        let distributor_virtual = AddressRange::new(
            VirtAddr::new(KERNEL_VIRTUAL_OFFSET + distributor_physical.start().as_usize()),
            VirtAddr::new(KERNEL_VIRTUAL_OFFSET + distributor_physical.end().as_usize()),
        )
        .unwrap();
        let redistributor_virtual = AddressRange::new(
            VirtAddr::new(KERNEL_VIRTUAL_OFFSET + redistributor_physical.start().as_usize()),
            VirtAddr::new(KERNEL_VIRTUAL_OFFSET + redistributor_physical.end().as_usize()),
        )
        .unwrap();
        QemuVirtGicV3Layout {
            distributor_physical,
            redistributor_physical,
            registers: GicV3MmioLayout::new(distributor_virtual, redistributor_virtual).unwrap(),
        }
    }

    #[test]
    fn bootstrap_device_access_is_bounded_and_naturally_aligned() {
        assert_eq!(
            checked_bootstrap_device_access(PhysAddr::new(0x080a_0008), size_of::<u64>()),
            Ok(0x080a_0008)
        );
        assert_eq!(
            checked_bootstrap_device_access(PhysAddr::new(0x080a_0004), size_of::<u64>()),
            Err(BootstrapDeviceAccessError::MisalignedAccess {
                address: 0x080a_0004,
                alignment: size_of::<u64>(),
            })
        );
        assert_eq!(
            checked_bootstrap_device_access(
                PhysAddr::new(BOOTSTRAP_DEVICE_PHYSICAL_END - 4),
                size_of::<u64>(),
            ),
            Err(BootstrapDeviceAccessError::MisalignedAccess {
                address: BOOTSTRAP_DEVICE_PHYSICAL_END - 4,
                alignment: size_of::<u64>(),
            })
        );
        assert_eq!(
            checked_bootstrap_device_access(
                PhysAddr::new(BOOTSTRAP_DEVICE_PHYSICAL_END),
                size_of::<u32>(),
            ),
            Err(BootstrapDeviceAccessError::OutsideTemporaryDeviceWindow {
                address: BOOTSTRAP_DEVICE_PHYSICAL_END,
                width: size_of::<u32>(),
            })
        );
        assert_eq!(
            checked_bootstrap_device_access(PhysAddr::new(usize::MAX - 3), size_of::<u32>()),
            Err(BootstrapDeviceAccessError::AddressOverflow)
        );
    }

    #[test]
    fn translates_only_complete_frames_in_the_bootstrap_ram_window() {
        let first = frame(BOOTSTRAP_RAM_PHYSICAL_START);
        let last = frame(BOOTSTRAP_RAM_PHYSICAL_END - PAGE_SIZE);
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(first, 0, PAGE_SIZE),
            Ok(KERNEL_VIRTUAL_OFFSET + BOOTSTRAP_RAM_PHYSICAL_START)
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(last, PAGE_SIZE - 1, 1),
            Ok(KERNEL_VIRTUAL_OFFSET + BOOTSTRAP_RAM_PHYSICAL_END - 1)
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame(0x3fff_f000), 0, 1),
            Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: 0x3fff_f000
            })
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame(BOOTSTRAP_RAM_PHYSICAL_END), 0, 1),
            Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: BOOTSTRAP_RAM_PHYSICAL_END
            })
        );
    }

    #[test]
    fn rejects_cross_page_and_overflowing_accesses() {
        let frame = frame(BOOTSTRAP_RAM_PHYSICAL_START);
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame, PAGE_SIZE - 1, 2),
            Err(BootstrapMemoryError::AccessOutOfBounds {
                offset: PAGE_SIZE - 1,
                length: 2
            })
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame, usize::MAX, 2),
            Err(BootstrapMemoryError::AddressOverflow)
        );
    }

    #[test]
    fn translates_only_ranges_inside_the_bootstrap_ram_window() {
        assert_eq!(
            BootstrapPhysicalMemory::translated_physical_range(
                PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_START),
                PAGE_SIZE,
            ),
            Ok(KERNEL_VIRTUAL_OFFSET + BOOTSTRAP_RAM_PHYSICAL_START)
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_physical_range(
                PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_END - 1),
                1,
            ),
            Ok(KERNEL_VIRTUAL_OFFSET + BOOTSTRAP_RAM_PHYSICAL_END - 1)
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_physical_range(
                PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_START - 1),
                1,
            ),
            Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: BOOTSTRAP_RAM_PHYSICAL_START - 1,
            })
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_physical_range(
                PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_END - 1),
                2,
            ),
            Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: BOOTSTRAP_RAM_PHYSICAL_END - 1,
            })
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_physical_range(PhysAddr::new(usize::MAX), 2),
            Err(BootstrapMemoryError::AddressOverflow)
        );
    }

    #[test]
    fn converts_linked_kernel_bounds_back_to_physical_addresses() {
        let linked_start = KERNEL_VIRTUAL_OFFSET + 0x4008_0000;
        let linked_end = linked_start + 0x20_0000;
        let range = physical_kernel_range(linked_start, linked_end).unwrap();
        assert_eq!(range.start(), PhysAddr::new(0x4008_0000));
        assert_eq!(range.end(), PhysAddr::new(0x4028_0000));

        assert_eq!(
            physical_kernel_range(KERNEL_VIRTUAL_OFFSET - 1, linked_end),
            Err(BootstrapMemoryError::InvalidKernelImage)
        );
        assert_eq!(
            physical_kernel_range(linked_start, linked_start),
            Err(BootstrapMemoryError::InvalidKernelImage)
        );
        assert_eq!(
            physical_kernel_range(linked_end, linked_start),
            Err(BootstrapMemoryError::InvalidKernelImage)
        );
    }

    #[test]
    fn derives_final_kernel_permissions_and_pinned_qemu_topology() {
        let base = KERNEL_VIRTUAL_OFFSET + 0x4008_0000;
        let layout = linked_kernel_image_layout([
            base,
            base + 0x18_0000,
            base,
            base + 0x20_000,
            base + 0x20_000,
            base + 0x30_000,
            base + 0x30_000,
            base + 0x18_0000,
            base + 0x100_000,
            base + 0x101_000,
        ])
        .unwrap();
        let ram = AddressRange::new(
            PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_START),
            PhysAddr::new(0x6000_0000),
        )
        .unwrap();
        let gic = gic_layout();
        let plan = final_kernel_page_table_plan_from_regions(layout, [Ok(ram)], gic).unwrap();

        assert_eq!(plan.required_table_frames(), 8);
        assert_eq!(plan.mappings().len(), 815);
        for (address, attributes) in [
            (base, MappingAttributes::kernel_code()),
            (base + 0x20_000, MappingAttributes::kernel_rodata()),
            (base + 0x30_000, MappingAttributes::kernel_data()),
        ] {
            assert_eq!(
                plan.translate(VirtAddr::new(address))
                    .unwrap()
                    .unwrap()
                    .attributes,
                attributes
            );
        }
        assert_eq!(
            plan.translate(VirtAddr::new(base + 0x100_000)).unwrap(),
            None
        );
        let uart = plan
            .translate(VirtAddr::new(PL011_VIRTUAL_BASE))
            .unwrap()
            .unwrap();
        assert_eq!(uart.level, TranslationLevel::L3);
        assert_eq!(uart.attributes, MappingAttributes::kernel_device());
        for (physical, virtual_range) in [
            (gic.distributor_physical, gic.registers.distributor()),
            (gic.redistributor_physical, gic.registers.redistributor()),
        ] {
            for offset in (0..physical.len()).step_by(PAGE_SIZE) {
                let translation = plan
                    .translate(VirtAddr::new(virtual_range.start().as_usize() + offset))
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    translation.physical_address,
                    PhysAddr::new(physical.start().as_usize() + offset)
                );
                assert_eq!(translation.level, TranslationLevel::L3);
                assert_eq!(translation.attributes, MappingAttributes::kernel_device());
            }
        }

        let mut memory = MemoryMap::<4>::new();
        memory.add_usable(ram).unwrap();
        memory.reserve(layout.image()).unwrap();
        let managed = memory.into_allocator().managed_frames();
        let proof = final_direct_map_layout(&plan, managed).expect("actual final-plan proof");
        assert_eq!(proof.managed_frames(), managed);

        let stack_pool = runtime_kernel_stack_pool().expect("runtime-stack arena");
        let stack_indices =
            TranslationIndices::new(stack_pool.arena().start()).expect("canonical stack arena");
        assert!(plan.tables().any(|role| {
            role == KernelTranslationTableRole::Level2 {
                l1: stack_indices.l1(),
            }
        }));
        assert!(plan.tables().any(|role| {
            role == KernelTranslationTableRole::Level3 {
                l1: stack_indices.l1(),
                l2: stack_indices.l2(),
            }
        }));
        assert_eq!(
            stack_pool.arena().start().as_usize(),
            KERNEL_STACK_ARENA_START
        );
        assert_eq!(
            stack_pool.arena().len(),
            KERNEL_STACK_ARENA_SLOTS * (KERNEL_STACK_USABLE_PAGES + 2) * PAGE_SIZE
        );
        let mut address = stack_pool.arena().start().as_usize();
        while address < stack_pool.arena().end().as_usize() {
            assert_eq!(plan.translate(VirtAddr::new(address)), Ok(None));
            address += PAGE_SIZE;
        }
    }

    #[test]
    fn rejects_a_final_mapping_inside_the_runtime_stack_arena() {
        let mut plan = QemuVirtKernelPageTablePlan::new().unwrap();
        plan.map(
            VirtAddr::new(KERNEL_STACK_ARENA_START),
            PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_START),
            TranslationLevel::L3,
            MappingAttributes::kernel_data(),
        )
        .unwrap();

        assert_eq!(
            validate_runtime_stack_arena(&plan),
            Err(FinalKernelMapError::RuntimeStackArenaMapped {
                address: KERNEL_STACK_ARENA_START,
            })
        );
    }

    #[test]
    fn proves_only_allocator_managed_final_ram_mappings() {
        let ram_start = BOOTSTRAP_RAM_PHYSICAL_START;
        let ram_end = ram_start + 2 * 1024 * 1024;
        let ram = AddressRange::new(PhysAddr::new(ram_start), PhysAddr::new(ram_end)).unwrap();
        let mut plan = QemuVirtKernelPageTablePlan::new().unwrap();
        plan.map_direct_frames(
            FrameRange::from_usable_bytes(ram),
            KERNEL_VIRTUAL_OFFSET,
            MappingAttributes::kernel_data(),
            &[],
        )
        .unwrap();

        let mut map = MemoryMap::<4>::new();
        map.add_usable(ram).unwrap();
        map.reserve(
            AddressRange::new(
                PhysAddr::new(ram_start),
                PhysAddr::new(ram_start + PAGE_SIZE),
            )
            .unwrap(),
        )
        .unwrap();
        map.reserve(
            AddressRange::new(
                PhysAddr::new(ram_start + 7 * PAGE_SIZE),
                PhysAddr::new(ram_start + 9 * PAGE_SIZE),
            )
            .unwrap(),
        )
        .unwrap();
        let managed = map.into_allocator().managed_frames();
        let layout = final_direct_map_layout(&plan, managed).expect("direct-map proof");

        let first = frame(ram_start + PAGE_SIZE);
        assert_eq!(
            layout.translated_address(first, PAGE_SIZE - 1, 1),
            Ok(KERNEL_VIRTUAL_OFFSET + ram_start + 2 * PAGE_SIZE - 1)
        );
        assert_eq!(
            layout.translated_address(frame(ram_start), 0, 1),
            Err(FinalDirectMapAccessError::UnmanagedFrame { address: ram_start })
        );
        assert_eq!(
            layout.translated_address(first, PAGE_SIZE - 1, 2),
            Err(FinalDirectMapAccessError::AccessOutOfBounds {
                offset: PAGE_SIZE - 1,
                length: 2,
            })
        );
        assert_eq!(layout.managed_frames(), managed);
    }

    #[test]
    fn direct_map_proof_rejects_holes_permissions_and_wrong_outputs() {
        let start = BOOTSTRAP_RAM_PHYSICAL_START;
        let bytes =
            AddressRange::new(PhysAddr::new(start), PhysAddr::new(start + 2 * PAGE_SIZE)).unwrap();
        let frames = FrameRange::from_usable_bytes(bytes);
        let mut map = MemoryMap::<1>::new();
        map.add_usable(bytes).unwrap();
        let managed = map.into_allocator().managed_frames();

        let mut missing = QemuVirtKernelPageTablePlan::new().unwrap();
        missing
            .map_direct_frames(
                frames,
                KERNEL_VIRTUAL_OFFSET,
                MappingAttributes::kernel_data(),
                &[DirectMapOverride::unmapped(
                    FrameRange::new(frame(start + PAGE_SIZE), 1).unwrap(),
                )
                .unwrap()],
            )
            .unwrap();
        assert_eq!(
            final_direct_map_layout(&missing, managed),
            Err(FinalDirectMapLayoutError::MissingMapping {
                frame_number: start / PAGE_SIZE + 1,
            })
        );

        let mut read_only = QemuVirtKernelPageTablePlan::new().unwrap();
        read_only
            .map_direct_frames(
                frames,
                KERNEL_VIRTUAL_OFFSET,
                MappingAttributes::kernel_rodata(),
                &[],
            )
            .unwrap();
        assert_eq!(
            final_direct_map_layout(&read_only, managed),
            Err(FinalDirectMapLayoutError::WrongAttributes {
                frame_number: start / PAGE_SIZE,
                attributes: MappingAttributes::kernel_rodata(),
            })
        );

        let mut wrong_output = QemuVirtKernelPageTablePlan::new().unwrap();
        wrong_output
            .map(
                VirtAddr::new(KERNEL_VIRTUAL_OFFSET + start),
                PhysAddr::new(start + PAGE_SIZE),
                TranslationLevel::L3,
                MappingAttributes::kernel_data(),
            )
            .unwrap();
        assert_eq!(
            final_direct_map_layout(&wrong_output, managed),
            Err(FinalDirectMapLayoutError::WrongPhysicalAddress {
                frame_number: start / PAGE_SIZE,
                translated_address: start + PAGE_SIZE,
            })
        );
    }

    #[test]
    fn direct_map_proof_rejects_an_empty_managed_domain() {
        let plan = QemuVirtKernelPageTablePlan::new().unwrap();
        let managed = MemoryMap::<1>::new().into_allocator().managed_frames();

        assert_eq!(
            final_direct_map_layout(&plan, managed),
            Err(FinalDirectMapLayoutError::NoManagedFrames)
        );
    }

    #[test]
    fn rejects_invalid_linker_layout_and_ram_outside_bootstrap_window() {
        let base = KERNEL_VIRTUAL_OFFSET + 0x4008_0000;
        assert_eq!(
            linked_kernel_image_layout([
                base,
                base + 0x5000,
                base,
                base + 0x1001,
                base + 0x2000,
                base + 0x3000,
                base + 0x3000,
                base + 0x5000,
                base + 0x3000,
                base + 0x4000,
            ]),
            Err(FinalKernelMapError::InvalidKernelLayout)
        );
        assert_eq!(
            linked_kernel_image_layout([
                base,
                base + 0x5000,
                base,
                base + 0x1000,
                base + 0x1000,
                base + 0x2000,
                base + 0x2000,
                base + 0x5000,
                base + 0x3000,
                base + 0x5000,
            ]),
            Err(FinalKernelMapError::InvalidKernelLayout)
        );

        let layout = linked_kernel_image_layout([
            base,
            base + 0x5000,
            base,
            base + 0x1000,
            base + 0x1000,
            base + 0x2000,
            base + 0x2000,
            base + 0x5000,
            base + 0x3000,
            base + 0x4000,
        ])
        .unwrap();
        let outside = AddressRange::new(
            PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_START - PAGE_SIZE),
            PhysAddr::new(BOOTSTRAP_RAM_PHYSICAL_START + PAGE_SIZE),
        )
        .unwrap();
        assert_eq!(
            final_kernel_page_table_plan_from_regions(layout, [Ok(outside)], gic_layout()),
            Err(FinalKernelMapError::RamOutsideBootstrap {
                start: BOOTSTRAP_RAM_PHYSICAL_START - PAGE_SIZE,
                end: BOOTSTRAP_RAM_PHYSICAL_START + PAGE_SIZE,
            })
        );
    }

    fn frame(address: usize) -> PageFrame {
        PageFrame::from_start(PhysAddr::new(address)).unwrap()
    }
}
