//! Early devices on QEMU's AArch64 `virt` machine.

use core::ptr::{self, read_volatile, write_volatile};

#[cfg(any(target_arch = "aarch64", test))]
use crate::arch::aarch64::kernel_page_table::DirectMapOverride;
use crate::arch::aarch64::kernel_page_table::{KernelPageTableError, KernelPageTablePlan};
use crate::arch::aarch64::paging::Descriptor;
#[cfg(any(target_arch = "aarch64", test))]
use crate::arch::aarch64::paging::{MappingAttributes, TranslationLevel};
use crate::arch::aarch64::user_page_table::TranslationTableMemory;
use crate::console::ByteSink;
#[cfg(target_arch = "aarch64")]
use crate::dtb::BootInfo;
use crate::dtb::{DeviceTree, Error as DeviceTreeError};
#[cfg(any(target_arch = "aarch64", test))]
use crate::memory::VirtAddr;
use crate::memory::{AddressRange, FrameRange, PAGE_SIZE, PageFrame, PhysAddr};
use crate::process_image::ProcessImageMemory;
use crate::user_copy::{UserMemoryReader, UserMemoryWriter};

/// Physical base of QEMU `virt`'s first PL011 UART.
pub const PL011_PHYSICAL_BASE: usize = 0x0900_0000;
/// Final higher-half virtual address of the first PL011 UART.
pub const PL011_VIRTUAL_BASE: usize = 0xffff_ff80_0900_0000;
/// Physical start of the RAM window reachable through the bootstrap alias.
pub const BOOTSTRAP_RAM_PHYSICAL_START: usize = 0x4000_0000;
/// Exclusive physical end of the RAM window reachable through the bootstrap alias.
pub const BOOTSTRAP_RAM_PHYSICAL_END: usize = 0x8000_0000;
/// Fixed physical-to-virtual offset of the QEMU `virt` kernel direct map.
pub const KERNEL_VIRTUAL_OFFSET: usize = 0xffff_ff80_0000_0000;
/// Maximum number of page-table frames in the pinned single-region layout.
pub const FINAL_KERNEL_TABLE_CAPACITY: usize = 8;
/// Maximum number of leaf mappings in the pinned 512 MiB layout.
pub const FINAL_KERNEL_MAPPING_CAPACITY: usize = 1024;
const UART_DR: usize = 0x00;
const UART_FR: usize = 0x18;
const UART_FR_BUSY: u32 = 1 << 3;
const UART_FR_TXFF: u32 = 1 << 5;

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
    /// The fixed table topology or mapping metadata cannot represent the layout.
    PageTable(KernelPageTableError),
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
) -> Result<QemuVirtKernelPageTablePlan, FinalKernelMapError> {
    let layout = kernel_image_layout()?;
    final_kernel_page_table_plan_from_regions(
        layout,
        info.memory_regions()
            .map(|region| region.map_err(FinalKernelMapError::DeviceTree)),
    )
}

#[cfg(any(target_arch = "aarch64", test))]
fn final_kernel_page_table_plan_from_regions<I>(
    layout: FinalKernelImageLayout,
    regions: I,
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
        plan = plan
            .with_direct_frames(
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

    plan = plan
        .with_mapping(
            VirtAddr::new(PL011_VIRTUAL_BASE),
            PhysAddr::new(PL011_PHYSICAL_BASE),
            TranslationLevel::L3,
            MappingAttributes::kernel_device(),
        )
        .map_err(FinalKernelMapError::PageTable)?;
    validate_critical_mappings(&plan, layout)?;
    Ok(plan)
}

#[cfg(any(target_arch = "aarch64", test))]
fn validate_critical_mappings(
    plan: &QemuVirtKernelPageTablePlan,
    layout: FinalKernelImageLayout,
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
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{
        BOOTSTRAP_RAM_PHYSICAL_END, BOOTSTRAP_RAM_PHYSICAL_START, BootstrapMemoryError,
        BootstrapPhysicalMemory, FinalKernelMapError, KERNEL_VIRTUAL_OFFSET, PL011_VIRTUAL_BASE,
        final_kernel_page_table_plan_from_regions, linked_kernel_image_layout,
        physical_kernel_range,
    };
    use crate::arch::aarch64::paging::{MappingAttributes, TranslationLevel};
    use crate::memory::{AddressRange, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr};

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
        let plan = final_kernel_page_table_plan_from_regions(layout, [Ok(ram)]).unwrap();

        assert_eq!(plan.required_table_frames(), 5);
        assert_eq!(plan.mappings().len(), 767);
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
            final_kernel_page_table_plan_from_regions(layout, [Ok(outside)]),
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
