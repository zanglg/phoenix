//! Pure AArch64 stage-1 translation descriptors and mapping plans.

use crate::memory::{PAGE_SIZE, PageFrame, PhysAddr, VirtAddr};

const ENTRIES_PER_TABLE: usize = 512;
const PHYSICAL_ADDRESS_BITS: u32 = 48;
const OUTPUT_ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;
const DESCRIPTOR_VALID: u64 = 1 << 0;
const DESCRIPTOR_TABLE_OR_PAGE: u64 = 1 << 1;
const DESCRIPTOR_ATTR_INDEX_SHIFT: u32 = 2;
const DESCRIPTOR_AP_SHIFT: u32 = 6;
const DESCRIPTOR_SH_SHIFT: u32 = 8;
const DESCRIPTOR_ACCESS_FLAG: u64 = 1 << 10;
const DESCRIPTOR_NOT_GLOBAL: u64 = 1 << 11;
const DESCRIPTOR_PXN: u64 = 1 << 53;
const DESCRIPTOR_UXN: u64 = 1 << 54;
const SHAREABILITY_OUTER: u64 = 0b10;
const SHAREABILITY_INNER: u64 = 0b11;
const VA_BITS: u32 = 39;
const LOWER_LIMIT: u64 = 1_u64 << VA_BITS;
const UPPER_BASE: u64 = u64::MAX - LOWER_LIMIT + 1;

/// Failure while defining an AArch64 translation or descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagingError {
    /// The virtual address is outside the configured lower and upper 39-bit regions.
    NonCanonicalVirtualAddress {
        /// Rejected virtual address.
        address: usize,
    },
    /// A virtual or physical address does not align to the mapping size.
    MisalignedMapping {
        /// Rejected virtual address.
        virtual_address: usize,
        /// Rejected physical address.
        physical_address: usize,
        /// Required mapping alignment.
        alignment: usize,
    },
    /// A descriptor output address does not align to its translation level.
    MisalignedOutputAddress {
        /// Rejected output physical address.
        address: usize,
        /// Required descriptor alignment.
        alignment: usize,
    },
    /// A physical address exceeds the implemented 48-bit descriptor field.
    PhysicalAddressTooWide {
        /// Rejected physical address.
        address: usize,
    },
    /// Virtual or physical range calculation overflowed.
    AddressOverflow,
    /// A writable mapping was requested with execution permission.
    WritableExecutable,
    /// Execution permission does not match the mapping's privilege domain.
    InvalidExecuteDomain,
    /// The virtual mapping overlaps an existing entry in the plan.
    AlreadyMapped,
    /// The fixed-capacity mapping plan has no free metadata entry.
    CapacityExceeded,
}

/// One leaf level in the current 39-bit, 4 KiB translation regime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranslationLevel {
    /// Level 1, where a block covers 1 GiB.
    L1,
    /// Level 2, where a block covers 2 MiB.
    L2,
    /// Level 3, where a page covers 4 KiB.
    L3,
}

impl TranslationLevel {
    /// Return the number of bytes translated by one descriptor at this level.
    pub const fn mapping_size(self) -> usize {
        match self {
            Self::L1 => 1 << 30,
            Self::L2 => 1 << 21,
            Self::L3 => PAGE_SIZE,
        }
    }

    const fn descriptor_type(self) -> u64 {
        match self {
            Self::L1 | Self::L2 => DESCRIPTOR_VALID,
            Self::L3 => DESCRIPTOR_VALID | DESCRIPTOR_TABLE_OR_PAGE,
        }
    }
}

/// Whether an address is translated through the lower or upper virtual region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressSpaceHalf {
    /// Lower region selected through TTBR0_EL1.
    Lower,
    /// Upper region selected through TTBR1_EL1.
    Upper,
}

/// Translation-table indices for a canonical 39-bit virtual address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TranslationIndices {
    half: AddressSpaceHalf,
    l1: usize,
    l2: usize,
    l3: usize,
}

impl TranslationIndices {
    /// Decode a canonical virtual address for the configured 39-bit regime.
    pub fn new(address: VirtAddr) -> Result<Self, PagingError> {
        let raw = address.as_usize() as u64;
        let half = if raw < LOWER_LIMIT {
            AddressSpaceHalf::Lower
        } else if raw >= UPPER_BASE {
            AddressSpaceHalf::Upper
        } else {
            return Err(PagingError::NonCanonicalVirtualAddress {
                address: address.as_usize(),
            });
        };

        Ok(Self {
            half,
            l1: ((raw >> 30) & 0x1ff) as usize,
            l2: ((raw >> 21) & 0x1ff) as usize,
            l3: ((raw >> 12) & 0x1ff) as usize,
        })
    }

    /// Return which translation-table base register covers the address.
    pub const fn half(self) -> AddressSpaceHalf {
        self.half
    }

    /// Return the level-1 table index.
    pub const fn l1(self) -> usize {
        self.l1
    }

    /// Return the level-2 table index.
    pub const fn l2(self) -> usize {
        self.l2
    }

    /// Return the level-3 table index.
    pub const fn l3(self) -> usize {
        self.l3
    }
}

/// MAIR attribute slot selected by a leaf mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MemoryType {
    /// Normal cacheable memory, using MAIR index zero.
    Normal = 0,
    /// Device-nGnRnE memory, using MAIR index one.
    Device = 1,
}

/// Read/write and privilege access encoded in descriptor AP bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Access {
    /// EL1 read/write; EL0 inaccessible.
    KernelReadWrite = 0b00,
    /// EL1 and EL0 read/write.
    UserReadWrite = 0b01,
    /// EL1 read-only; EL0 inaccessible.
    KernelReadOnly = 0b10,
    /// EL1 and EL0 read-only.
    UserReadOnly = 0b11,
}

impl Access {
    const fn writable(self) -> bool {
        matches!(self, Self::KernelReadWrite | Self::UserReadWrite)
    }

    const fn user_accessible(self) -> bool {
        matches!(self, Self::UserReadWrite | Self::UserReadOnly)
    }
}

/// Execution domain permitted by a leaf mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Execute {
    /// Execution is prohibited at EL1 and EL0.
    Never,
    /// EL1 may execute; EL0 may not.
    Kernel,
    /// EL0 may execute; EL1 may not.
    User,
}

/// Validated memory, access, and execution attributes for one mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingAttributes {
    memory_type: MemoryType,
    access: Access,
    execute: Execute,
}

impl MappingAttributes {
    /// Validate an explicit mapping-attribute combination.
    pub const fn new(
        memory_type: MemoryType,
        access: Access,
        execute: Execute,
    ) -> Result<Self, PagingError> {
        if access.writable() && !matches!(execute, Execute::Never) {
            return Err(PagingError::WritableExecutable);
        }
        if matches!(execute, Execute::Kernel) && access.user_accessible()
            || matches!(execute, Execute::User) && !access.user_accessible()
        {
            return Err(PagingError::InvalidExecuteDomain);
        }
        Ok(Self {
            memory_type,
            access,
            execute,
        })
    }

    /// Kernel read-only executable normal memory.
    pub const fn kernel_code() -> Self {
        Self {
            memory_type: MemoryType::Normal,
            access: Access::KernelReadOnly,
            execute: Execute::Kernel,
        }
    }

    /// Kernel read-only, non-executable normal memory.
    pub const fn kernel_rodata() -> Self {
        Self {
            memory_type: MemoryType::Normal,
            access: Access::KernelReadOnly,
            execute: Execute::Never,
        }
    }

    /// Kernel read/write, non-executable normal memory.
    pub const fn kernel_data() -> Self {
        Self {
            memory_type: MemoryType::Normal,
            access: Access::KernelReadWrite,
            execute: Execute::Never,
        }
    }

    /// Kernel read/write, non-executable device memory.
    pub const fn kernel_device() -> Self {
        Self {
            memory_type: MemoryType::Device,
            access: Access::KernelReadWrite,
            execute: Execute::Never,
        }
    }

    /// User read-only executable normal memory, prohibited at EL1.
    pub const fn user_code() -> Self {
        Self {
            memory_type: MemoryType::Normal,
            access: Access::UserReadOnly,
            execute: Execute::User,
        }
    }

    /// User and kernel read/write, non-executable normal memory.
    pub const fn user_data() -> Self {
        Self {
            memory_type: MemoryType::Normal,
            access: Access::UserReadWrite,
            execute: Execute::Never,
        }
    }

    /// Return the selected memory type.
    pub const fn memory_type(self) -> MemoryType {
        self.memory_type
    }

    /// Return the selected access permission.
    pub const fn access(self) -> Access {
        self.access
    }

    /// Return the selected execution domain.
    pub const fn execute(self) -> Execute {
        self.execute
    }

    const fn descriptor_bits(self) -> u64 {
        let shareability = match self.memory_type {
            MemoryType::Normal => SHAREABILITY_INNER,
            MemoryType::Device => SHAREABILITY_OUTER,
        };
        let mut bits = (self.memory_type as u64) << DESCRIPTOR_ATTR_INDEX_SHIFT
            | (self.access as u64) << DESCRIPTOR_AP_SHIFT
            | shareability << DESCRIPTOR_SH_SHIFT
            | DESCRIPTOR_ACCESS_FLAG;
        if self.access.user_accessible() {
            bits |= DESCRIPTOR_NOT_GLOBAL;
        }
        match self.execute {
            Execute::Never => bits | DESCRIPTOR_PXN | DESCRIPTOR_UXN,
            Execute::Kernel => bits | DESCRIPTOR_UXN,
            Execute::User => bits | DESCRIPTOR_PXN,
        }
    }
}

/// A raw, validated AArch64 stage-1 descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Descriptor(u64);

impl Descriptor {
    /// Construct an invalid descriptor.
    pub const fn invalid() -> Self {
        Self(0)
    }

    /// Construct a next-level table descriptor.
    pub fn table(next_table: PageFrame) -> Result<Self, PagingError> {
        let address = next_table.start_address().as_usize();
        validate_physical_width(address)?;
        Ok(Self(
            address as u64 | DESCRIPTOR_VALID | DESCRIPTOR_TABLE_OR_PAGE,
        ))
    }

    /// Construct a level-1 block, level-2 block, or level-3 page descriptor.
    pub fn leaf(
        level: TranslationLevel,
        output: PhysAddr,
        attributes: MappingAttributes,
    ) -> Result<Self, PagingError> {
        let size = level.mapping_size();
        if !output.as_usize().is_multiple_of(size) {
            return Err(PagingError::MisalignedOutputAddress {
                address: output.as_usize(),
                alignment: size,
            });
        }
        validate_physical_width(output.as_usize())?;
        Ok(Self(
            output.as_usize() as u64 | level.descriptor_type() | attributes.descriptor_bits(),
        ))
    }

    /// Return the encoded 64-bit descriptor value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Return whether the descriptor valid bit is set.
    pub const fn is_valid(self) -> bool {
        self.0 & DESCRIPTOR_VALID != 0
    }

    /// Return the page-aligned output or next-table physical address.
    pub const fn output_address(self) -> PhysAddr {
        PhysAddr::new((self.0 & OUTPUT_ADDRESS_MASK) as usize)
    }
}

/// One mapping retained by an offline mapping plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlannedMapping {
    virtual_start: VirtAddr,
    physical_start: PhysAddr,
    level: TranslationLevel,
    attributes: MappingAttributes,
    descriptor: Descriptor,
}

impl PlannedMapping {
    const EMPTY: Self = Self {
        virtual_start: VirtAddr::new(0),
        physical_start: PhysAddr::new(0),
        level: TranslationLevel::L3,
        attributes: MappingAttributes::kernel_data(),
        descriptor: Descriptor::invalid(),
    };

    /// Return the first virtual address covered by this mapping.
    pub const fn virtual_start(self) -> VirtAddr {
        self.virtual_start
    }

    /// Return the first physical address covered by this mapping.
    pub const fn physical_start(self) -> PhysAddr {
        self.physical_start
    }

    /// Return the translation level used by this mapping.
    pub const fn level(self) -> TranslationLevel {
        self.level
    }

    /// Return the validated mapping attributes.
    pub const fn attributes(self) -> MappingAttributes {
        self.attributes
    }

    /// Return the raw hardware descriptor corresponding to the mapping.
    pub const fn descriptor(self) -> Descriptor {
        self.descriptor
    }

    fn virtual_end(self) -> Result<usize, PagingError> {
        self.virtual_start
            .as_usize()
            .checked_add(self.level.mapping_size())
            .ok_or(PagingError::AddressOverflow)
    }

    fn contains(self, address: VirtAddr) -> bool {
        self.virtual_start.as_usize() <= address.as_usize()
            && self.virtual_end().is_ok_and(|end| address.as_usize() < end)
    }
}

/// Result of translating a virtual address through an offline mapping plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Translation {
    /// Translated physical address, including the original in-mapping offset.
    pub physical_address: PhysAddr,
    /// Level at which translation terminated.
    pub level: TranslationLevel,
    /// Access and memory attributes of the leaf mapping.
    pub attributes: MappingAttributes,
}

/// Fixed-capacity mapping model used before mutating real translation tables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappingPlan<const CAPACITY: usize> {
    entries: [PlannedMapping; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> MappingPlan<CAPACITY> {
    /// Construct an empty mapping plan.
    pub const fn new() -> Self {
        Self {
            entries: [PlannedMapping::EMPTY; CAPACITY],
            len: 0,
        }
    }

    /// Insert one block or page mapping, preserving virtual-address order.
    pub fn map(
        &mut self,
        virtual_start: VirtAddr,
        physical_start: PhysAddr,
        level: TranslationLevel,
        attributes: MappingAttributes,
    ) -> Result<(), PagingError> {
        let first_indices = TranslationIndices::new(virtual_start)?;
        let size = level.mapping_size();
        if !virtual_start.as_usize().is_multiple_of(size)
            || !physical_start.as_usize().is_multiple_of(size)
        {
            return Err(PagingError::MisalignedMapping {
                virtual_address: virtual_start.as_usize(),
                physical_address: physical_start.as_usize(),
                alignment: size,
            });
        }
        let virtual_end = virtual_start
            .as_usize()
            .checked_add(size)
            .ok_or(PagingError::AddressOverflow)?;
        let last = VirtAddr::new(virtual_end - 1);
        if TranslationIndices::new(last)?.half() != first_indices.half() {
            return Err(PagingError::NonCanonicalVirtualAddress {
                address: last.as_usize(),
            });
        }
        validate_physical_width(physical_start.as_usize())?;
        let physical_last = physical_start
            .as_usize()
            .checked_add(size - 1)
            .ok_or(PagingError::AddressOverflow)?;
        validate_physical_width(physical_last)?;
        let descriptor = Descriptor::leaf(level, physical_start, attributes)?;
        let mapping = PlannedMapping {
            virtual_start,
            physical_start,
            level,
            attributes,
            descriptor,
        };

        let insertion =
            self.entries[..self.len].partition_point(|entry| entry.virtual_start < virtual_start);
        if insertion > 0 && mappings_overlap(self.entries[insertion - 1], mapping)?
            || insertion < self.len && mappings_overlap(self.entries[insertion], mapping)?
        {
            return Err(PagingError::AlreadyMapped);
        }
        if self.len == CAPACITY {
            return Err(PagingError::CapacityExceeded);
        }
        self.entries.copy_within(insertion..self.len, insertion + 1);
        self.entries[insertion] = mapping;
        self.len += 1;
        Ok(())
    }

    /// Remove an exact mapping and return it when present.
    pub fn unmap(
        &mut self,
        virtual_start: VirtAddr,
        level: TranslationLevel,
    ) -> Option<PlannedMapping> {
        let index = self.entries[..self.len]
            .iter()
            .position(|entry| entry.virtual_start == virtual_start && entry.level == level)?;
        let removed = self.entries[index];
        self.entries.copy_within(index + 1..self.len, index);
        self.len -= 1;
        self.entries[self.len] = PlannedMapping::EMPTY;
        Some(removed)
    }

    /// Translate an address through the planned leaf mappings.
    pub fn translate(&self, address: VirtAddr) -> Result<Option<Translation>, PagingError> {
        TranslationIndices::new(address)?;
        let Some(mapping) = self
            .entries()
            .iter()
            .copied()
            .find(|entry| entry.contains(address))
        else {
            return Ok(None);
        };
        let offset = address.as_usize() - mapping.virtual_start.as_usize();
        let physical_address = mapping
            .physical_start
            .checked_add(offset)
            .map_err(|_| PagingError::AddressOverflow)?;
        Ok(Some(Translation {
            physical_address,
            level: mapping.level,
            attributes: mapping.attributes,
        }))
    }

    /// Return planned mappings sorted by virtual start address.
    pub fn entries(&self) -> &[PlannedMapping] {
        &self.entries[..self.len]
    }
}

impl<const CAPACITY: usize> Default for MappingPlan<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

fn mappings_overlap(left: PlannedMapping, right: PlannedMapping) -> Result<bool, PagingError> {
    Ok(left.virtual_start.as_usize() < right.virtual_end()?
        && right.virtual_start.as_usize() < left.virtual_end()?)
}

fn validate_physical_width(address: usize) -> Result<(), PagingError> {
    if (address as u64) < (1_u64 << PHYSICAL_ADDRESS_BITS) {
        Ok(())
    } else {
        Err(PagingError::PhysicalAddressTooWide { address })
    }
}

const _: () = assert!(ENTRIES_PER_TABLE == 512);

#[cfg(test)]
mod tests {
    use super::{
        Access, AddressSpaceHalf, Descriptor, Execute, MappingAttributes, MappingPlan, MemoryType,
        PagingError, Translation, TranslationIndices, TranslationLevel,
    };
    use crate::memory::{PAGE_SIZE, PageFrame, PhysAddr, VirtAddr};

    #[test]
    fn decodes_lower_and_upper_39_bit_indices() {
        let lower = TranslationIndices::new(VirtAddr::new(0x0000_0000_4008_1234))
            .expect("lower canonical address");
        let upper = TranslationIndices::new(VirtAddr::new(0xffff_ff80_4008_1234))
            .expect("upper canonical address");

        assert_eq!(lower.half(), AddressSpaceHalf::Lower);
        assert_eq!(upper.half(), AddressSpaceHalf::Upper);
        assert_eq!((lower.l1(), lower.l2(), lower.l3()), (1, 0, 129));
        assert_eq!((upper.l1(), upper.l2(), upper.l3()), (1, 0, 129));
    }

    #[test]
    fn rejects_gap_between_lower_and_upper_regions() {
        assert_eq!(
            TranslationIndices::new(VirtAddr::new(1_usize << 40)),
            Err(PagingError::NonCanonicalVirtualAddress {
                address: 1_usize << 40,
            })
        );
    }

    #[test]
    fn mapping_sizes_match_three_level_four_kibibyte_translation() {
        assert_eq!(TranslationLevel::L1.mapping_size(), 1 << 30);
        assert_eq!(TranslationLevel::L2.mapping_size(), 1 << 21);
        assert_eq!(TranslationLevel::L3.mapping_size(), PAGE_SIZE);
    }

    #[test]
    fn rejects_writable_executable_and_wrong_domain_mappings() {
        assert_eq!(
            MappingAttributes::new(MemoryType::Normal, Access::KernelReadWrite, Execute::Kernel,),
            Err(PagingError::WritableExecutable)
        );
        assert_eq!(
            MappingAttributes::new(MemoryType::Normal, Access::KernelReadOnly, Execute::User,),
            Err(PagingError::InvalidExecuteDomain)
        );
    }

    #[test]
    fn descriptor_encodes_output_access_and_execute_bits() {
        let descriptor = Descriptor::leaf(
            TranslationLevel::L3,
            PhysAddr::new(0x1234_5000),
            MappingAttributes::user_code(),
        )
        .expect("valid page descriptor");

        assert!(descriptor.is_valid());
        assert_eq!(descriptor.output_address(), PhysAddr::new(0x1234_5000));
        assert_eq!(descriptor.raw() & 0b11, 0b11);
        assert_ne!(descriptor.raw() & (1 << 53), 0);
        assert_eq!(descriptor.raw() & (1 << 54), 0);
        assert_ne!(descriptor.raw() & (1 << 11), 0);
    }

    #[test]
    fn block_and_page_descriptor_types_differ() {
        let block = Descriptor::leaf(
            TranslationLevel::L2,
            PhysAddr::new(0x4000_0000),
            MappingAttributes::kernel_data(),
        )
        .expect("valid block descriptor");
        let page = Descriptor::leaf(
            TranslationLevel::L3,
            PhysAddr::new(0x4000_0000),
            MappingAttributes::kernel_data(),
        )
        .expect("valid page descriptor");

        assert_eq!(block.raw() & 0b11, 0b01);
        assert_eq!(page.raw() & 0b11, 0b11);
    }

    #[test]
    fn table_descriptor_points_to_an_aligned_frame() {
        let frame = PageFrame::from_start(PhysAddr::new(0x8080_0000)).expect("aligned frame");
        let descriptor = Descriptor::table(frame).expect("valid table descriptor");

        assert_eq!(descriptor.raw() & 0b11, 0b11);
        assert_eq!(descriptor.output_address(), PhysAddr::new(0x8080_0000));
    }

    #[test]
    fn rejects_misaligned_or_too_wide_physical_outputs() {
        assert_eq!(
            Descriptor::leaf(
                TranslationLevel::L2,
                PhysAddr::new(0x4000_1000),
                MappingAttributes::kernel_data(),
            ),
            Err(PagingError::MisalignedOutputAddress {
                address: 0x4000_1000,
                alignment: 1 << 21,
            })
        );
        assert_eq!(
            Descriptor::leaf(
                TranslationLevel::L3,
                PhysAddr::new(1_usize << 48),
                MappingAttributes::kernel_data(),
            ),
            Err(PagingError::PhysicalAddressTooWide {
                address: 1_usize << 48,
            })
        );
    }

    #[test]
    fn mapping_plan_sorts_translates_and_unmaps() {
        let mut plan = MappingPlan::<4>::new();
        plan.map(
            VirtAddr::new(0xffff_ff80_4020_0000),
            PhysAddr::new(0x4020_0000),
            TranslationLevel::L2,
            MappingAttributes::kernel_data(),
        )
        .expect("later mapping");
        plan.map(
            VirtAddr::new(0xffff_ff80_4000_0000),
            PhysAddr::new(0x4000_0000),
            TranslationLevel::L2,
            MappingAttributes::kernel_code(),
        )
        .expect("earlier mapping");

        assert_eq!(
            plan.entries()[0].virtual_start(),
            VirtAddr::new(0xffff_ff80_4000_0000)
        );
        assert_eq!(
            plan.translate(VirtAddr::new(0xffff_ff80_4020_1234)),
            Ok(Some(Translation {
                physical_address: PhysAddr::new(0x4020_1234),
                level: TranslationLevel::L2,
                attributes: MappingAttributes::kernel_data(),
            }))
        );
        assert!(
            plan.unmap(VirtAddr::new(0xffff_ff80_4020_0000), TranslationLevel::L2)
                .is_some()
        );
        assert_eq!(
            plan.translate(VirtAddr::new(0xffff_ff80_4020_1234)),
            Ok(None)
        );
    }

    #[test]
    fn mapping_plan_rejects_overlap_alignment_and_capacity() {
        let mut plan = MappingPlan::<1>::new();
        plan.map(
            VirtAddr::new(0x4000_0000),
            PhysAddr::new(0x4000_0000),
            TranslationLevel::L2,
            MappingAttributes::kernel_data(),
        )
        .expect("first mapping");

        assert_eq!(
            plan.map(
                VirtAddr::new(0x4000_1000),
                PhysAddr::new(0x5000_0000),
                TranslationLevel::L3,
                MappingAttributes::kernel_data(),
            ),
            Err(PagingError::AlreadyMapped)
        );
        assert_eq!(
            plan.map(
                VirtAddr::new(0x4020_1000),
                PhysAddr::new(0x4020_0000),
                TranslationLevel::L2,
                MappingAttributes::kernel_data(),
            ),
            Err(PagingError::MisalignedMapping {
                virtual_address: 0x4020_1000,
                physical_address: 0x4020_0000,
                alignment: 1 << 21,
            })
        );
        assert_eq!(
            plan.map(
                VirtAddr::new(0x4040_0000),
                PhysAddr::new(0x4040_0000),
                TranslationLevel::L2,
                MappingAttributes::kernel_data(),
            ),
            Err(PagingError::CapacityExceeded)
        );
    }
}
