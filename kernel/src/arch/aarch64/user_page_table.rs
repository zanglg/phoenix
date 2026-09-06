//! Owned AArch64 three-level translation tables for lower-half user pages.

use crate::arch::aarch64::paging::{
    Access, AddressSpaceHalf, Descriptor, Execute, MappingAttributes, MemoryType, PagingError,
    TranslationIndices, TranslationLevel,
};
use crate::memory::{AllocationError, FrameAllocator, PageFrame, VirtAddr};
use crate::process_image::PopulatedProcessImage;
use crate::user::{UserAddr, UserPermissions};

const ENTRIES_PER_TABLE: usize = 512;

/// Failure while planning an AArch64 user translation hierarchy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserPageTableError {
    /// Address or descriptor validation failed.
    Paging(PagingError),
    /// A user virtual address is not aligned to the 4 KiB translation granule.
    MisalignedUserPage {
        /// Rejected user address.
        address: usize,
    },
    /// The generic permissions request an AArch64-unrepresentable unreadable page.
    UnreadableUserPage,
    /// A leaf already exists for the requested user virtual page.
    DuplicateUserPage,
    /// The fixed table-topology capacity cannot describe another intermediate table.
    TableCapacityExceeded,
    /// The fixed leaf-metadata capacity cannot describe another user page.
    LeafCapacityExceeded,
}

/// Failure while assigning physical frames to planned translation tables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserPageTableAllocationError {
    /// The allocator has fewer free frames than the table topology requires.
    OutOfMemory,
    /// The allocator rejected a single-frame request.
    Allocator(AllocationError),
    /// An allocated frame cannot be encoded in an AArch64 table descriptor.
    Paging(PagingError),
}

/// Role of one 4 KiB table page in the three-level hierarchy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TranslationTableRole {
    /// Root table installed in `TTBR0_EL1` and indexed at level 1.
    RootL1,
    /// Level-2 table reached through one root entry.
    Level2 {
        /// Parent level-1 index.
        l1: usize,
    },
    /// Level-3 table containing final 4 KiB page descriptors.
    Level3 {
        /// Ancestor level-1 index.
        l1: usize,
        /// Parent level-2 index.
        l2: usize,
    },
}

/// One validated user leaf retained by the table plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlannedUserLeaf {
    virtual_start: UserAddr,
    physical_frame: PageFrame,
    permissions: UserPermissions,
    indices: TranslationIndices,
    descriptor: Descriptor,
}

impl PlannedUserLeaf {
    /// Return the aligned user virtual page start.
    pub const fn virtual_start(self) -> UserAddr {
        self.virtual_start
    }

    /// Return the populated physical frame mapped by this leaf.
    pub const fn physical_frame(self) -> PageFrame {
        self.physical_frame
    }

    /// Return the architecture-neutral final user permissions.
    pub const fn permissions(self) -> UserPermissions {
        self.permissions
    }

    /// Return the prevalidated AArch64 page descriptor.
    pub const fn descriptor(self) -> Descriptor {
        self.descriptor
    }
}

/// Fixed-capacity topology and leaf plan for one lower-half address space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserPageTablePlan<const TABLES: usize, const LEAVES: usize> {
    tables: [Option<TranslationTableRole>; TABLES],
    table_count: usize,
    leaves: [Option<PlannedUserLeaf>; LEAVES],
    leaf_count: usize,
}

impl<const TABLES: usize, const LEAVES: usize> UserPageTablePlan<TABLES, LEAVES> {
    /// Create a plan containing the mandatory level-1 root.
    pub fn new() -> Result<Self, UserPageTableError> {
        if TABLES == 0 {
            return Err(UserPageTableError::TableCapacityExceeded);
        }
        let mut tables = [None; TABLES];
        tables[0] = Some(TranslationTableRole::RootL1);
        Ok(Self {
            tables,
            table_count: 1,
            leaves: [None; LEAVES],
            leaf_count: 0,
        })
    }

    /// Add one 4 KiB user mapping as an all-or-nothing topology update.
    pub fn map_page(
        &mut self,
        virtual_start: UserAddr,
        physical_frame: PageFrame,
        permissions: UserPermissions,
    ) -> Result<(), UserPageTableError> {
        let mut candidate = *self;
        candidate.map_page_inner(virtual_start, physical_frame, permissions)?;
        *self = candidate;
        Ok(())
    }

    /// Add every page from a fully populated process image transactionally.
    pub fn map_process_image<const MAPPINGS: usize, const PAGES: usize>(
        &mut self,
        image: &PopulatedProcessImage<MAPPINGS, PAGES>,
    ) -> Result<(), UserPageTableError> {
        let mut candidate = *self;
        for page in image.pages() {
            candidate.map_page_inner(page.virtual_start(), page.frame(), page.permissions())?;
        }
        *self = candidate;
        Ok(())
    }

    /// Return planned table roles in parent-before-child order.
    pub fn tables(
        &self,
    ) -> impl ExactSizeIterator<Item = TranslationTableRole> + DoubleEndedIterator + '_ {
        self.tables[..self.table_count]
            .iter()
            .map(|role| role.expect("active user table role slot"))
    }

    /// Return leaves sorted by user virtual address.
    pub fn leaves(&self) -> impl ExactSizeIterator<Item = PlannedUserLeaf> + '_ {
        self.leaves[..self.leaf_count]
            .iter()
            .map(|leaf| leaf.expect("active user leaf slot"))
    }

    /// Return the exact number of physical frames needed for translation tables.
    pub const fn required_table_frames(&self) -> usize {
        self.table_count
    }

    /// Assign all table frames as one allocator transaction.
    pub fn allocate<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<AllocatedUserPageTables<TABLES, LEAVES>, UserPageTableAllocationError> {
        let mut candidate = *allocator;
        let mut frames = [None; TABLES];
        for slot in &mut frames[..self.table_count] {
            let frame = candidate
                .allocate()
                .map_err(UserPageTableAllocationError::Allocator)?
                .ok_or(UserPageTableAllocationError::OutOfMemory)?;
            Descriptor::table(frame).map_err(UserPageTableAllocationError::Paging)?;
            *slot = Some(frame);
        }
        *allocator = candidate;
        Ok(AllocatedUserPageTables { plan: self, frames })
    }

    fn map_page_inner(
        &mut self,
        virtual_start: UserAddr,
        physical_frame: PageFrame,
        permissions: UserPermissions,
    ) -> Result<(), UserPageTableError> {
        if !virtual_start.is_page_aligned() {
            return Err(UserPageTableError::MisalignedUserPage {
                address: virtual_start.as_usize(),
            });
        }
        if !permissions.readable() {
            return Err(UserPageTableError::UnreadableUserPage);
        }
        let indices = TranslationIndices::new(VirtAddr::new(virtual_start.as_usize()))
            .map_err(UserPageTableError::Paging)?;
        if indices.half() != AddressSpaceHalf::Lower {
            return Err(UserPageTableError::Paging(
                PagingError::NonCanonicalVirtualAddress {
                    address: virtual_start.as_usize(),
                },
            ));
        }
        if self
            .leaves()
            .any(|leaf| leaf.virtual_start == virtual_start)
        {
            return Err(UserPageTableError::DuplicateUserPage);
        }
        if self.leaf_count == LEAVES {
            return Err(UserPageTableError::LeafCapacityExceeded);
        }

        self.ensure_table(TranslationTableRole::Level2 { l1: indices.l1() })?;
        self.ensure_table(TranslationTableRole::Level3 {
            l1: indices.l1(),
            l2: indices.l2(),
        })?;
        let attributes = user_attributes(permissions)?;
        let descriptor = Descriptor::leaf(
            TranslationLevel::L3,
            physical_frame.start_address(),
            attributes,
        )
        .map_err(UserPageTableError::Paging)?;
        let leaf = PlannedUserLeaf {
            virtual_start,
            physical_frame,
            permissions,
            indices,
            descriptor,
        };
        let insertion = self.leaves[..self.leaf_count].partition_point(|current| {
            current.expect("active user leaf slot").virtual_start < virtual_start
        });
        self.leaves
            .copy_within(insertion..self.leaf_count, insertion + 1);
        self.leaves[insertion] = Some(leaf);
        self.leaf_count += 1;
        Ok(())
    }

    fn ensure_table(&mut self, role: TranslationTableRole) -> Result<(), UserPageTableError> {
        if self.tables().any(|current| current == role) {
            return Ok(());
        }
        if self.table_count == TABLES {
            return Err(UserPageTableError::TableCapacityExceeded);
        }
        let insertion = self.tables[..self.table_count]
            .partition_point(|current| current.expect("active user table role slot") < role);
        self.tables
            .copy_within(insertion..self.table_count, insertion + 1);
        self.tables[insertion] = Some(role);
        self.table_count += 1;
        Ok(())
    }
}

fn user_attributes(permissions: UserPermissions) -> Result<MappingAttributes, UserPageTableError> {
    if !permissions.readable() {
        return Err(UserPageTableError::UnreadableUserPage);
    }
    let access = if permissions.writable() {
        Access::UserReadWrite
    } else {
        Access::UserReadOnly
    };
    let execute = if permissions.executable() {
        Execute::User
    } else {
        Execute::Never
    };
    MappingAttributes::new(MemoryType::Normal, access, execute).map_err(UserPageTableError::Paging)
}

/// One table role paired with its uniquely owned physical frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocatedTranslationTable {
    role: TranslationTableRole,
    frame: PageFrame,
}

impl AllocatedTranslationTable {
    /// Return this table's position in the translation hierarchy.
    pub const fn role(self) -> TranslationTableRole {
        self.role
    }

    /// Return the physical frame backing this table.
    pub const fn frame(self) -> PageFrame {
        self.frame
    }
}

/// Private table-memory operations required to materialize the plan.
pub trait TranslationTableMemory {
    /// Backend-specific failure.
    type Error;

    /// Clear all 512 descriptors in a private translation-table frame.
    fn clear_table(&mut self, frame: PageFrame) -> Result<(), Self::Error>;

    /// Write one validated descriptor into a private table frame.
    fn write_descriptor(
        &mut self,
        frame: PageFrame,
        index: usize,
        descriptor: Descriptor,
    ) -> Result<(), Self::Error>;
}

/// Kind of table-memory operation that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableMaterializationStage {
    /// Clearing all descriptors before reuse.
    Clear,
    /// Linking a parent table to an intermediate child.
    Link,
    /// Installing a final user page descriptor.
    Leaf,
}

/// Materialization failure retaining ownership of every table frame.
#[derive(Debug, Eq, PartialEq)]
pub struct TableMaterializationError<E, const TABLES: usize, const LEAVES: usize> {
    tables: AllocatedUserPageTables<TABLES, LEAVES>,
    stage: TableMaterializationStage,
    table: PageFrame,
    index: Option<usize>,
    error: E,
}

impl<E, const TABLES: usize, const LEAVES: usize> TableMaterializationError<E, TABLES, LEAVES> {
    /// Return the operation category that failed.
    pub const fn stage(&self) -> TableMaterializationStage {
        self.stage
    }

    /// Return the table frame being modified when the backend failed.
    pub const fn table(&self) -> PageFrame {
        self.table
    }

    /// Return the descriptor index, or `None` for a whole-table clear.
    pub const fn index(&self) -> Option<usize> {
        self.index
    }

    /// Return the backend-specific error by reference.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover table ownership and the backend error for retry or release.
    pub fn into_parts(self) -> (AllocatedUserPageTables<TABLES, LEAVES>, E) {
        (self.tables, self.error)
    }
}

/// Allocated but not yet completely initialized translation-table frames.
#[derive(Debug, Eq, PartialEq)]
pub struct AllocatedUserPageTables<const TABLES: usize, const LEAVES: usize> {
    plan: UserPageTablePlan<TABLES, LEAVES>,
    frames: [Option<PageFrame>; TABLES],
}

impl<const TABLES: usize, const LEAVES: usize> AllocatedUserPageTables<TABLES, LEAVES> {
    /// Return table roles and frames in parent-before-child order.
    pub fn tables(
        &self,
    ) -> impl ExactSizeIterator<Item = AllocatedTranslationTable> + DoubleEndedIterator + '_ {
        self.plan
            .tables()
            .enumerate()
            .map(|(index, role)| AllocatedTranslationTable {
                role,
                frame: self.frames[index].expect("allocated user table frame slot"),
            })
    }

    /// Clear and write every private table, producing a publishable root only on success.
    pub fn materialize<M: TranslationTableMemory>(
        self,
        memory: &mut M,
    ) -> Result<
        MaterializedUserPageTables<TABLES, LEAVES>,
        TableMaterializationError<M::Error, TABLES, LEAVES>,
    > {
        for table_index in 0..self.plan.table_count {
            let frame = self.frames[table_index].expect("allocated user table frame slot");
            if let Err(error) = memory.clear_table(frame) {
                return Err(TableMaterializationError {
                    tables: self,
                    stage: TableMaterializationStage::Clear,
                    table: frame,
                    index: None,
                    error,
                });
            }
        }

        for table_index in 1..self.plan.table_count {
            let role = self.plan.tables[table_index].expect("active user table role slot");
            let child = self.frames[table_index].expect("allocated user table frame slot");
            let (parent_role, entry) = match role {
                TranslationTableRole::RootL1 => unreachable!("root is first and unique"),
                TranslationTableRole::Level2 { l1 } => (TranslationTableRole::RootL1, l1),
                TranslationTableRole::Level3 { l1, l2 } => {
                    (TranslationTableRole::Level2 { l1 }, l2)
                }
            };
            let parent = self.frame_for(parent_role);
            let descriptor =
                Descriptor::table(child).expect("table frames validated at allocation");
            if let Err(error) = memory.write_descriptor(parent, entry, descriptor) {
                return Err(TableMaterializationError {
                    tables: self,
                    stage: TableMaterializationStage::Link,
                    table: parent,
                    index: Some(entry),
                    error,
                });
            }
        }

        for leaf_index in 0..self.plan.leaf_count {
            let leaf = self.plan.leaves[leaf_index].expect("active user leaf slot");
            let table = self.frame_for(TranslationTableRole::Level3 {
                l1: leaf.indices.l1(),
                l2: leaf.indices.l2(),
            });
            let entry = leaf.indices.l3();
            if let Err(error) = memory.write_descriptor(table, entry, leaf.descriptor) {
                return Err(TableMaterializationError {
                    tables: self,
                    stage: TableMaterializationStage::Leaf,
                    table,
                    index: Some(entry),
                    error,
                });
            }
        }

        Ok(MaterializedUserPageTables { tables: self })
    }

    /// Return all unpublished table frames to their allocator transactionally.
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

    fn frame_for(&self, role: TranslationTableRole) -> PageFrame {
        let index = self
            .plan
            .tables()
            .position(|current| current == role)
            .expect("planned parent table exists");
        self.frames[index].expect("allocated user table frame slot")
    }
}

/// Completely cleared and linked user tables whose root may be published.
#[derive(Debug, Eq, PartialEq)]
pub struct MaterializedUserPageTables<const TABLES: usize, const LEAVES: usize> {
    tables: AllocatedUserPageTables<TABLES, LEAVES>,
}

impl<const TABLES: usize, const LEAVES: usize> MaterializedUserPageTables<TABLES, LEAVES> {
    /// Return the physical root suitable for a future checked `TTBR0_EL1` activation.
    pub fn root_frame(&self) -> PageFrame {
        self.tables.frame_for(TranslationTableRole::RootL1)
    }

    /// Return table roles and physical frames for inspection and ownership accounting.
    pub fn tables(
        &self,
    ) -> impl ExactSizeIterator<Item = AllocatedTranslationTable> + DoubleEndedIterator + '_ {
        self.tables.tables()
    }

    /// Return the final leaf descriptors in user virtual-address order.
    pub fn leaves(&self) -> impl ExactSizeIterator<Item = PlannedUserLeaf> + '_ {
        self.tables.plan.leaves()
    }

    /// Release tables that have never been installed in a live translation regime.
    ///
    /// A future activation API will consume this value into a separate active
    /// address-space owner, so that active roots cannot call this method.
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

/// Why populated pages and a materialized hierarchy cannot form one address space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressSpaceAssemblyErrorReason {
    /// The number of resident pages differs from the number of table leaves.
    PageCountMismatch {
        /// Number of populated resident pages.
        resident_pages: usize,
        /// Number of final translation leaves.
        table_leaves: usize,
    },
    /// Virtual address, physical frame, or permission metadata differs at one sorted page.
    PageMismatch {
        /// Zero-based virtual-order page index.
        index: usize,
    },
}

/// Assembly failure that returns both independent ownership objects.
#[derive(Debug, Eq, PartialEq)]
pub struct AddressSpaceAssemblyError<
    const MAPPINGS: usize,
    const PAGES: usize,
    const TABLES: usize,
    const LEAVES: usize,
> {
    image: PopulatedProcessImage<MAPPINGS, PAGES>,
    tables: MaterializedUserPageTables<TABLES, LEAVES>,
    reason: AddressSpaceAssemblyErrorReason,
}

impl<const MAPPINGS: usize, const PAGES: usize, const TABLES: usize, const LEAVES: usize>
    AddressSpaceAssemblyError<MAPPINGS, PAGES, TABLES, LEAVES>
{
    /// Return the exact mismatch detected before ownership was combined.
    pub const fn reason(&self) -> AddressSpaceAssemblyErrorReason {
        self.reason
    }

    /// Recover both owners so they can be inspected, rebuilt, or released.
    pub fn into_parts(
        self,
    ) -> (
        PopulatedProcessImage<MAPPINGS, PAGES>,
        MaterializedUserPageTables<TABLES, LEAVES>,
    ) {
        (self.image, self.tables)
    }
}

/// Populated leaf frames and materialized tables owned as one inactive address space.
#[derive(Debug, Eq, PartialEq)]
pub struct PreparedUserAddressSpace<
    const MAPPINGS: usize,
    const PAGES: usize,
    const TABLES: usize,
    const LEAVES: usize,
> {
    image: PopulatedProcessImage<MAPPINGS, PAGES>,
    tables: MaterializedUserPageTables<TABLES, LEAVES>,
}

impl<const MAPPINGS: usize, const PAGES: usize, const TABLES: usize, const LEAVES: usize>
    PreparedUserAddressSpace<MAPPINGS, PAGES, TABLES, LEAVES>
{
    /// Verify exact leaf identity and combine both unique owners.
    pub fn new(
        image: PopulatedProcessImage<MAPPINGS, PAGES>,
        tables: MaterializedUserPageTables<TABLES, LEAVES>,
    ) -> Result<Self, AddressSpaceAssemblyError<MAPPINGS, PAGES, TABLES, LEAVES>> {
        let resident_pages = image.pages().len();
        let table_leaves = tables.leaves().len();
        if resident_pages != table_leaves {
            return Err(AddressSpaceAssemblyError {
                image,
                tables,
                reason: AddressSpaceAssemblyErrorReason::PageCountMismatch {
                    resident_pages,
                    table_leaves,
                },
            });
        }
        let mismatch = image.pages().zip(tables.leaves()).position(|(page, leaf)| {
            page.virtual_start() != leaf.virtual_start()
                || page.frame() != leaf.physical_frame()
                || page.permissions() != leaf.permissions()
        });
        if let Some(index) = mismatch {
            return Err(AddressSpaceAssemblyError {
                image,
                tables,
                reason: AddressSpaceAssemblyErrorReason::PageMismatch { index },
            });
        }
        Ok(Self { image, tables })
    }

    /// Return the physical L1 root for a future ownership-consuming activation API.
    pub fn root_frame(&self) -> PageFrame {
        self.tables.root_frame()
    }

    /// Return the initial user program counter.
    pub const fn entry(&self) -> UserAddr {
        self.image.entry()
    }

    /// Return the initial user stack pointer.
    pub const fn stack_pointer(&self) -> UserAddr {
        self.image.stack_pointer()
    }

    /// Return the exact number of resident user pages.
    pub fn resident_page_count(&self) -> usize {
        self.image.pages().len()
    }

    /// Consume the complete owner, install its root in TTBR0, and enter EL0t.
    ///
    /// # Safety
    ///
    /// Call only on the single boot CPU while ASID zero is private. The
    /// bootstrap physical-memory mapping used to construct the tables must
    /// remain coherent, `VBAR_EL1` and an EL1 stack must be active, and no
    /// caller may retain aliases to owned data or table frames.
    #[cfg(target_arch = "aarch64")]
    pub unsafe fn activate_and_enter(self) -> ! {
        let root = self.root_frame();
        let entry = self.entry();
        let stack_pointer = self.stack_pointer();
        // SAFETY: consuming `self` retains exclusive ownership of the complete
        // table hierarchy and all leaf frames forever because this call cannot
        // return. The caller supplies the remaining CPU-state requirements.
        unsafe {
            super::user_entry::activate_and_enter(root, entry, stack_pointer);
        }
    }

    /// Release every unpublished data and table frame as one allocator transaction.
    pub fn release_unpublished<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<(), (Self, AllocationError)> {
        let mut candidate = *allocator;
        let table_error = {
            let mut error = None;
            for table in self.tables.tables().rev() {
                if let Err(current) = candidate.deallocate(table.frame()) {
                    error = Some(current);
                    break;
                }
            }
            error
        };
        if let Some(error) = table_error {
            return Err((self, error));
        }
        let page_error = {
            let mut error = None;
            for page in self.image.pages().rev() {
                if let Err(current) = candidate.deallocate(page.frame()) {
                    error = Some(current);
                    break;
                }
            }
            error
        };
        if let Some(error) = page_error {
            return Err((self, error));
        }
        *allocator = candidate;
        Ok(())
    }
}

const _: () = assert!(ENTRIES_PER_TABLE == 512);

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use super::{
        TableMaterializationStage, TranslationTableMemory, TranslationTableRole,
        UserPageTableAllocationError, UserPageTableError, UserPageTablePlan,
    };
    use crate::arch::aarch64::paging::Descriptor;
    use crate::memory::{AddressRange, MemoryMap, PAGE_SIZE, PageFrame, PhysAddr};
    use crate::user::{UserAddr, UserPermissions};

    #[test]
    fn reuses_intermediate_tables_and_orders_the_topology() {
        let mut plan = UserPageTablePlan::<6, 4>::new().unwrap();
        plan.map_page(
            user(0x4000_0000),
            frame(0x20_0000),
            UserPermissions::read_execute(),
        )
        .unwrap();
        plan.map_page(
            user(0x0040_0000),
            frame(0x20_1000),
            UserPermissions::read_execute(),
        )
        .unwrap();
        plan.map_page(
            user(0x0040_1000),
            frame(0x20_2000),
            UserPermissions::read_only(),
        )
        .unwrap();
        plan.map_page(
            user(0x0080_0000),
            frame(0x20_3000),
            UserPermissions::read_write(),
        )
        .unwrap();

        assert_eq!(
            plan.tables().collect::<Vec<_>>(),
            [
                TranslationTableRole::RootL1,
                TranslationTableRole::Level2 { l1: 0 },
                TranslationTableRole::Level2 { l1: 1 },
                TranslationTableRole::Level3 { l1: 0, l2: 2 },
                TranslationTableRole::Level3 { l1: 0, l2: 4 },
                TranslationTableRole::Level3 { l1: 1, l2: 0 },
            ]
        );
        assert_eq!(plan.required_table_frames(), 6);
        let starts: Vec<_> = plan.leaves().map(|leaf| leaf.virtual_start()).collect();
        assert_eq!(
            starts,
            [
                user(0x0040_0000),
                user(0x0040_1000),
                user(0x0080_0000),
                user(0x4000_0000)
            ]
        );
    }

    #[test]
    fn rejects_bad_permissions_duplicates_and_capacity_transactionally() {
        let mut plan = UserPageTablePlan::<3, 1>::new().unwrap();
        assert_eq!(
            plan.map_page(
                user(0x0040_0001),
                frame(0x30_0000),
                UserPermissions::read_only()
            ),
            Err(UserPageTableError::MisalignedUserPage {
                address: 0x0040_0001
            })
        );
        assert_eq!(
            plan.map_page(
                user(0x0040_0000),
                frame(0x30_0000),
                UserPermissions::new(false, false, true).unwrap()
            ),
            Err(UserPageTableError::UnreadableUserPage)
        );
        plan.map_page(
            user(0x0040_0000),
            frame(0x30_0000),
            UserPermissions::read_execute(),
        )
        .unwrap();
        let before = plan;
        assert_eq!(
            plan.map_page(
                user(0x0040_0000),
                frame(0x30_1000),
                UserPermissions::read_only()
            ),
            Err(UserPageTableError::DuplicateUserPage)
        );
        assert_eq!(plan, before);
        assert_eq!(
            plan.map_page(
                user(0x0080_0000),
                frame(0x30_1000),
                UserPermissions::read_write()
            ),
            Err(UserPageTableError::LeafCapacityExceeded)
        );
        assert_eq!(plan, before);

        let mut small = UserPageTablePlan::<2, 1>::new().unwrap();
        let before = small;
        assert_eq!(
            small.map_page(
                user(0x0040_0000),
                frame(0x30_0000),
                UserPermissions::read_only()
            ),
            Err(UserPageTableError::TableCapacityExceeded)
        );
        assert_eq!(small, before);
    }

    #[test]
    fn table_frame_allocation_is_atomic_on_out_of_memory() {
        let mut plan = UserPageTablePlan::<3, 1>::new().unwrap();
        plan.map_page(
            user(0x0040_0000),
            frame(0x40_0000),
            UserPermissions::read_execute(),
        )
        .unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(bytes(0x50_0000, 0x50_2000)).unwrap();
        let mut allocator = map.into_allocator();
        let before = allocator;

        assert_eq!(
            plan.allocate(&mut allocator),
            Err(UserPageTableAllocationError::OutOfMemory)
        );
        assert_eq!(allocator, before);
    }

    #[test]
    fn materialization_clears_links_and_installs_final_leaves() {
        let mut plan = UserPageTablePlan::<4, 3>::new().unwrap();
        plan.map_page(
            user(0x0040_0000),
            frame(0x60_0000),
            UserPermissions::read_execute(),
        )
        .unwrap();
        plan.map_page(
            user(0x0040_1000),
            frame(0x60_1000),
            UserPermissions::read_only(),
        )
        .unwrap();
        plan.map_page(
            user(0x0080_0000),
            frame(0x60_2000),
            UserPermissions::read_write(),
        )
        .unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(bytes(0x70_0000, 0x70_4000)).unwrap();
        let mut allocator = map.into_allocator();
        let before = allocator;
        let allocated = plan.allocate(&mut allocator).unwrap();
        let table_frames: Vec<_> = allocated.tables().collect();
        let mut memory = TestTableMemory::new(&table_frames);

        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.root_frame();
        let l2 = table_frames
            .iter()
            .find(|table| table.role() == TranslationTableRole::Level2 { l1: 0 })
            .unwrap()
            .frame();
        let code_l3 = table_frames
            .iter()
            .find(|table| table.role() == TranslationTableRole::Level3 { l1: 0, l2: 2 })
            .unwrap()
            .frame();
        assert_eq!(
            memory.descriptor(root, 0).output_address(),
            l2.start_address()
        );
        assert_eq!(
            memory.descriptor(l2, 2).output_address(),
            code_l3.start_address()
        );
        assert_eq!(
            memory.descriptor(code_l3, 0).output_address(),
            PhysAddr::new(0x60_0000)
        );
        assert_eq!(
            memory.descriptor(code_l3, 1).output_address(),
            PhysAddr::new(0x60_1000)
        );
        materialized.release_unpublished(&mut allocator).unwrap();
        assert_eq!(allocator, before);
    }

    #[test]
    fn materialization_failure_returns_tables_for_clean_retry() {
        let mut plan = UserPageTablePlan::<3, 1>::new().unwrap();
        plan.map_page(
            user(0x0040_0000),
            frame(0x80_0000),
            UserPermissions::read_execute(),
        )
        .unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(bytes(0x90_0000, 0x90_3000)).unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let frames: Vec<_> = allocated.tables().collect();
        let mut failing = TestTableMemory::new(&frames);
        failing.fail_write = Some(0);

        let error = allocated
            .materialize(&mut failing)
            .expect_err("first descriptor write fails");
        assert_eq!(error.stage(), TableMaterializationStage::Link);
        assert_eq!(error.index(), Some(0));
        assert_eq!(error.error(), &TestTableMemoryError::Injected);
        let (allocated, backend_error) = error.into_parts();
        assert_eq!(backend_error, TestTableMemoryError::Injected);

        let retry_frames: Vec<_> = allocated.tables().collect();
        let mut retry = TestTableMemory::new(&retry_frames);
        let materialized = allocated.materialize(&mut retry).unwrap();
        assert_eq!(retry.clear_count, 3);
        materialized.release_unpublished(&mut allocator).unwrap();
    }

    fn user(address: usize) -> UserAddr {
        UserAddr::new(address).unwrap()
    }

    fn frame(address: usize) -> PageFrame {
        PageFrame::from_start(PhysAddr::new(address)).unwrap()
    }

    fn bytes(start: usize, end: usize) -> AddressRange<PhysAddr> {
        AddressRange::new(PhysAddr::new(start), PhysAddr::new(end)).unwrap()
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestTableMemoryError {
        UnknownFrame,
        InvalidIndex,
        Injected,
    }

    struct TestTableMemory {
        frames: Vec<PageFrame>,
        entries: Vec<Vec<Descriptor>>,
        clear_count: usize,
        write_count: usize,
        fail_write: Option<usize>,
    }

    impl TestTableMemory {
        fn new(tables: &[super::AllocatedTranslationTable]) -> Self {
            Self {
                frames: tables.iter().map(|table| table.frame()).collect(),
                entries: vec![vec![Descriptor::invalid(); 512]; tables.len()],
                clear_count: 0,
                write_count: 0,
                fail_write: None,
            }
        }

        fn frame_index(&self, frame: PageFrame) -> Result<usize, TestTableMemoryError> {
            self.frames
                .iter()
                .position(|candidate| *candidate == frame)
                .ok_or(TestTableMemoryError::UnknownFrame)
        }

        fn descriptor(&self, frame: PageFrame, index: usize) -> Descriptor {
            self.entries[self.frame_index(frame).unwrap()][index]
        }
    }

    impl TranslationTableMemory for TestTableMemory {
        type Error = TestTableMemoryError;

        fn clear_table(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
            let index = self.frame_index(frame)?;
            self.entries[index].fill(Descriptor::invalid());
            self.clear_count += 1;
            Ok(())
        }

        fn write_descriptor(
            &mut self,
            frame: PageFrame,
            index: usize,
            descriptor: Descriptor,
        ) -> Result<(), Self::Error> {
            let frame_index = self.frame_index(frame)?;
            let entry = self.entries[frame_index]
                .get_mut(index)
                .ok_or(TestTableMemoryError::InvalidIndex)?;
            if self.fail_write == Some(self.write_count) {
                return Err(TestTableMemoryError::Injected);
            }
            *entry = descriptor;
            self.write_count += 1;
            Ok(())
        }
    }

    const _: () = assert!(PAGE_SIZE == 4096);
}
