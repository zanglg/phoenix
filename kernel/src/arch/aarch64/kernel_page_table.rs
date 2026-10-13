//! Owned mixed-level translation tables for the AArch64 kernel half.

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

use crate::arch::aarch64::paging::{
    AddressSpaceHalf, Descriptor, MappingAttributes, MappingPlan, PagingError, PlannedMapping,
    TranslationIndices, TranslationLevel,
};
use crate::arch::aarch64::user_page_table::TranslationTableMemory;
use crate::memory::{
    AllocationError, FrameAllocator, FrameRange, KernelStackLayout, KernelStackMappingBackend,
    OwnedKernelStack, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr,
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
    /// A range start was not aligned to the 4 KiB translation granule.
    MisalignedVirtualStart {
        /// Rejected virtual address.
        address: usize,
    },
    /// A leaf mapping and reserved intermediate-table topology would overlap.
    TableLeafConflict {
        /// First conflicting virtual address.
        address: usize,
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

/// One mapped or deliberately absent interval inside a physical direct map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectMapOverride {
    frames: FrameRange,
    attributes: Option<MappingAttributes>,
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
        Ok(Self {
            frames,
            attributes: Some(attributes),
        })
    }

    /// Define a nonempty physical-frame interval omitted from the direct map.
    pub fn unmapped(frames: FrameRange) -> Result<Self, KernelPageTableError> {
        if frames.is_empty() {
            return Err(KernelPageTableError::InvalidOverrideOrder);
        }
        Ok(Self {
            frames,
            attributes: None,
        })
    }

    /// Return the overridden physical-frame interval.
    pub const fn frames(self) -> FrameRange {
        self.frames
    }

    /// Return replacement attributes, or `None` for a deliberate mapping hole.
    pub const fn attributes(self) -> Option<MappingAttributes> {
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

    /// Consume this unpublished plan and add one mapping without a rollback copy.
    ///
    /// This is intended for single-owner boot builders that discard the whole
    /// plan on failure and need to keep early-stack usage bounded.
    pub fn with_mapping(
        mut self,
        virtual_start: VirtAddr,
        physical_start: PhysAddr,
        level: TranslationLevel,
        attributes: MappingAttributes,
    ) -> Result<Self, KernelPageTableError> {
        self.map_inner(virtual_start, physical_start, level, attributes)?;
        Ok(self)
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
        candidate.map_range_inner(virtual_start, physical_start, length, attributes)?;
        *self = candidate;
        Ok(())
    }

    /// Reserve linked L2/L3 topology for an upper-half range without mapping leaves.
    ///
    /// The complete reservation is committed only if every covered page is
    /// canonical, currently unmapped, and representable by the fixed table
    /// capacity. Existing identical empty topology is accepted. Later L3 maps
    /// may populate individual pages, but coarser leaves cannot replace any
    /// reserved child table.
    pub fn reserve_l3_range(
        &mut self,
        virtual_start: VirtAddr,
        length: usize,
    ) -> Result<(), KernelPageTableError> {
        let mut candidate = *self;
        candidate.reserve_l3_range_inner(virtual_start, length)?;
        *self = candidate;
        Ok(())
    }

    /// Consume an unpublished plan and reserve empty L3 topology without a rollback copy.
    pub fn with_l3_range_reservation(
        mut self,
        virtual_start: VirtAddr,
        length: usize,
    ) -> Result<Self, KernelPageTableError> {
        self.reserve_l3_range_inner(virtual_start, length)?;
        Ok(self)
    }

    fn reserve_l3_range_inner(
        &mut self,
        virtual_start: VirtAddr,
        length: usize,
    ) -> Result<(), KernelPageTableError> {
        if length == 0 {
            return Err(KernelPageTableError::EmptyRange);
        }
        if !length.is_multiple_of(PAGE_SIZE) {
            return Err(KernelPageTableError::MisalignedLength { length });
        }
        let start = virtual_start.as_usize();
        if !start.is_multiple_of(PAGE_SIZE) {
            return Err(KernelPageTableError::MisalignedVirtualStart { address: start });
        }
        let end = start
            .checked_add(length)
            .ok_or(KernelPageTableError::Paging(PagingError::AddressOverflow))?;

        let mut address = start;
        while address < end {
            let virtual_address = VirtAddr::new(address);
            let indices =
                TranslationIndices::new(virtual_address).map_err(KernelPageTableError::Paging)?;
            if indices.half() != AddressSpaceHalf::Upper {
                return Err(KernelPageTableError::LowerHalfMapping { address });
            }
            if self
                .translate(virtual_address)
                .map_err(KernelPageTableError::Paging)?
                .is_some()
            {
                return Err(KernelPageTableError::TableLeafConflict { address });
            }
            self.ensure_table(KernelTranslationTableRole::Level2 { l1: indices.l1() })?;
            self.ensure_table(KernelTranslationTableRole::Level3 {
                l1: indices.l1(),
                l2: indices.l2(),
            })?;
            address = address
                .checked_add(PAGE_SIZE)
                .ok_or(KernelPageTableError::Paging(PagingError::AddressOverflow))?;
        }
        Ok(())
    }

    fn map_range_inner(
        &mut self,
        virtual_start: VirtAddr,
        physical_start: PhysAddr,
        length: usize,
        attributes: MappingAttributes,
    ) -> Result<(), KernelPageTableError> {
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
            self.map_inner(
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
        Ok(())
    }

    /// Map complete physical frames at one fixed higher-half offset.
    ///
    /// `overrides` must be strictly ordered and disjoint. Intersections split
    /// the default direct map at exact page boundaries; an override may replace
    /// permissions or deliberately leave a hole. The greedy range mapper then
    /// selects the largest legal leaves for every mapped piece. The complete
    /// physical region is committed atomically.
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
        candidate.map_direct_frames_inner(frames, virtual_offset, default_attributes, overrides)?;
        *self = candidate;
        Ok(())
    }

    /// Consume an unpublished plan and append a direct map without a rollback copy.
    ///
    /// The caller receives no partially changed plan on failure, making this
    /// form appropriate for the large fixed-capacity early-boot builder.
    pub fn with_direct_frames(
        mut self,
        frames: FrameRange,
        virtual_offset: usize,
        default_attributes: MappingAttributes,
        overrides: &[DirectMapOverride],
    ) -> Result<Self, KernelPageTableError> {
        if default_attributes.access().user_accessible() {
            return Err(KernelPageTableError::UserAccessibleMapping);
        }
        validate_overrides(overrides)?;
        self.map_direct_frames_inner(frames, virtual_offset, default_attributes, overrides)?;
        Ok(self)
    }

    fn map_direct_frames_inner(
        &mut self,
        frames: FrameRange,
        virtual_offset: usize,
        default_attributes: MappingAttributes,
        overrides: &[DirectMapOverride],
    ) -> Result<(), KernelPageTableError> {
        if frames.is_empty() {
            return Ok(());
        }
        let mut current = frames.start_frame_number();
        let end = frames.end_frame_number();
        while current < end {
            let mut boundary = end;
            let mut attributes = Some(default_attributes);
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
            if let Some(attributes) = attributes {
                self.map_range_inner(
                    VirtAddr::new(virtual_start),
                    PhysAddr::new(physical_start),
                    length,
                    attributes,
                )?;
            }
            current = boundary;
        }
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
        let topology_conflict = match level {
            TranslationLevel::L1 => self.tables().any(|role| {
                matches!(role, KernelTranslationTableRole::Level2 { l1 } if l1 == indices.l1())
            }),
            TranslationLevel::L2 => self.tables().any(|role| {
                matches!(
                    role,
                    KernelTranslationTableRole::Level3 { l1, l2 }
                        if l1 == indices.l1() && l2 == indices.l2()
                )
            }),
            TranslationLevel::L3 => false,
        };
        if topology_conflict {
            return Err(KernelPageTableError::TableLeafConflict {
                address: virtual_start.as_usize(),
            });
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
        MaterializedKernelPageTables<TABLES>,
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

        let mut tables = [None; TABLES];
        for (index, slot) in tables[..self.plan.table_count].iter_mut().enumerate() {
            *slot = Some(AllocatedKernelTranslationTable {
                role: self.plan.tables[index].expect("active kernel table role slot"),
                frame: self.frames[index].expect("allocated kernel table frame slot"),
            });
        }
        Ok(MaterializedKernelPageTables {
            tables,
            table_count: self.plan.table_count,
        })
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
pub struct MaterializedKernelPageTables<const TABLES: usize> {
    tables: [Option<AllocatedKernelTranslationTable>; TABLES],
    table_count: usize,
}

impl<const TABLES: usize> MaterializedKernelPageTables<TABLES> {
    /// Return the physical L1 root intended for `TTBR1_EL1`.
    pub fn root_frame(&self) -> PageFrame {
        self.tables[0]
            .expect("materialized kernel root table slot")
            .frame()
    }

    /// Return all owned table frames for inspection and accounting.
    pub fn tables(
        &self,
    ) -> impl ExactSizeIterator<Item = AllocatedKernelTranslationTable> + DoubleEndedIterator + '_
    {
        self.tables[..self.table_count]
            .iter()
            .map(|table| table.expect("active materialized kernel table slot"))
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
        let mut candidate = *allocator;
        for table in self.tables[..self.table_count].iter().rev().flatten() {
            if let Err(error) = candidate.deallocate(table.frame()) {
                return Err((self, error));
            }
        }
        *allocator = candidate;
        Ok(())
    }

    fn frame_for_role(&self, role: KernelTranslationTableRole) -> Option<PageFrame> {
        self.tables()
            .find(|table| table.role() == role)
            .map(AllocatedKernelTranslationTable::frame)
    }
}

/// Checked access to descriptor slots in one active kernel-table hierarchy.
///
/// # Safety
///
/// An implementation must read the exact aligned 64-bit descriptor selected by
/// an owned table frame and index. After a successful read, an unsafe write to
/// that same slot must complete as one non-tearing store and cannot fail or
/// panic while the caller preserves the implementation's exclusive-access
/// contract. Implementations must not redirect an owned frame to unrelated
/// memory.
pub unsafe trait ActiveKernelTableMemory {
    /// Backend-specific preflight failure.
    type Error;

    /// Read one descriptor before any transaction writes occur.
    fn read_descriptor(&mut self, frame: PageFrame, index: usize) -> Result<u64, Self::Error>;

    /// Replace one descriptor after all fallible preflight work has succeeded.
    ///
    /// # Safety
    ///
    /// The frame and index must have been successfully read through this same
    /// backend without any intervening access that can revoke or alias the
    /// descriptor slot.
    unsafe fn write_descriptor(&mut self, frame: PageFrame, index: usize, descriptor: Descriptor);
}

/// Visibility and TLB maintenance after a batch of kernel L3 leaf stores.
///
/// # Safety
///
/// Returning must make every preceding descriptor store visible to the active
/// translation regime, invalidate cached translations for every supplied page,
/// and complete the barriers required before code can use a new mapping or
/// release frames from a retired mapping. The caller serializes descriptor
/// mutation against IRQ, preemption, and other CPUs.
pub unsafe trait KernelLeafTlbMaintenance<const PAGES: usize> {
    /// Publish one complete batch of leaf changes.
    fn synchronize(&mut self, pages: &[VirtAddr; PAGES]);
}

/// Failure while publishing or retiring one guarded AArch64 kernel stack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Aarch64KernelStackMapError<E> {
    /// A stack address could not be decoded under the current translation regime.
    Paging(PagingError),
    /// A stack page selected the lower TTBR0 half.
    LowerHalf {
        /// Rejected virtual page.
        address: usize,
    },
    /// The retained hierarchy does not own the required prelinked L3 table.
    MissingLevel3Table {
        /// Virtual page whose table is absent.
        address: usize,
    },
    /// A page that must be invalid already contains a descriptor.
    DescriptorOccupied {
        /// Conflicting virtual page.
        address: usize,
        /// Observed raw descriptor.
        descriptor: u64,
    },
    /// A mapped usable page no longer contains its exact owned descriptor.
    DescriptorMismatch {
        /// Virtual page with unexpected state.
        address: usize,
        /// Required raw descriptor.
        expected: u64,
        /// Observed raw descriptor.
        actual: u64,
    },
    /// The mapping token does not describe this hierarchy and stack owner.
    MappingTokenMismatch,
    /// Descriptor construction rejected an owned physical frame.
    InvalidFrame(PagingError),
    /// Descriptor preflight access failed before any mutation occurred.
    Memory {
        /// Virtual page whose descriptor could not be read.
        address: usize,
        /// Backend-specific failure.
        error: E,
    },
}

/// Exact identity of one published guarded stack mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Aarch64KernelStackMapping<const STACK_PAGES: usize> {
    root: PageFrame,
    layout: KernelStackLayout,
    frames: [PageFrame; STACK_PAGES],
}

#[derive(Clone, Copy)]
struct StackLeafUpdate {
    address: VirtAddr,
    table: PageFrame,
    index: usize,
    descriptor: Descriptor,
}

/// Serialized mapper for prelinked L3 leaves in one active TTBR1 hierarchy.
///
/// All recoverable checks and backend reads finish before the first descriptor
/// store. The commit phase uses only infallible stores followed by one complete
/// maintenance batch, so a returned error never leaves a partial mapping.
pub struct Aarch64KernelStackMapper<'a, M, T, const TABLES: usize, const STACK_PAGES: usize> {
    tables: &'a MaterializedKernelPageTables<TABLES>,
    memory: &'a mut M,
    maintenance: &'a mut T,
}

impl<'a, M, T, const TABLES: usize, const STACK_PAGES: usize>
    Aarch64KernelStackMapper<'a, M, T, TABLES, STACK_PAGES>
{
    /// Bind exclusive descriptor access to the exact active table owner.
    ///
    /// # Safety
    ///
    /// `tables` must be the active, permanently retained TTBR1 hierarchy.
    /// `memory` must address those exact table frames and be uniquely held for
    /// the mapper's lifetime. IRQs and preemption must remain unable to mutate
    /// these descriptors, and no other CPU may access them concurrently.
    pub unsafe fn new(
        tables: &'a MaterializedKernelPageTables<TABLES>,
        memory: &'a mut M,
        maintenance: &'a mut T,
    ) -> Self {
        Self {
            tables,
            memory,
            maintenance,
        }
    }
}

impl<
    M: ActiveKernelTableMemory,
    T: KernelLeafTlbMaintenance<STACK_PAGES>,
    const TABLES: usize,
    const STACK_PAGES: usize,
> Aarch64KernelStackMapper<'_, M, T, TABLES, STACK_PAGES>
{
    fn leaf_location(
        &self,
        address: VirtAddr,
    ) -> Result<(PageFrame, usize), Aarch64KernelStackMapError<M::Error>> {
        let indices =
            TranslationIndices::new(address).map_err(Aarch64KernelStackMapError::Paging)?;
        if indices.half() != AddressSpaceHalf::Upper {
            return Err(Aarch64KernelStackMapError::LowerHalf {
                address: address.as_usize(),
            });
        }
        let role = KernelTranslationTableRole::Level3 {
            l1: indices.l1(),
            l2: indices.l2(),
        };
        let table = self.tables.frame_for_role(role).ok_or(
            Aarch64KernelStackMapError::MissingLevel3Table {
                address: address.as_usize(),
            },
        )?;
        Ok((table, indices.l3()))
    }

    fn read_raw(
        &mut self,
        address: VirtAddr,
    ) -> Result<(PageFrame, usize, u64), Aarch64KernelStackMapError<M::Error>> {
        let (table, index) = self.leaf_location(address)?;
        let raw = self.memory.read_descriptor(table, index).map_err(|error| {
            Aarch64KernelStackMapError::Memory {
                address: address.as_usize(),
                error,
            }
        })?;
        Ok((table, index, raw))
    }

    fn require_invalid(
        &mut self,
        address: VirtAddr,
    ) -> Result<(PageFrame, usize), Aarch64KernelStackMapError<M::Error>> {
        let (table, index, raw) = self.read_raw(address)?;
        if raw != Descriptor::invalid().raw() {
            return Err(Aarch64KernelStackMapError::DescriptorOccupied {
                address: address.as_usize(),
                descriptor: raw,
            });
        }
        Ok((table, index))
    }

    fn preflight_map(
        &mut self,
        stack: &OwnedKernelStack<STACK_PAGES>,
    ) -> Result<[StackLeafUpdate; STACK_PAGES], Aarch64KernelStackMapError<M::Error>> {
        let layout = stack.layout();
        self.require_invalid(layout.lower_guard().start())?;
        self.require_invalid(layout.upper_guard().start())?;
        let mut updates = [None; STACK_PAGES];
        for (slot, (address, frame)) in updates.iter_mut().zip(stack.mappings()) {
            let (table, index) = self.require_invalid(address)?;
            let descriptor = Descriptor::leaf(
                TranslationLevel::L3,
                frame.start_address(),
                MappingAttributes::kernel_data(),
            )
            .map_err(Aarch64KernelStackMapError::InvalidFrame)?;
            *slot = Some(StackLeafUpdate {
                address,
                table,
                index,
                descriptor,
            });
        }
        Ok(updates.map(|update| update.expect("complete owned stack mapping")))
    }

    fn preflight_unmap(
        &mut self,
        stack: &OwnedKernelStack<STACK_PAGES>,
        mapping: Aarch64KernelStackMapping<STACK_PAGES>,
    ) -> Result<[StackLeafUpdate; STACK_PAGES], Aarch64KernelStackMapError<M::Error>> {
        let mut frames = [None; STACK_PAGES];
        for (slot, frame) in frames.iter_mut().zip(stack.frames()) {
            *slot = Some(frame);
        }
        let frames = frames.map(|frame| frame.expect("complete owned stack frames"));
        if mapping.root != self.tables.root_frame()
            || mapping.layout != stack.layout()
            || mapping.frames != frames
        {
            return Err(Aarch64KernelStackMapError::MappingTokenMismatch);
        }

        let layout = stack.layout();
        self.require_invalid(layout.lower_guard().start())?;
        self.require_invalid(layout.upper_guard().start())?;
        let mut updates = [None; STACK_PAGES];
        for (slot, (address, frame)) in updates.iter_mut().zip(stack.mappings()) {
            let (table, index, raw) = self.read_raw(address)?;
            let descriptor = Descriptor::leaf(
                TranslationLevel::L3,
                frame.start_address(),
                MappingAttributes::kernel_data(),
            )
            .map_err(Aarch64KernelStackMapError::InvalidFrame)?;
            if raw != descriptor.raw() {
                return Err(Aarch64KernelStackMapError::DescriptorMismatch {
                    address: address.as_usize(),
                    expected: descriptor.raw(),
                    actual: raw,
                });
            }
            *slot = Some(StackLeafUpdate {
                address,
                table,
                index,
                descriptor,
            });
        }
        Ok(updates.map(|update| update.expect("complete mapped stack")))
    }
}

// SAFETY: construction requires one exact active hierarchy, exclusive table
// access, and serialized execution. All fallible reads and validation precede
// infallible descriptor stores; maintenance completes visibility and TLBI
// before success, so errors preserve the entire prior mapping state.
unsafe impl<
    M: ActiveKernelTableMemory,
    T: KernelLeafTlbMaintenance<STACK_PAGES>,
    const TABLES: usize,
    const STACK_PAGES: usize,
> KernelStackMappingBackend<STACK_PAGES>
    for Aarch64KernelStackMapper<'_, M, T, TABLES, STACK_PAGES>
{
    type Error = Aarch64KernelStackMapError<M::Error>;
    type Mapping = Aarch64KernelStackMapping<STACK_PAGES>;

    unsafe fn map_stack(
        &mut self,
        stack: &OwnedKernelStack<STACK_PAGES>,
    ) -> Result<Self::Mapping, Self::Error> {
        let updates = self.preflight_map(stack)?;
        for update in &updates {
            // SAFETY: preflight successfully read this exact slot through the
            // same exclusively held backend and no intervening access occurs.
            unsafe {
                self.memory
                    .write_descriptor(update.table, update.index, update.descriptor);
            }
        }
        let pages = updates.map(|update| update.address);
        self.maintenance.synchronize(&pages);
        let mut frames = [None; STACK_PAGES];
        for (slot, frame) in frames.iter_mut().zip(stack.frames()) {
            *slot = Some(frame);
        }
        Ok(Aarch64KernelStackMapping {
            root: self.tables.root_frame(),
            layout: stack.layout(),
            frames: frames.map(|frame| frame.expect("complete owned stack frames")),
        })
    }

    unsafe fn unmap_stack(
        &mut self,
        stack: &OwnedKernelStack<STACK_PAGES>,
        mapping: Self::Mapping,
    ) -> Result<(), (Self::Mapping, Self::Error)> {
        let updates = match self.preflight_unmap(stack, mapping) {
            Ok(updates) => updates,
            Err(error) => return Err((mapping, error)),
        };
        for update in &updates {
            // SAFETY: preflight successfully read and matched this exact slot
            // through the same exclusive backend with no intervening access.
            unsafe {
                self.memory
                    .write_descriptor(update.table, update.index, Descriptor::invalid());
            }
        }
        let pages = updates.map(|update| update.address);
        self.maintenance.synchronize(&pages);
        Ok(())
    }
}

/// Publish a permanently owned contiguous RW/NX heap between two invalid pages.
///
/// # Safety
/// The hierarchy must be active on the sole CPU, local IRQs masked, and the
/// caller must permanently retain the backing frames in a unique heap owner.
/// No safe allocation may escape before this function succeeds. No competing
/// mapper may access these leaves, and the backing may never be released while
/// this alias exists. All fallible reads finish before any descriptor changes.
pub unsafe fn publish_permanent_heap<
    M: ActiveKernelTableMemory,
    T: KernelLeafTlbMaintenance<4>,
    const TABLES: usize,
>(
    tables: &MaterializedKernelPageTables<TABLES>,
    memory: &mut M,
    tlb: &mut T,
    backing: crate::memory::FrameRange,
    start: VirtAddr,
) -> Result<(), Aarch64KernelStackMapError<M::Error>> {
    use crate::arch::aarch64::paging::AddressSpaceHalf;
    assert_eq!(backing.len(), 4);
    assert!(start.as_usize().is_multiple_of(PAGE_SIZE));
    let mut updates = [None; 4];
    for index in 0..6 {
        let address = VirtAddr::new(
            start
                .as_usize()
                .checked_sub(PAGE_SIZE)
                .and_then(|base| base.checked_add(index * PAGE_SIZE))
                .ok_or(Aarch64KernelStackMapError::Paging(
                    PagingError::AddressOverflow,
                ))?,
        );
        let indices =
            TranslationIndices::new(address).map_err(Aarch64KernelStackMapError::Paging)?;
        if indices.half() != AddressSpaceHalf::Upper {
            return Err(Aarch64KernelStackMapError::LowerHalf {
                address: address.as_usize(),
            });
        }
        let role = KernelTranslationTableRole::Level3 {
            l1: indices.l1(),
            l2: indices.l2(),
        };
        let table =
            tables
                .frame_for_role(role)
                .ok_or(Aarch64KernelStackMapError::MissingLevel3Table {
                    address: address.as_usize(),
                })?;
        let raw = memory
            .read_descriptor(table, indices.l3())
            .map_err(|error| Aarch64KernelStackMapError::Memory {
                address: address.as_usize(),
                error,
            })?;
        if raw != 0 {
            return Err(Aarch64KernelStackMapError::DescriptorOccupied {
                address: address.as_usize(),
                descriptor: raw,
            });
        }
        if (1..=4).contains(&index) {
            let physical = PhysAddr::new((backing.start_frame_number() + index - 1) * PAGE_SIZE);
            let descriptor = Descriptor::leaf(
                TranslationLevel::L3,
                physical,
                MappingAttributes::kernel_data(),
            )
            .map_err(Aarch64KernelStackMapError::InvalidFrame)?;
            updates[index - 1] = Some(StackLeafUpdate {
                address,
                table,
                index: indices.l3(),
                descriptor,
            });
        }
    }
    let updates = updates.map(|entry| entry.expect("four preflighted heap leaves"));
    for update in updates {
        // SAFETY: Every exact descriptor slot was preflighted above; exclusive
        // access remains held and the commit operation is infallible.
        unsafe {
            memory.write_descriptor(update.table, update.index, update.descriptor);
        }
    }
    tlb.synchronize(&updates.map(|update| update.address));
    Ok(())
}

/// Single-CPU AArch64 maintenance for global TTBR1 L3 leaf changes.
#[cfg(target_arch = "aarch64")]
pub struct Aarch64KernelLeafTlb;

#[cfg(target_arch = "aarch64")]
// SAFETY: the sequence publishes preceding descriptor stores, broadcasts one
// all-ASID EL1 invalidation per page, and completes before returning. The
// mapper constructor separately requires exclusive single-core mutation.
unsafe impl<const PAGES: usize> KernelLeafTlbMaintenance<PAGES> for Aarch64KernelLeafTlb {
    fn synchronize(&mut self, pages: &[VirtAddr; PAGES]) {
        // SAFETY: the trait contract and mapper transaction establish valid
        // descriptor stores and exclusive mutation before this barrier.
        unsafe {
            asm!("dsb ishst", options(nostack, preserves_flags));
        }
        for page in pages {
            let operand = (page.as_usize() >> 12) & ((1_usize << 44) - 1);
            // SAFETY: `VAAE1IS` accepts the page number of a canonical VA and
            // invalidates the global EL1 translation for all ASIDs.
            unsafe {
                asm!(
                    "tlbi vaae1is, {operand}",
                    operand = in(reg) operand,
                    options(nostack, preserves_flags)
                );
            }
        }
        // SAFETY: the completion barrier and ISB prevent later execution or
        // frame release from racing stale translations.
        unsafe {
            asm!("dsb ish", "isb", options(nostack, preserves_flags));
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
        Aarch64KernelStackMapError, Aarch64KernelStackMapper, ActiveKernelTableMemory,
        AllocatedKernelTranslationTable, DirectMapOverride, KernelLeafTlbMaintenance,
        KernelPageTableError, KernelPageTablePlan, KernelTableMaterializationStage,
        KernelTranslationTableRole, MaterializedKernelPageTables,
    };
    use crate::arch::aarch64::paging::{
        Descriptor, MappingAttributes, PagingError, TranslationIndices, TranslationLevel,
    };
    use crate::arch::aarch64::user_page_table::TranslationTableMemory;
    use crate::memory::{
        AddressRange, FrameRange, KernelStackKind, KernelStackMemory, KernelStackPool, MemoryMap,
        PAGE_SIZE, PageFrame, PhysAddr, VirtAddr,
    };

    const OFFSET: usize = 0xffff_ff80_0000_0000;

    #[test]
    fn final_owner_drops_large_planning_metadata_before_publication() {
        assert!(core::mem::size_of::<KernelPageTablePlan<8, 1024>>() < 48 * 1024);
        assert!(core::mem::size_of::<MaterializedKernelPageTables<8>>() < 512);
    }

    #[test]
    fn heap_preflights_both_guards_and_every_leaf_before_commit() {
        let guard = VirtAddr::new(0xffff_ff90_0010_0000);
        let start = VirtAddr::new(guard.as_usize() + PAGE_SIZE);
        let backing = FrameRange::new(
            PageFrame::from_start(PhysAddr::new(0x6400_0000)).unwrap(),
            4,
        )
        .unwrap();
        let mut plan = KernelPageTablePlan::<3, 0>::new().unwrap();
        plan.reserve_l3_range(guard, 6 * PAGE_SIZE).unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(range(0x6300_0000, 0x6300_3000)).unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let frames: Vec<_> = allocated.tables().collect();
        let mut memory = TestMemory::new(&frames);
        let tables = allocated.materialize(&mut memory).unwrap();
        let mut tlb = TestMaintenance::default();
        for read in 0..6 {
            memory.live_read_count = 0;
            memory.fail_live_read = Some(read);
            // SAFETY: Host model owns descriptors and backing; no hardware uses
            // these mappings, and injected failure must retain the empty state.
            assert!(
                unsafe {
                    super::publish_permanent_heap(&tables, &mut memory, &mut tlb, backing, start)
                }
                .is_err()
            );
            assert_eq!(memory.live_write_count, 0);
            assert!(tlb.batches.is_empty());
        }
        memory.fail_live_read = None;
        // SAFETY: The single-threaded host model exclusively retains all frames.
        unsafe { super::publish_permanent_heap(&tables, &mut memory, &mut tlb, backing, start) }
            .unwrap();
        assert_eq!(memory.live_write_count, 4);
        assert_eq!(tlb.batches.len(), 1);
        assert!(!descriptor_for(&frames, &memory, guard).is_valid());
        assert!(
            !descriptor_for(
                &frames,
                &memory,
                VirtAddr::new(start.as_usize() + 4 * PAGE_SIZE)
            )
            .is_valid()
        );
        for index in 0..4 {
            assert_eq!(
                descriptor_for(
                    &frames,
                    &memory,
                    VirtAddr::new(start.as_usize() + index * PAGE_SIZE)
                ),
                Descriptor::leaf(
                    TranslationLevel::L3,
                    PhysAddr::new(0x6400_0000 + index * PAGE_SIZE),
                    MappingAttributes::kernel_data()
                )
                .unwrap()
            );
        }
        // SAFETY: Same host ownership; a second publication must fail unchanged.
        assert!(
            unsafe {
                super::publish_permanent_heap(&tables, &mut memory, &mut tlb, backing, start)
            }
            .is_err()
        );
        assert_eq!(memory.live_write_count, 4);
        assert_eq!(tlb.batches.len(), 1);
    }

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
    fn reserves_empty_l3_topology_across_a_table_boundary() {
        let start = OFFSET + 0x401f_f000;
        let mut plan = KernelPageTablePlan::<4, 1>::new().unwrap();
        plan.reserve_l3_range(VirtAddr::new(start), 2 * PAGE_SIZE)
            .unwrap();

        assert_eq!(
            plan.tables().collect::<Vec<_>>(),
            [
                KernelTranslationTableRole::RootL1,
                KernelTranslationTableRole::Level2 { l1: 1 },
                KernelTranslationTableRole::Level3 { l1: 1, l2: 0 },
                KernelTranslationTableRole::Level3 { l1: 1, l2: 1 },
            ]
        );
        assert_eq!(plan.translate(VirtAddr::new(start)).unwrap(), None);
        assert_eq!(
            plan.translate(VirtAddr::new(start + PAGE_SIZE)).unwrap(),
            None
        );

        plan.map(
            VirtAddr::new(start),
            PhysAddr::new(0x5000_0000),
            TranslationLevel::L3,
            MappingAttributes::kernel_data(),
        )
        .unwrap();
        assert_eq!(plan.required_table_frames(), 4);
        assert_eq!(plan.mappings().len(), 1);
        let before_coarse_leaf = plan;
        assert_eq!(
            plan.map(
                VirtAddr::new(OFFSET + 0x4020_0000),
                PhysAddr::new(0x4020_0000),
                TranslationLevel::L2,
                MappingAttributes::kernel_data(),
            ),
            Err(KernelPageTableError::TableLeafConflict {
                address: OFFSET + 0x4020_0000,
            })
        );
        assert_eq!(plan, before_coarse_leaf);
    }

    #[test]
    fn rejects_invalid_l3_reservations_transactionally() {
        let start = OFFSET + 0x401f_f000;
        let mut plan = KernelPageTablePlan::<3, 1>::new().unwrap();
        let original = plan;
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(start), 2 * PAGE_SIZE),
            Err(KernelPageTableError::TableCapacityExceeded)
        );
        assert_eq!(plan, original);
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(start + 1), PAGE_SIZE),
            Err(KernelPageTableError::MisalignedVirtualStart { address: start + 1 })
        );
        assert_eq!(plan, original);
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(start), PAGE_SIZE + 1),
            Err(KernelPageTableError::MisalignedLength {
                length: PAGE_SIZE + 1,
            })
        );
        assert_eq!(plan, original);
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(start), 0),
            Err(KernelPageTableError::EmptyRange)
        );
        assert_eq!(plan, original);
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(0x4000_0000), PAGE_SIZE),
            Err(KernelPageTableError::LowerHalfMapping {
                address: 0x4000_0000,
            })
        );
        assert_eq!(plan, original);
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(1 << 39), PAGE_SIZE),
            Err(KernelPageTableError::Paging(
                PagingError::NonCanonicalVirtualAddress { address: 1 << 39 }
            ))
        );
        assert_eq!(plan, original);
        let overflow_start = usize::MAX - (PAGE_SIZE - 1);
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(overflow_start), PAGE_SIZE),
            Err(KernelPageTableError::Paging(PagingError::AddressOverflow))
        );
        assert_eq!(plan, original);

        plan.map(
            VirtAddr::new(OFFSET + 0x5000_0000),
            PhysAddr::new(0x5000_0000),
            TranslationLevel::L3,
            MappingAttributes::kernel_data(),
        )
        .unwrap();
        let before_conflict = plan;
        assert_eq!(
            plan.reserve_l3_range(VirtAddr::new(OFFSET + 0x5000_0000), PAGE_SIZE),
            Err(KernelPageTableError::TableLeafConflict {
                address: OFFSET + 0x5000_0000,
            })
        );
        assert_eq!(plan, before_conflict);
    }

    #[test]
    fn direct_map_splits_permissions_and_leaves_guard_holes() {
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
        let guard = DirectMapOverride::unmapped(frame_range(0x4008_3000, 1)).unwrap();
        let mut plan = KernelPageTablePlan::<3, 513>::new().unwrap();
        plan.map_direct_frames(
            frame_range(0x4000_0000, 1024),
            OFFSET,
            MappingAttributes::kernel_data(),
            &[text, rodata, guard],
        )
        .unwrap();

        assert_eq!(plan.required_table_frames(), 3);
        assert_eq!(plan.mappings().len(), 512);
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
        let absent = plan.translate(VirtAddr::new(OFFSET + 0x4008_3000)).unwrap();
        let large = plan
            .translate(VirtAddr::new(OFFSET + 0x4021_2345))
            .unwrap()
            .unwrap();
        assert_eq!(ordinary.attributes, MappingAttributes::kernel_data());
        assert_eq!(ordinary.level, TranslationLevel::L3);
        assert_eq!(executable.attributes, MappingAttributes::kernel_code());
        assert_eq!(readonly.attributes, MappingAttributes::kernel_rodata());
        assert_eq!(absent, None);
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
        assert_eq!(
            DirectMapOverride::unmapped(frame_range(0x5000_0000, 0)),
            Err(KernelPageTableError::InvalidOverrideOrder)
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
    fn materializes_reserved_l3_topology_with_invalid_leaf() {
        let address = OFFSET + 0x5000_0000;
        let mut plan = KernelPageTablePlan::<3, 0>::new().unwrap();
        plan.reserve_l3_range(VirtAddr::new(address), PAGE_SIZE)
            .unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(range(0x6200_0000, 0x6200_3000)).unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let frames: Vec<_> = allocated.tables().collect();
        let mut memory = TestMemory::new(&frames);

        let materialized = allocated.materialize(&mut memory).unwrap();
        let indices =
            crate::arch::aarch64::paging::TranslationIndices::new(VirtAddr::new(address)).unwrap();
        let root = materialized.root_frame();
        let l2 = frame_for(
            &frames,
            KernelTranslationTableRole::Level2 { l1: indices.l1() },
        );
        let l3 = frame_for(
            &frames,
            KernelTranslationTableRole::Level3 {
                l1: indices.l1(),
                l2: indices.l2(),
            },
        );
        assert_eq!(
            memory.descriptor(root, indices.l1()).output_address(),
            l2.start_address()
        );
        assert_eq!(
            memory.descriptor(l2, indices.l2()).output_address(),
            l3.start_address()
        );
        assert!(!memory.descriptor(l3, indices.l3()).is_valid());
        materialized.release_unpublished(&mut allocator).unwrap();
    }

    #[test]
    fn live_stack_mapper_preflights_failures_and_retires_before_release() {
        const ARENA: usize = 0xffff_ff90_0000_0000;
        let mut pool = KernelStackPool::<1, 2>::new(VirtAddr::new(ARENA)).unwrap();
        let mut plan = KernelPageTablePlan::<3, 0>::new().unwrap();
        plan.reserve_l3_range(pool.arena().start(), pool.arena().len())
            .unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(range(0x6300_0000, 0x6300_5000)).unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let table_frames: Vec<_> = allocated.tables().collect();
        let mut table_memory = TestMemory::new(&table_frames);
        let tables = allocated.materialize(&mut table_memory).unwrap();
        let mut clear_memory = ClearMemory;
        let stack = pool
            .allocate(&mut allocator, &mut clear_memory, KernelStackKind::Thread)
            .unwrap();
        let layout = stack.layout();
        let expected_mappings: Vec<_> = stack.mappings().collect();
        let expected_pages: Vec<_> = expected_mappings
            .iter()
            .map(|(address, _)| *address)
            .collect();
        let mut maintenance = TestMaintenance::default();

        table_memory.fail_live_read = Some(2);
        // SAFETY: this host model treats `tables` as the active hierarchy,
        // gives the mapper exclusive descriptor access, and models complete
        // maintenance synchronously.
        let mut mapper = unsafe {
            Aarch64KernelStackMapper::<_, _, 3, 2>::new(
                &tables,
                &mut table_memory,
                &mut maintenance,
            )
        };
        let (stack, Aarch64KernelStackMapError::Memory { error, .. }) =
            (match stack.publish(&mut mapper) {
                Ok(_) => panic!("injected mapping read should fail"),
                Err(failure) => failure,
            })
        else {
            panic!("expected preflight memory failure");
        };
        assert_eq!(error, TestMemoryError::Injected);
        assert_eq!(mapper.memory.live_write_count, 0);
        assert!(mapper.maintenance.batches.is_empty());

        mapper.memory.fail_live_read = None;
        let mapped = stack.publish(&mut mapper).expect("publish stack leaves");
        assert_eq!(mapper.memory.live_write_count, 2);
        assert_eq!(mapper.maintenance.batches.len(), 1);
        assert_eq!(mapper.maintenance.batches[0], expected_pages);
        for (address, frame) in expected_mappings.iter().copied() {
            let indices = TranslationIndices::new(address).unwrap();
            let l3 = frame_for(
                &table_frames,
                KernelTranslationTableRole::Level3 {
                    l1: indices.l1(),
                    l2: indices.l2(),
                },
            );
            let expected = Descriptor::leaf(
                TranslationLevel::L3,
                frame.start_address(),
                MappingAttributes::kernel_data(),
            )
            .unwrap();
            assert_eq!(mapper.memory.descriptor(l3, indices.l3()), expected);
        }
        assert!(
            !descriptor_for(&table_frames, mapper.memory, layout.lower_guard().start()).is_valid()
        );
        assert!(
            !descriptor_for(&table_frames, mapper.memory, layout.upper_guard().start()).is_valid()
        );

        let (first_address, first_frame) = expected_mappings[0];
        let first_indices = TranslationIndices::new(first_address).unwrap();
        let first_l3 = frame_for(
            &table_frames,
            KernelTranslationTableRole::Level3 {
                l1: first_indices.l1(),
                l2: first_indices.l2(),
            },
        );
        let first_table_index = mapper.memory.frame_index(first_l3).unwrap();
        let expected_first = Descriptor::leaf(
            TranslationLevel::L3,
            first_frame.start_address(),
            MappingAttributes::kernel_data(),
        )
        .unwrap();
        mapper.memory.entries[first_table_index][first_indices.l3()] = Descriptor::invalid();
        let writes_before_mismatch = mapper.memory.live_write_count;
        let (mapped, Aarch64KernelStackMapError::DescriptorMismatch { address, .. }) = mapped
            .unpublish(&mut mapper)
            .expect_err("descriptor mismatch")
        else {
            panic!("expected exact descriptor mismatch")
        };
        assert_eq!(address, first_address.as_usize());
        assert_eq!(mapper.memory.live_write_count, writes_before_mismatch);
        assert_eq!(mapper.maintenance.batches.len(), 1);
        mapper.memory.entries[first_table_index][first_indices.l3()] = expected_first;

        mapper.memory.fail_live_read = Some(mapper.memory.live_read_count + 2);
        let writes_before_failed_unmap = mapper.memory.live_write_count;
        let (mapped, Aarch64KernelStackMapError::Memory { error, .. }) = mapped
            .unpublish(&mut mapper)
            .expect_err("injected retirement read failure")
        else {
            panic!("expected retirement preflight failure")
        };
        assert_eq!(error, TestMemoryError::Injected);
        assert_eq!(mapper.memory.live_write_count, writes_before_failed_unmap);
        assert_eq!(mapper.maintenance.batches.len(), 1);

        mapper.memory.fail_live_read = None;
        let stack = match mapped.unpublish(&mut mapper) {
            Ok(stack) => stack,
            Err(_) => panic!("retry should retire stack leaves"),
        };
        assert_eq!(mapper.memory.live_write_count, 4);
        assert_eq!(mapper.maintenance.batches.len(), 2);
        assert_eq!(mapper.maintenance.batches[1], expected_pages);
        for address in stack.mappings().map(|(address, _)| address) {
            assert!(!descriptor_for(&table_frames, mapper.memory, address).is_valid());
        }
        stack
            .release_unpublished(&mut pool, &mut allocator)
            .unwrap();
        assert_eq!(pool.allocated_slots(), 0);
        assert_eq!(allocator.total_free_frames(), 2);
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

    fn descriptor_for(
        frames: &[AllocatedKernelTranslationTable],
        memory: &TestMemory,
        address: VirtAddr,
    ) -> Descriptor {
        let indices = TranslationIndices::new(address).unwrap();
        let l3 = frame_for(
            frames,
            KernelTranslationTableRole::Level3 {
                l1: indices.l1(),
                l2: indices.l2(),
            },
        );
        memory.descriptor(l3, indices.l3())
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
        live_read_count: usize,
        fail_live_read: Option<usize>,
        live_write_count: usize,
    }

    impl TestMemory {
        fn new(tables: &[AllocatedKernelTranslationTable]) -> Self {
            Self {
                frames: tables.iter().map(|table| table.frame()).collect(),
                entries: vec![vec![Descriptor::invalid(); 512]; tables.len()],
                write_count: 0,
                fail_write: None,
                live_read_count: 0,
                fail_live_read: None,
                live_write_count: 0,
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

    // SAFETY: this host model owns fixed descriptor arrays. Successful reads
    // select an exact slot, and the matching unsafe write is infallible and
    // non-tearing within the single-threaded test.
    unsafe impl ActiveKernelTableMemory for TestMemory {
        type Error = TestMemoryError;

        fn read_descriptor(&mut self, frame: PageFrame, index: usize) -> Result<u64, Self::Error> {
            if self.live_read_count == self.fail_live_read.unwrap_or(usize::MAX) {
                return Err(TestMemoryError::Injected);
            }
            self.live_read_count += 1;
            let frame_index = self.frame_index(frame)?;
            self.entries[frame_index]
                .get(index)
                .copied()
                .map(Descriptor::raw)
                .ok_or(TestMemoryError::InvalidIndex)
        }

        unsafe fn write_descriptor(
            &mut self,
            frame: PageFrame,
            index: usize,
            descriptor: Descriptor,
        ) {
            let frame_index = self.frame_index(frame).expect("preflighted table frame");
            self.entries[frame_index][index] = descriptor;
            self.live_write_count += 1;
        }
    }

    struct ClearMemory;

    impl KernelStackMemory for ClearMemory {
        type Error = core::convert::Infallible;

        fn clear_frame(&mut self, _frame: PageFrame) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct TestMaintenance {
        batches: Vec<Vec<VirtAddr>>,
    }

    // SAFETY: this single-threaded host model records the complete batch and
    // has no hardware translation cache to invalidate.
    unsafe impl<const PAGES: usize> KernelLeafTlbMaintenance<PAGES> for TestMaintenance {
        fn synchronize(&mut self, pages: &[VirtAddr; PAGES]) {
            self.batches.push(pages.to_vec());
        }
    }
}
