//! Owned mixed-level translation tables for the AArch64 kernel half.

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

use crate::arch::aarch64::paging::{
    AddressSpaceHalf, Descriptor, MappingAttributes, MappingPlan, PagingError, PlannedMapping,
    TranslationIndices, TranslationLevel,
};
use crate::arch::aarch64::user_page_table::TranslationTableMemory;
use crate::memory::{
    AllocationError, FrameAllocator, FrameRange, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr,
};

/// Failure while planning the final upper-half hierarchy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelPageTableError {
    /// Generic address, overlap, descriptor, or capacity validation failed.
    Paging(PagingError),
    /// A mapping selected the lower TTBR0 half.
    LowerHalfMapping {
        /// Rejected virtual address.
        address: usize,
    },
    /// A kernel mapping would be accessible from EL0.
    UserAccessibleMapping,
    /// A range contained no complete bytes to map.
    EmptyRange,
    /// The byte length was not a multiple of the 4 KiB granule.
    MisalignedLength {
        /// Rejected length.
        length: usize,
    },
    /// The fixed topology cannot describe another intermediate table.
    TableCapacityExceeded,
    /// Direct-map overrides are empty, reversed, overlapping, or not sorted.
    InvalidOverrideOrder,
}

/// Failure while assigning frames to the complete table topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelPageTableAllocationError {
    /// The allocator has fewer frames than the complete topology needs.
    OutOfMemory,
    /// The allocator rejected a frame request.
    Allocator(AllocationError),
    /// An allocated table frame cannot be encoded in a descriptor.
    Paging(PagingError),
}

/// Role of one table page in the upper-half hierarchy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum KernelTranslationTableRole {
    /// Root table installed in `TTBR1_EL1` and indexed at level 1.
    RootL1,
    /// Level-2 table reached through one root entry.
    Level2 {
        /// Parent level-1 index.
        l1: usize,
    },
    /// Level-3 table containing final 4 KiB descriptors.
    Level3 {
        /// Ancestor level-1 index.
        l1: usize,
        /// Parent level-2 index.
        l2: usize,
    },
}

/// One permission override inside an otherwise uniform physical direct map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectMapOverride {
    frames: FrameRange,
    attributes: MappingAttributes,
}

impl DirectMapOverride {
    /// Define a nonempty physical-frame interval with kernel-only attributes.
    pub fn new(
        frames: FrameRange,
        attributes: MappingAttributes,
    ) -> Result<Self, KernelPageTableError> {
        if frames.is_empty() {
            return Err(KernelPageTableError::InvalidOverrideOrder);
        }
        if attributes.access().user_accessible() {
            return Err(KernelPageTableError::UserAccessibleMapping);
        }
        Ok(Self { frames, attributes })
    }

    /// Return the overridden physical-frame interval.
    pub const fn frames(self) -> FrameRange {
        self.frames
    }

    /// Return the replacement kernel mapping attributes.
    pub const fn attributes(self) -> MappingAttributes {
        self.attributes
    }
}

/// Fixed-capacity upper-half topology with mixed block/page leaves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelPageTablePlan<const TABLES: usize, const MAPPINGS: usize> {
    tables: [Option<KernelTranslationTableRole>; TABLES],
    table_count: usize,
    mappings: MappingPlan<MAPPINGS>,
}

impl<const TABLES: usize, const MAPPINGS: usize> KernelPageTablePlan<TABLES, MAPPINGS> {
    /// Create a plan containing the mandatory L1 root.
    pub fn new() -> Result<Self, KernelPageTableError> {
        if TABLES == 0 {
            return Err(KernelPageTableError::TableCapacityExceeded);
        }
        let mut tables = [None; TABLES];
        tables[0] = Some(KernelTranslationTableRole::RootL1);
        Ok(Self {
            tables,
            table_count: 1,
            mappings: MappingPlan::new(),
        })
    }

    /// Add one upper-half block or page transactionally.
    pub fn map(
        &mut self,
        virtual_start: VirtAddr,
        physical_start: PhysAddr,
        level: TranslationLevel,
        attributes: MappingAttributes,
    ) -> Result<(), KernelPageTableError> {
        let mut candidate = *self;
        candidate.map_inner(virtual_start, physical_start, level, attributes)?;
        *self = candidate;
        Ok(())
    }

    /// Map one page-aligned byte range using the largest legal leaf at each address.
    ///
    /// The complete range is committed only when every mapping and intermediate
    /// table fits. This greedy decomposition prefers 1 GiB, then 2 MiB, then
    /// 4 KiB leaves while preserving exact range boundaries.
    pub fn map_range(
        &mut self,
        virtual_start: VirtAddr,
        physical_start: PhysAddr,
        length: usize,
        attributes: MappingAttributes,
    ) -> Result<(), KernelPageTableError> {
        if length == 0 {
            return Err(KernelPageTableError::EmptyRange);
        }
        if !length.is_multiple_of(PAGE_SIZE) {
            return Err(KernelPageTableError::MisalignedLength { length });
        }
        let mut candidate = *self;
        let mut virtual_address = virtual_start.as_usize();
        let mut physical_address = physical_start.as_usize();
        let mut remaining = length;
        while remaining != 0 {
            let level = [
                TranslationLevel::L1,
                TranslationLevel::L2,
                TranslationLevel::L3,
            ]
            .into_iter()
            .find(|level| {
                let size = level.mapping_size();
                remaining >= size
                    && virtual_address.is_multiple_of(size)
                    && physical_address.is_multiple_of(size)
            })
            .expect("page-aligned range always permits an L3 leaf");
            candidate.map_inner(
                VirtAddr::new(virtual_address),
                PhysAddr::new(physical_address),
                level,
                attributes,
            )?;
            let size = level.mapping_size();
            virtual_address = virtual_address
                .checked_add(size)
                .ok_or(KernelPageTableError::Paging(PagingError::AddressOverflow))?;
            physical_address = physical_address
                .checked_add(size)
                .ok_or(KernelPageTableError::Paging(PagingError::AddressOverflow))?;
            remaining -= size;
        }
        *self = candidate;
        Ok(())
    }

    /// Map complete physical frames at one fixed higher-half offset.
    ///
    /// `overrides` must be strictly ordered and disjoint. Intersections split
    /// the default direct map at exact page boundaries; the greedy range
    /// mapper then selects the largest legal leaves for every piece. The
    /// complete physical region is committed atomically.
    pub fn map_direct_frames(
        &mut self,
        frames: FrameRange,
        virtual_offset: usize,
        default_attributes: MappingAttributes,
        overrides: &[DirectMapOverride],
    ) -> Result<(), KernelPageTableError> {
        if default_attributes.access().user_accessible() {
            return Err(KernelPageTableError::UserAccessibleMapping);
        }
        validate_overrides(overrides)?;
        if frames.is_empty() {
            return Ok(());
        }

        let mut candidate = *self;
        let mut current = frames.start_frame_number();
        let end = frames.end_frame_number();
        while current < end {
            let mut boundary = end;
            let mut attributes = default_attributes;
            for replacement in overrides {
                let replacement_start = replacement.frames.start_frame_number();
                let replacement_end = replacement.frames.end_frame_number();
                if replacement_end <= current {
                    continue;
                }
                if replacement_start > current {
                    boundary = boundary.min(replacement_start);
                } else {
                    attributes = replacement.attributes;
                    boundary = boundary.min(replacement_end);
                }
                break;
            }

            let physical_start = current
                .checked_mul(PAGE_SIZE)
                .ok_or(KernelPageTableError::Paging(PagingError::AddressOverflow))?;
            let virtual_start = physical_start
                .checked_add(virtual_offset)
                .ok_or(KernelPageTableError::Paging(PagingError::AddressOverflow))?;
            let frame_count = boundary - current;
            let length = frame_count
                .checked_mul(PAGE_SIZE)
                .ok_or(KernelPageTableError::Paging(PagingError::AddressOverflow))?;
            candidate.map_range(
                VirtAddr::new(virtual_start),
                PhysAddr::new(physical_start),
                length,
                attributes,
            )?;
            current = boundary;
        }
        *self = candidate;
        Ok(())
    }

    /// Return table roles in deterministic parent-before-child order.
    pub fn tables(
        &self,
    ) -> impl ExactSizeIterator<Item = KernelTranslationTableRole> + DoubleEndedIterator + '_ {
        self.tables[..self.table_count]
            .iter()
            .map(|role| role.expect("active kernel table role slot"))
    }

    /// Return leaf mappings ordered by virtual address.
    pub fn mappings(&self) -> &[PlannedMapping] {
        self.mappings.entries()
    }

    /// Translate one address through the offline final-kernel plan.
    pub fn translate(
        &self,
        address: VirtAddr,
    ) -> Result<Option<crate::arch::aarch64::paging::Translation>, PagingError> {
        self.mappings.translate(address)
    }

    /// Return the exact number of required table frames.
    pub const fn required_table_frames(&self) -> usize {
        self.table_count
    }

    /// Assign all table frames as one allocator transaction.
    pub fn allocate<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<AllocatedKernelPageTables<TABLES, MAPPINGS>, KernelPageTableAllocationError> {
        let mut candidate = *allocator;
        let mut frames = [None; TABLES];
        for slot in &mut frames[..self.table_count] {
            let frame = candidate
                .allocate()
                .map_err(KernelPageTableAllocationError::Allocator)?
                .ok_or(KernelPageTableAllocationError::OutOfMemory)?;
            Descriptor::table(frame).map_err(KernelPageTableAllocationError::Paging)?;
            *slot = Some(frame);
        }
        *allocator = candidate;
        Ok(AllocatedKernelPageTables { plan: self, frames })
    }

    fn map_inner(
        &mut self,
        virtual_start: VirtAddr,
        physical_start: PhysAddr,
        level: TranslationLevel,
        attributes: MappingAttributes,
    ) -> Result<(), KernelPageTableError> {
        let indices =
            TranslationIndices::new(virtual_start).map_err(KernelPageTableError::Paging)?;
        if indices.half() != AddressSpaceHalf::Upper {
            return Err(KernelPageTableError::LowerHalfMapping {
                address: virtual_start.as_usize(),
            });
        }
        if attributes.access().user_accessible() {
            return Err(KernelPageTableError::UserAccessibleMapping);
        }
        self.mappings
            .map(virtual_start, physical_start, level, attributes)
            .map_err(KernelPageTableError::Paging)?;
        match level {
            TranslationLevel::L1 => {}
            TranslationLevel::L2 => {
                self.ensure_table(KernelTranslationTableRole::Level2 { l1: indices.l1() })?;
            }
            TranslationLevel::L3 => {
                self.ensure_table(KernelTranslationTableRole::Level2 { l1: indices.l1() })?;
                self.ensure_table(KernelTranslationTableRole::Level3 {
                    l1: indices.l1(),
                    l2: indices.l2(),
                })?;
            }
        }
        Ok(())
    }

    fn ensure_table(
        &mut self,
        role: KernelTranslationTableRole,
    ) -> Result<(), KernelPageTableError> {
        if self.tables().any(|current| current == role) {
            return Ok(());
        }
        if self.table_count == TABLES {
            return Err(KernelPageTableError::TableCapacityExceeded);
        }
        let insertion = self.tables[..self.table_count]
            .partition_point(|current| current.expect("active kernel table role slot") < role);
        self.tables
            .copy_within(insertion..self.table_count, insertion + 1);
        self.tables[insertion] = Some(role);
        self.table_count += 1;
        Ok(())
    }
}

fn validate_overrides(overrides: &[DirectMapOverride]) -> Result<(), KernelPageTableError> {
    let mut previous_end = 0;
    for (index, replacement) in overrides.iter().enumerate() {
        let start = replacement.frames.start_frame_number();
        let end = replacement.frames.end_frame_number();
        if replacement.frames.is_empty() || index != 0 && start < previous_end {
            return Err(KernelPageTableError::InvalidOverrideOrder);
        }
        previous_end = end;
    }
    Ok(())
}

/// One table role paired with its uniquely owned frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocatedKernelTranslationTable {
    role: KernelTranslationTableRole,
    frame: PageFrame,
}

impl AllocatedKernelTranslationTable {
    /// Return this table's hierarchy role.
    pub const fn role(self) -> KernelTranslationTableRole {
        self.role
    }

    /// Return the backing physical frame.
    pub const fn frame(self) -> PageFrame {
        self.frame
    }
}

/// Kind of private table-memory operation that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelTableMaterializationStage {
    /// Clearing every descriptor in one table frame.
    Clear,
    /// Linking a parent table to an intermediate child.
    Link,
    /// Installing a final block or page descriptor.
    Leaf,
}

/// Materialization failure retaining all allocated table frames.
#[derive(Debug, Eq, PartialEq)]
pub struct KernelTableMaterializationError<E, const TABLES: usize, const MAPPINGS: usize> {
    tables: AllocatedKernelPageTables<TABLES, MAPPINGS>,
    stage: KernelTableMaterializationStage,
    table: PageFrame,
    index: Option<usize>,
    error: E,
}

impl<E, const TABLES: usize, const MAPPINGS: usize>
    KernelTableMaterializationError<E, TABLES, MAPPINGS>
{
    /// Return the operation category that failed.
    pub const fn stage(&self) -> KernelTableMaterializationStage {
        self.stage
    }

    /// Return the table frame being modified.
    pub const fn table(&self) -> PageFrame {
        self.table
    }

    /// Return the descriptor index, or `None` for a clear failure.
    pub const fn index(&self) -> Option<usize> {
        self.index
    }

    /// Return the backend-specific failure.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover table ownership and the backend failure.
    pub fn into_parts(self) -> (AllocatedKernelPageTables<TABLES, MAPPINGS>, E) {
        (self.tables, self.error)
    }
}

/// Allocated but not yet completely initialized upper-half tables.
#[derive(Debug, Eq, PartialEq)]
pub struct AllocatedKernelPageTables<const TABLES: usize, const MAPPINGS: usize> {
    plan: KernelPageTablePlan<TABLES, MAPPINGS>,
    frames: [Option<PageFrame>; TABLES],
}

impl<const TABLES: usize, const MAPPINGS: usize> AllocatedKernelPageTables<TABLES, MAPPINGS> {
    /// Return roles and frames in deterministic parent-before-child order.
    pub fn tables(
        &self,
    ) -> impl ExactSizeIterator<Item = AllocatedKernelTranslationTable> + DoubleEndedIterator + '_
    {
        self.plan
            .tables()
            .enumerate()
            .map(|(index, role)| AllocatedKernelTranslationTable {
                role,
                frame: self.frames[index].expect("allocated kernel table frame slot"),
            })
    }

    /// Clear and populate every private table, publishing no system register.
    pub fn materialize<M: TranslationTableMemory>(
        self,
        memory: &mut M,
    ) -> Result<
        MaterializedKernelPageTables<TABLES, MAPPINGS>,
        KernelTableMaterializationError<M::Error, TABLES, MAPPINGS>,
    > {
        for table_index in 0..self.plan.table_count {
            let frame = self.frames[table_index].expect("allocated kernel table frame slot");
            if let Err(error) = memory.clear_table(frame) {
                return Err(KernelTableMaterializationError {
                    tables: self,
                    stage: KernelTableMaterializationStage::Clear,
                    table: frame,
                    index: None,
                    error,
                });
            }
        }

        for table_index in 1..self.plan.table_count {
            let role = self.plan.tables[table_index].expect("active kernel table role slot");
            let child = self.frames[table_index].expect("allocated kernel table frame slot");
            let (parent_role, entry) = match role {
                KernelTranslationTableRole::RootL1 => unreachable!("root is first and unique"),
                KernelTranslationTableRole::Level2 { l1 } => {
                    (KernelTranslationTableRole::RootL1, l1)
                }
                KernelTranslationTableRole::Level3 { l1, l2 } => {
                    (KernelTranslationTableRole::Level2 { l1 }, l2)
                }
            };
            let parent = self.frame_for(parent_role);
            let descriptor =
                Descriptor::table(child).expect("table frames were validated at allocation");
            if let Err(error) = memory.write_descriptor(parent, entry, descriptor) {
                return Err(KernelTableMaterializationError {
                    tables: self,
                    stage: KernelTableMaterializationStage::Link,
                    table: parent,
                    index: Some(entry),
                    error,
                });
            }
        }

        for mapping in self.plan.mappings() {
            let indices = TranslationIndices::new(mapping.virtual_start())
                .expect("planned mapping remains canonical");
            let (role, entry) = match mapping.level() {
                TranslationLevel::L1 => (KernelTranslationTableRole::RootL1, indices.l1()),
                TranslationLevel::L2 => (
                    KernelTranslationTableRole::Level2 { l1: indices.l1() },
                    indices.l2(),
                ),
                TranslationLevel::L3 => (
                    KernelTranslationTableRole::Level3 {
                        l1: indices.l1(),
                        l2: indices.l2(),
                    },
                    indices.l3(),
                ),
            };
            let table = self.frame_for(role);
            if let Err(error) = memory.write_descriptor(table, entry, mapping.descriptor()) {
                return Err(KernelTableMaterializationError {
                    tables: self,
                    stage: KernelTableMaterializationStage::Leaf,
                    table,
                    index: Some(entry),
                    error,
                });
            }
        }

        Ok(MaterializedKernelPageTables { tables: self })
    }

    /// Release every unpublished table frame transactionally.
    pub fn release<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<(), (Self, AllocationError)> {
        let mut candidate = *allocator;
        for frame in self.frames[..self.plan.table_count].iter().rev().flatten() {
            if let Err(error) = candidate.deallocate(*frame) {
                return Err((self, error));
            }
        }
        *allocator = candidate;
        Ok(())
    }

    fn frame_for(&self, role: KernelTranslationTableRole) -> PageFrame {
        let index = self
            .plan
            .tables()
            .position(|current| current == role)
            .expect("planned parent table exists");
        self.frames[index].expect("allocated kernel table frame slot")
    }
}

/// Completely materialized but unpublished final kernel hierarchy.
#[derive(Debug, Eq, PartialEq)]
pub struct MaterializedKernelPageTables<const TABLES: usize, const MAPPINGS: usize> {
    tables: AllocatedKernelPageTables<TABLES, MAPPINGS>,
}

impl<const TABLES: usize, const MAPPINGS: usize> MaterializedKernelPageTables<TABLES, MAPPINGS> {
    /// Return the physical L1 root intended for `TTBR1_EL1`.
    pub fn root_frame(&self) -> PageFrame {
        self.tables.frame_for(KernelTranslationTableRole::RootL1)
    }

    /// Return all owned table frames for inspection and accounting.
    pub fn tables(
        &self,
    ) -> impl ExactSizeIterator<Item = AllocatedKernelTranslationTable> + DoubleEndedIterator + '_
    {
        self.tables.tables()
    }

    /// Return the exact final mixed-level mappings.
    pub fn mappings(&self) -> &[PlannedMapping] {
        self.tables.plan.mappings()
    }

    /// Replace `TTBR1_EL1` with this permanently retained hierarchy.
    ///
    /// This deliberately leaves `TTBR0_EL1` unchanged. The bootstrap identity
    /// map remains available until the user-address-space activation path
    /// replaces TTBR0 independently.
    ///
    /// # Safety
    ///
    /// The caller must prove that this value remains alive forever and that
    /// its mappings cover the current PC, stack, exception vectors, all live
    /// kernel references, and any MMIO accessed after the switch. Table frames
    /// and every mapped physical object must remain valid. This operation is
    /// currently restricted to the single boot CPU with exceptions masked.
    #[cfg(target_arch = "aarch64")]
    pub unsafe fn activate(&'static self) {
        let root = self.root_frame().start_address().as_usize();
        // SAFETY: the caller supplies a complete, permanently owned TTBR1
        // hierarchy. The store barrier publishes descriptor writes before the
        // base-register update; the broadcast invalidation and final barriers
        // prevent stale translations from surviving publication.
        unsafe {
            asm!(
                "dsb ishst",
                "msr ttbr1_el1, {root}",
                "isb",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                root = in(reg) root,
                options(nostack, preserves_flags)
            );
        }
    }

    /// Release tables that have never been installed in a live translation regime.
    pub fn release_unpublished<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<(), (Self, AllocationError)> {
        match self.tables.release(allocator) {
            Ok(()) => Ok(()),
            Err((tables, error)) => Err((Self { tables }, error)),
        }
    }
}

const _: () = assert!(PAGE_SIZE == 4096);

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use super::{
        AllocatedKernelTranslationTable, DirectMapOverride, KernelPageTableError,
        KernelPageTablePlan, KernelTableMaterializationStage, KernelTranslationTableRole,
    };
    use crate::arch::aarch64::paging::{
        Descriptor, MappingAttributes, PagingError, TranslationLevel,
    };
    use crate::arch::aarch64::user_page_table::TranslationTableMemory;
    use crate::memory::{
        AddressRange, FrameRange, MemoryMap, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr,
    };

    const OFFSET: usize = 0xffff_ff80_0000_0000;

    #[test]
    fn greedily_decomposes_ranges_at_exact_boundaries() {
        let mut plan = KernelPageTablePlan::<4, 3>::new().unwrap();
        plan.map_range(
            VirtAddr::new(OFFSET + 0x401f_f000),
            PhysAddr::new(0x401f_f000),
            (1 << 21) + 2 * PAGE_SIZE,
            MappingAttributes::kernel_data(),
        )
        .unwrap();

        let mappings = plan.mappings();
        assert_eq!(mappings.len(), 3);
        assert_eq!(mappings[0].level(), TranslationLevel::L3);
        assert_eq!(mappings[1].level(), TranslationLevel::L2);
        assert_eq!(mappings[2].level(), TranslationLevel::L3);
        assert_eq!(plan.required_table_frames(), 4);
    }

    #[test]
    fn combines_device_pages_and_normal_blocks_in_order() {
        let mut plan = KernelPageTablePlan::<4, 3>::new().unwrap();
        plan.map_range(
            VirtAddr::new(OFFSET + 0x4000_0000),
            PhysAddr::new(0x4000_0000),
            2 * (1 << 21),
            MappingAttributes::kernel_data(),
        )
        .unwrap();
        plan.map(
            VirtAddr::new(OFFSET + 0x0900_0000),
            PhysAddr::new(0x0900_0000),
            TranslationLevel::L3,
            MappingAttributes::kernel_device(),
        )
        .unwrap();

        assert_eq!(
            plan.tables().collect::<Vec<_>>(),
            [
                KernelTranslationTableRole::RootL1,
                KernelTranslationTableRole::Level2 { l1: 0 },
                KernelTranslationTableRole::Level2 { l1: 1 },
                KernelTranslationTableRole::Level3 { l1: 0, l2: 72 },
            ]
        );
        assert_eq!(
            plan.mappings()[0].physical_start(),
            PhysAddr::new(0x0900_0000)
        );
        assert_eq!(plan.mappings()[1].level(), TranslationLevel::L2);
    }

    #[test]
    fn direct_map_splits_kernel_permissions_without_losing_large_blocks() {
        let text = DirectMapOverride::new(
            frame_range(0x4008_0000, 2),
            MappingAttributes::kernel_code(),
        )
        .unwrap();
        let rodata = DirectMapOverride::new(
            frame_range(0x4008_2000, 1),
            MappingAttributes::kernel_rodata(),
        )
        .unwrap();
        let mut plan = KernelPageTablePlan::<3, 513>::new().unwrap();
        plan.map_direct_frames(
            frame_range(0x4000_0000, 1024),
            OFFSET,
            MappingAttributes::kernel_data(),
            &[text, rodata],
        )
        .unwrap();

        assert_eq!(plan.required_table_frames(), 3);
        assert_eq!(plan.mappings().len(), 513);
        let ordinary = plan
            .translate(VirtAddr::new(OFFSET + 0x4000_0000))
            .unwrap()
            .unwrap();
        let executable = plan
            .translate(VirtAddr::new(OFFSET + 0x4008_1000))
            .unwrap()
            .unwrap();
        let readonly = plan
            .translate(VirtAddr::new(OFFSET + 0x4008_2000))
            .unwrap()
            .unwrap();
        let large = plan
            .translate(VirtAddr::new(OFFSET + 0x4021_2345))
            .unwrap()
            .unwrap();
        assert_eq!(ordinary.attributes, MappingAttributes::kernel_data());
        assert_eq!(ordinary.level, TranslationLevel::L3);
        assert_eq!(executable.attributes, MappingAttributes::kernel_code());
        assert_eq!(readonly.attributes, MappingAttributes::kernel_rodata());
        assert_eq!(large.attributes, MappingAttributes::kernel_data());
        assert_eq!(large.level, TranslationLevel::L2);
        assert_eq!(large.physical_address, PhysAddr::new(0x4021_2345));
    }

    #[test]
    fn direct_map_rejects_invalid_overrides_transactionally() {
        let mut plan = KernelPageTablePlan::<3, 4>::new().unwrap();
        let before = plan;
        let later = DirectMapOverride::new(
            frame_range(0x4000_2000, 2),
            MappingAttributes::kernel_code(),
        )
        .unwrap();
        let earlier = DirectMapOverride::new(
            frame_range(0x4000_1000, 2),
            MappingAttributes::kernel_rodata(),
        )
        .unwrap();

        assert_eq!(
            plan.map_direct_frames(
                frame_range(0x4000_0000, 4),
                OFFSET,
                MappingAttributes::kernel_data(),
                &[later, earlier],
            ),
            Err(KernelPageTableError::InvalidOverrideOrder)
        );
        assert_eq!(plan, before);
        assert_eq!(
            DirectMapOverride::new(frame_range(0x5000_0000, 1), MappingAttributes::user_data(),),
            Err(KernelPageTableError::UserAccessibleMapping)
        );
    }

    #[test]
    fn rejects_lower_user_overlap_alignment_and_capacity_transactionally() {
        let mut plan = KernelPageTablePlan::<2, 1>::new().unwrap();
        assert_eq!(
            plan.map(
                VirtAddr::new(0x4000_0000),
                PhysAddr::new(0x4000_0000),
                TranslationLevel::L2,
                MappingAttributes::kernel_data()
            ),
            Err(KernelPageTableError::LowerHalfMapping {
                address: 0x4000_0000
            })
        );
        assert_eq!(
            plan.map(
                VirtAddr::new(OFFSET + 0x4000_0000),
                PhysAddr::new(0x4000_0000),
                TranslationLevel::L2,
                MappingAttributes::user_data()
            ),
            Err(KernelPageTableError::UserAccessibleMapping)
        );
        assert_eq!(
            plan.map_range(
                VirtAddr::new(OFFSET + 0x4000_0000),
                PhysAddr::new(0x4000_0000),
                PAGE_SIZE + 1,
                MappingAttributes::kernel_data()
            ),
            Err(KernelPageTableError::MisalignedLength {
                length: PAGE_SIZE + 1
            })
        );
        plan.map(
            VirtAddr::new(OFFSET + 0x4000_0000),
            PhysAddr::new(0x4000_0000),
            TranslationLevel::L2,
            MappingAttributes::kernel_data(),
        )
        .unwrap();
        let before = plan;
        assert_eq!(
            plan.map(
                VirtAddr::new(OFFSET + 0x4000_0000),
                PhysAddr::new(0x5000_0000),
                TranslationLevel::L2,
                MappingAttributes::kernel_data()
            ),
            Err(KernelPageTableError::Paging(PagingError::AlreadyMapped))
        );
        assert_eq!(plan, before);
        assert_eq!(
            plan.map(
                VirtAddr::new(OFFSET + 0x4020_0000),
                PhysAddr::new(0x4020_0000),
                TranslationLevel::L2,
                MappingAttributes::kernel_data()
            ),
            Err(KernelPageTableError::Paging(PagingError::CapacityExceeded))
        );
        assert_eq!(plan, before);
    }

    #[test]
    fn materializes_links_and_mixed_leaves_then_releases_atomically() {
        let mut plan = KernelPageTablePlan::<4, 2>::new().unwrap();
        plan.map(
            VirtAddr::new(OFFSET + 0x4000_0000),
            PhysAddr::new(0x4000_0000),
            TranslationLevel::L2,
            MappingAttributes::kernel_data(),
        )
        .unwrap();
        plan.map(
            VirtAddr::new(OFFSET + 0x0900_0000),
            PhysAddr::new(0x0900_0000),
            TranslationLevel::L3,
            MappingAttributes::kernel_device(),
        )
        .unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(range(0x6000_0000, 0x6000_4000)).unwrap();
        let mut allocator = map.into_allocator();
        let original = allocator;
        let allocated = plan.allocate(&mut allocator).unwrap();
        let frames: Vec<_> = allocated.tables().collect();
        let mut memory = TestMemory::new(&frames);

        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.root_frame();
        let ram_l2 = frame_for(&frames, KernelTranslationTableRole::Level2 { l1: 1 });
        let device_l2 = frame_for(&frames, KernelTranslationTableRole::Level2 { l1: 0 });
        let device_l3 = frame_for(
            &frames,
            KernelTranslationTableRole::Level3 { l1: 0, l2: 72 },
        );
        assert_eq!(
            memory.descriptor(root, 1).output_address(),
            ram_l2.start_address()
        );
        assert_eq!(
            memory.descriptor(ram_l2, 0).output_address(),
            PhysAddr::new(0x4000_0000)
        );
        assert_eq!(
            memory.descriptor(root, 0).output_address(),
            device_l2.start_address()
        );
        assert_eq!(
            memory.descriptor(device_l2, 72).output_address(),
            device_l3.start_address()
        );
        assert_eq!(
            memory.descriptor(device_l3, 0).output_address(),
            PhysAddr::new(0x0900_0000)
        );
        materialized.release_unpublished(&mut allocator).unwrap();
        assert_eq!(allocator, original);
    }

    #[test]
    fn materialization_failure_returns_retryable_ownership() {
        let mut plan = KernelPageTablePlan::<2, 1>::new().unwrap();
        plan.map(
            VirtAddr::new(OFFSET + 0x4000_0000),
            PhysAddr::new(0x4000_0000),
            TranslationLevel::L2,
            MappingAttributes::kernel_data(),
        )
        .unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(range(0x6100_0000, 0x6100_2000)).unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let frames: Vec<_> = allocated.tables().collect();
        let mut failing = TestMemory::new(&frames);
        failing.fail_write = Some(0);

        let error = allocated.materialize(&mut failing).unwrap_err();
        assert_eq!(error.stage(), KernelTableMaterializationStage::Link);
        let (allocated, TestMemoryError::Injected) = error.into_parts() else {
            panic!("expected injected backend failure")
        };
        let retry_frames: Vec<_> = allocated.tables().collect();
        let mut retry = TestMemory::new(&retry_frames);
        let materialized = allocated.materialize(&mut retry).unwrap();
        materialized.release_unpublished(&mut allocator).unwrap();
    }

    fn range(start: usize, end: usize) -> AddressRange<PhysAddr> {
        AddressRange::new(PhysAddr::new(start), PhysAddr::new(end)).unwrap()
    }

    fn frame_for(
        frames: &[AllocatedKernelTranslationTable],
        role: KernelTranslationTableRole,
    ) -> PageFrame {
        frames
            .iter()
            .find(|table| table.role() == role)
            .unwrap()
            .frame()
    }

    fn frame_range(start: usize, frame_count: usize) -> FrameRange {
        FrameRange::new(
            PageFrame::from_start(PhysAddr::new(start)).unwrap(),
            frame_count,
        )
        .unwrap()
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestMemoryError {
        UnknownFrame,
        InvalidIndex,
        Injected,
    }

    struct TestMemory {
        frames: Vec<PageFrame>,
        entries: Vec<Vec<Descriptor>>,
        write_count: usize,
        fail_write: Option<usize>,
    }

    impl TestMemory {
        fn new(tables: &[AllocatedKernelTranslationTable]) -> Self {
            Self {
                frames: tables.iter().map(|table| table.frame()).collect(),
                entries: vec![vec![Descriptor::invalid(); 512]; tables.len()],
                write_count: 0,
                fail_write: None,
            }
        }

        fn frame_index(&self, frame: PageFrame) -> Result<usize, TestMemoryError> {
            self.frames
                .iter()
                .position(|candidate| *candidate == frame)
                .ok_or(TestMemoryError::UnknownFrame)
        }

        fn descriptor(&self, frame: PageFrame, index: usize) -> Descriptor {
            self.entries[self.frame_index(frame).unwrap()][index]
        }
    }

    impl TranslationTableMemory for TestMemory {
        type Error = TestMemoryError;

        fn clear_table(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
            let index = self.frame_index(frame)?;
            self.entries[index].fill(Descriptor::invalid());
            Ok(())
        }

        fn write_descriptor(
            &mut self,
            frame: PageFrame,
            index: usize,
            descriptor: Descriptor,
        ) -> Result<(), Self::Error> {
            if self.write_count == self.fail_write.unwrap_or(usize::MAX) {
                return Err(TestMemoryError::Injected);
            }
            self.write_count += 1;
            let frame_index = self.frame_index(frame)?;
            let slot = self.entries[frame_index]
                .get_mut(index)
                .ok_or(TestMemoryError::InvalidIndex)?;
            *slot = descriptor;
            Ok(())
        }
    }
}
