//! Transactional planning and frame ownership for an initial user process image.

use crate::elf::{ElfError, ElfImage};
use crate::memory::{AllocationError, FrameAllocator, PAGE_SIZE, PageFrame};
use crate::user::{
    UserAddr, UserAddressError, UserAddressSpacePlan, UserMappingKind, UserPermissions,
    UserStackLayout,
};

/// Failure while combining a validated ELF image and guarded stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessImageError {
    /// Re-reading a validated load segment unexpectedly failed.
    Elf(ElfError),
    /// A program or stack mapping violates the user address-space contract.
    Address(UserAddressError),
    /// A program segment occupies part of the required unmapped stack guard.
    StackGuardOverlap,
    /// The fixed page-metadata capacity cannot describe the entire image.
    PageCapacityExceeded,
    /// Page-count arithmetic overflowed.
    PageCountOverflow,
}

/// Failure while assigning physical frames to a complete process-image plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessImageAllocationError {
    /// The allocator has fewer free frames than the image requires.
    OutOfMemory,
    /// The allocator rejected an otherwise valid single-frame request.
    Allocator(AllocationError),
}

/// Destination-memory operation required while populating an image page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopulationStage {
    /// Clear the entire physical page before exposing any source bytes.
    Clear,
    /// Copy the initialized ELF prefix into an already-cleared page.
    Initialize,
}

/// Abstract access to physical frames while they are private to the loader.
///
/// Implementations may use a permanent physical direct map, a temporary
/// mapping window, or host-owned test storage. The trait itself neither
/// creates mappings nor publishes frames to EL0.
pub trait ProcessImageMemory {
    /// Backend-specific failure.
    type Error;

    /// Set all `PAGE_SIZE` bytes in `frame` to zero.
    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error>;

    /// Copy `bytes` into a private frame at `offset`.
    fn write_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), Self::Error>;
}

/// Population failure retaining ownership of every process-image frame.
#[derive(Debug, Eq, PartialEq)]
pub struct ProcessImagePopulationError<'a, E, const MAPPINGS: usize, const PAGES: usize> {
    image: AllocatedProcessImage<'a, MAPPINGS, PAGES>,
    page_index: usize,
    stage: PopulationStage,
    error: E,
}

impl<'a, E, const MAPPINGS: usize, const PAGES: usize>
    ProcessImagePopulationError<'a, E, MAPPINGS, PAGES>
{
    /// Return the zero-based virtual-order page index that failed.
    pub const fn page_index(&self) -> usize {
        self.page_index
    }

    /// Return whether clearing or initialized-byte copying failed.
    pub const fn stage(&self) -> PopulationStage {
        self.stage
    }

    /// Return the backend-specific error by reference.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover frame ownership and the backend error for retry or rollback.
    pub fn into_parts(self) -> (AllocatedProcessImage<'a, MAPPINGS, PAGES>, E) {
        (self.image, self.error)
    }
}

/// One page that must be cleared and optionally initialized before mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlannedUserPage<'a> {
    virtual_start: UserAddr,
    initial_bytes: &'a [u8],
    permissions: UserPermissions,
    kind: UserMappingKind,
}

impl<'a> PlannedUserPage<'a> {
    /// Return the aligned user virtual address of this page.
    pub const fn virtual_start(self) -> UserAddr {
        self.virtual_start
    }

    /// Return bytes copied at offset zero after the whole destination page is cleared.
    pub const fn initial_bytes(self) -> &'a [u8] {
        self.initial_bytes
    }

    /// Return the final EL0 permissions installed after population.
    pub const fn permissions(self) -> UserPermissions {
        self.permissions
    }

    /// Return why the page belongs to the image.
    pub const fn kind(self) -> UserMappingKind {
        self.kind
    }

    /// Return the zero-filled suffix length after initialized bytes.
    pub const fn zero_suffix_size(self) -> usize {
        PAGE_SIZE - self.initial_bytes.len()
    }
}

/// Complete, allocation-free plan for program segments and the initial stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessImagePlan<'a, const MAPPINGS: usize, const PAGES: usize> {
    address_space: UserAddressSpacePlan<MAPPINGS>,
    pages: [Option<PlannedUserPage<'a>>; PAGES],
    page_count: usize,
    entry: UserAddr,
    stack: UserStackLayout,
}

impl<'a, const MAPPINGS: usize, const PAGES: usize> ProcessImagePlan<'a, MAPPINGS, PAGES> {
    /// Combine a fully validated ELF image with one guarded initial stack.
    ///
    /// Construction mutates only local metadata. No caller-owned frame or page
    /// table can be left behind when an error is returned.
    pub fn new(image: ElfImage<'a>, stack: UserStackLayout) -> Result<Self, ProcessImageError> {
        let mut result = Self {
            address_space: UserAddressSpacePlan::new(),
            pages: [None; PAGES],
            page_count: 0,
            entry: image.entry(),
            stack,
        };

        for segment in image.load_segments() {
            let segment = segment.map_err(ProcessImageError::Elf)?;
            if segment.virtual_range().overlaps(stack.guard_range()) {
                return Err(ProcessImageError::StackGuardOverlap);
            }
            result
                .address_space
                .map(
                    segment.virtual_range(),
                    segment.permissions(),
                    UserMappingKind::Program,
                )
                .map_err(ProcessImageError::Address)?;

            let page_count = segment.virtual_range().byte_len() / PAGE_SIZE;
            for page_index in 0..page_count {
                let page_offset = page_index
                    .checked_mul(PAGE_SIZE)
                    .ok_or(ProcessImageError::PageCountOverflow)?;
                let virtual_start = segment
                    .virtual_range()
                    .start()
                    .checked_add(page_offset)
                    .map_err(ProcessImageError::Address)?;
                let initialized = if page_offset < segment.data().len() {
                    let end = page_offset
                        .checked_add(PAGE_SIZE)
                        .ok_or(ProcessImageError::PageCountOverflow)?
                        .min(segment.data().len());
                    &segment.data()[page_offset..end]
                } else {
                    &[]
                };
                result.insert_page(PlannedUserPage {
                    virtual_start,
                    initial_bytes: initialized,
                    permissions: segment.permissions(),
                    kind: UserMappingKind::Program,
                })?;
            }
        }

        if result
            .address_space
            .mappings()
            .iter()
            .any(|mapping| mapping.range().overlaps(stack.guard_range()))
        {
            return Err(ProcessImageError::StackGuardOverlap);
        }
        result
            .address_space
            .map(
                stack.usable_range(),
                UserPermissions::read_write(),
                UserMappingKind::Stack,
            )
            .map_err(ProcessImageError::Address)?;

        let stack_pages = stack.usable_range().byte_len() / PAGE_SIZE;
        for page_index in 0..stack_pages {
            let page_offset = page_index
                .checked_mul(PAGE_SIZE)
                .ok_or(ProcessImageError::PageCountOverflow)?;
            let virtual_start = stack
                .usable_range()
                .start()
                .checked_add(page_offset)
                .map_err(ProcessImageError::Address)?;
            result.insert_page(PlannedUserPage {
                virtual_start,
                initial_bytes: &[],
                permissions: UserPermissions::read_write(),
                kind: UserMappingKind::Stack,
            })?;
        }

        Ok(result)
    }

    /// Return the executable entry address.
    pub const fn entry(&self) -> UserAddr {
        self.entry
    }

    /// Return the guarded stack layout and initial stack pointer.
    pub const fn stack(&self) -> UserStackLayout {
        self.stack
    }

    /// Return the sorted virtual mapping plan.
    pub const fn address_space(&self) -> &UserAddressSpacePlan<MAPPINGS> {
        &self.address_space
    }

    /// Return per-page population work sorted by virtual address.
    pub fn pages(&self) -> impl ExactSizeIterator<Item = PlannedUserPage<'a>> + '_ {
        self.pages[..self.page_count]
            .iter()
            .map(|page| page.expect("active process-image page slot"))
    }

    /// Return the exact number of physical frames required by this image.
    pub const fn required_frames(&self) -> usize {
        self.page_count
    }

    /// Assign one physical frame to every planned page as one transaction.
    ///
    /// A private allocator snapshot is committed only after all requests
    /// succeed. Therefore both out-of-memory and allocator errors leave the
    /// caller's allocator byte-for-byte unchanged.
    pub fn allocate<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<AllocatedProcessImage<'a, MAPPINGS, PAGES>, ProcessImageAllocationError> {
        let mut candidate = *allocator;
        let mut frames = [None; PAGES];
        for slot in &mut frames[..self.page_count] {
            *slot = Some(
                candidate
                    .allocate()
                    .map_err(ProcessImageAllocationError::Allocator)?
                    .ok_or(ProcessImageAllocationError::OutOfMemory)?,
            );
        }
        *allocator = candidate;
        Ok(AllocatedProcessImage { plan: self, frames })
    }

    fn insert_page(&mut self, page: PlannedUserPage<'a>) -> Result<(), ProcessImageError> {
        if self.page_count == PAGES {
            return Err(ProcessImageError::PageCapacityExceeded);
        }
        let insertion = self.pages[..self.page_count].partition_point(|current| {
            current
                .expect("active process-image page slot")
                .virtual_start
                < page.virtual_start
        });
        self.pages
            .copy_within(insertion..self.page_count, insertion + 1);
        self.pages[insertion] = Some(page);
        self.page_count += 1;
        Ok(())
    }
}

/// One planned user page paired with its uniquely owned physical frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocatedUserPage<'a> {
    planned: PlannedUserPage<'a>,
    frame: PageFrame,
}

impl<'a> AllocatedUserPage<'a> {
    /// Return the virtual, contents, permissions, and purpose plan.
    pub const fn planned(self) -> PlannedUserPage<'a> {
        self.planned
    }

    /// Return the physical frame owned for this page.
    pub const fn frame(self) -> PageFrame {
        self.frame
    }
}

/// Complete page plan with ownership of every assigned physical frame.
#[derive(Debug, Eq, PartialEq)]
pub struct AllocatedProcessImage<'a, const MAPPINGS: usize, const PAGES: usize> {
    plan: ProcessImagePlan<'a, MAPPINGS, PAGES>,
    frames: [Option<PageFrame>; PAGES],
}

impl<'a, const MAPPINGS: usize, const PAGES: usize> AllocatedProcessImage<'a, MAPPINGS, PAGES> {
    /// Return the immutable planning metadata.
    pub const fn plan(&self) -> &ProcessImagePlan<'a, MAPPINGS, PAGES> {
        &self.plan
    }

    /// Iterate over virtual pages and their physical frames in virtual order.
    pub fn pages(&self) -> impl ExactSizeIterator<Item = AllocatedUserPage<'a>> + '_ {
        self.plan
            .pages()
            .enumerate()
            .map(|(index, planned)| AllocatedUserPage {
                planned,
                frame: self.frames[index].expect("allocated process-image frame slot"),
            })
    }

    /// Clear and initialize every owned frame without publishing any mapping.
    ///
    /// Pages are always cleared before source bytes are written. A failure
    /// returns this ownership object inside the error. Retrying is safe because
    /// a retry starts again by clearing every page.
    pub fn populate<M: ProcessImageMemory>(
        self,
        memory: &mut M,
    ) -> Result<
        PopulatedProcessImage<'a, MAPPINGS, PAGES>,
        ProcessImagePopulationError<'a, M::Error, MAPPINGS, PAGES>,
    > {
        for index in 0..self.plan.page_count {
            let planned = self.plan.pages[index].expect("active process-image page slot");
            let frame = self.frames[index].expect("allocated process-image frame slot");
            if let Err(error) = memory.clear_frame(frame) {
                return Err(ProcessImagePopulationError {
                    image: self,
                    page_index: index,
                    stage: PopulationStage::Clear,
                    error,
                });
            }
            if !planned.initial_bytes().is_empty()
                && let Err(error) = memory.write_frame(frame, 0, planned.initial_bytes())
            {
                return Err(ProcessImagePopulationError {
                    image: self,
                    page_index: index,
                    stage: PopulationStage::Initialize,
                    error,
                });
            }
        }
        Ok(PopulatedProcessImage { image: self })
    }

    /// Return every owned frame to the allocator as one transaction.
    ///
    /// On error, the allocator is unchanged and ownership is returned with the
    /// error so the caller can diagnose or retry without leaking the image.
    pub fn release<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<(), (Self, AllocationError)> {
        let mut candidate = *allocator;
        for frame in self.frames[..self.plan.page_count].iter().rev().flatten() {
            if let Err(error) = candidate.deallocate(*frame) {
                return Err((self, error));
            }
        }
        *allocator = candidate;
        Ok(())
    }
}

/// Process-image frames whose complete clear-and-copy plan has succeeded.
#[derive(Debug, Eq, PartialEq)]
pub struct PopulatedProcessImage<'a, const MAPPINGS: usize, const PAGES: usize> {
    image: AllocatedProcessImage<'a, MAPPINGS, PAGES>,
}

impl<'a, const MAPPINGS: usize, const PAGES: usize> PopulatedProcessImage<'a, MAPPINGS, PAGES> {
    /// Return the immutable executable and virtual mapping plan.
    pub const fn plan(&self) -> &ProcessImagePlan<'a, MAPPINGS, PAGES> {
        self.image.plan()
    }

    /// Iterate over populated frames in user virtual-address order.
    pub fn pages(&self) -> impl ExactSizeIterator<Item = AllocatedUserPage<'a>> + '_ {
        self.image.pages()
    }

    /// Return all frames to their allocator when the image was never published.
    ///
    /// A caller that has installed mappings to these frames must first retire
    /// those mappings and complete the architecture's TLB invalidation rules.
    pub fn release<const MEMORY_RANGES: usize>(
        self,
        allocator: &mut FrameAllocator<MEMORY_RANGES>,
    ) -> Result<(), (Self, AllocationError)> {
        match self.image.release(allocator) {
            Ok(()) => Ok(()),
            Err((image, error)) => Err((Self { image }, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use super::{
        PopulationStage, ProcessImageAllocationError, ProcessImageError, ProcessImageMemory,
        ProcessImagePlan,
    };
    use crate::elf::ElfImage;
    use crate::memory::{AddressRange, MemoryMap, PAGE_SIZE, PhysAddr};
    use crate::user::{
        UserAddr, UserAddressError, UserMappingKind, UserPermissions, UserStackLayout,
    };

    const TEXT_HEADER: usize = 64;
    const DATA_HEADER: usize = 64 + 56;

    #[test]
    fn combines_segments_stack_and_per_page_initialization() {
        let bytes = fixture(0x40_0000, 0x50_0000, PAGE_SIZE + 3, 2 * PAGE_SIZE + 7);
        let image = ElfImage::parse(&bytes).expect("valid executable");
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 2, 1).unwrap();
        let plan = ProcessImagePlan::<3, 7>::new(image, stack).expect("complete image plan");
        let pages: Vec<_> = plan.pages().collect();

        assert_eq!(plan.entry(), UserAddr::new(0x40_0000).unwrap());
        assert_eq!(plan.stack(), stack);
        assert_eq!(plan.required_frames(), 7);
        assert_eq!(plan.address_space().mappings().len(), 3);
        assert_eq!(pages[0].virtual_start(), UserAddr::new(0x40_0000).unwrap());
        assert_eq!(pages[0].initial_bytes(), &[0xa5; PAGE_SIZE][..]);
        assert_eq!(pages[0].permissions(), UserPermissions::read_execute());
        assert_eq!(pages[1].initial_bytes(), &[0xa5; 3]);
        assert_eq!(pages[1].zero_suffix_size(), PAGE_SIZE - 3);
        assert_eq!(pages[2].virtual_start(), UserAddr::new(0x50_0000).unwrap());
        assert_eq!(pages[4].initial_bytes(), &[0x5a; 7]);
        assert_eq!(pages[5].kind(), UserMappingKind::Stack);
        assert_eq!(pages[6].kind(), UserMappingKind::Stack);
        assert!(pages[5].initial_bytes().is_empty());
    }

    #[test]
    fn sorts_pages_even_when_program_headers_are_not_in_address_order() {
        let bytes = fixture(0x60_0000, 0x40_0000, 1, 1);
        let image = ElfImage::parse(&bytes).unwrap();
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 1, 1).unwrap();
        let plan = ProcessImagePlan::<3, 3>::new(image, stack).unwrap();
        let starts: Vec<_> = plan.pages().map(|page| page.virtual_start()).collect();

        assert_eq!(
            starts,
            [
                UserAddr::new(0x40_0000).unwrap(),
                UserAddr::new(0x60_0000).unwrap(),
                UserAddr::new(0x7f_f000).unwrap(),
            ]
        );
    }

    #[test]
    fn rejects_stack_guard_collision_and_metadata_exhaustion() {
        let bytes = fixture(0x40_0000, 0x50_0000, 1, 1);
        let image = ElfImage::parse(&bytes).unwrap();
        let colliding = UserStackLayout::new(UserAddr::new(0x50_2000).unwrap(), 1, 1).unwrap();
        assert_eq!(
            ProcessImagePlan::<3, 3>::new(image, colliding),
            Err(ProcessImageError::StackGuardOverlap)
        );

        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 1, 1).unwrap();
        assert_eq!(
            ProcessImagePlan::<2, 3>::new(image, stack),
            Err(ProcessImageError::Address(
                UserAddressError::CapacityExceeded
            ))
        );
        assert_eq!(
            ProcessImagePlan::<3, 2>::new(image, stack),
            Err(ProcessImageError::PageCapacityExceeded)
        );
    }

    #[test]
    fn frame_assignment_and_release_are_transactional() {
        let bytes = fixture(0x40_0000, 0x50_0000, 1, 1);
        let image = ElfImage::parse(&bytes).unwrap();
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 1, 1).unwrap();
        let plan = ProcessImagePlan::<3, 3>::new(image, stack).unwrap();
        let mut map = MemoryMap::<2>::new();
        map.add_usable(physical_bytes(0x10_0000, 0x10_3000))
            .unwrap();
        let mut allocator = map.into_allocator();
        let original = allocator;

        let allocated = plan.allocate(&mut allocator).expect("three frames");
        assert_eq!(allocator.total_free_frames(), 0);
        let physical: Vec<_> = allocated
            .pages()
            .map(|page| page.frame().start_address().as_usize())
            .collect();
        assert_eq!(physical, [0x10_0000, 0x10_1000, 0x10_2000]);
        allocated.release(&mut allocator).expect("release succeeds");
        assert_eq!(allocator, original);
    }

    #[test]
    fn out_of_memory_does_not_modify_allocator() {
        let bytes = fixture(0x40_0000, 0x50_0000, 1, 1);
        let image = ElfImage::parse(&bytes).unwrap();
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 1, 1).unwrap();
        let plan = ProcessImagePlan::<3, 3>::new(image, stack).unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(physical_bytes(0x20_0000, 0x20_2000))
            .unwrap();
        let mut allocator = map.into_allocator();
        let original = allocator;

        assert_eq!(
            plan.allocate(&mut allocator),
            Err(ProcessImageAllocationError::OutOfMemory)
        );
        assert_eq!(allocator, original);
    }

    #[test]
    fn failed_release_returns_ownership_and_does_not_modify_wrong_allocator() {
        let bytes = fixture(0x40_0000, 0x50_0000, 1, 1);
        let image = ElfImage::parse(&bytes).unwrap();
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 1, 1).unwrap();
        let plan = ProcessImagePlan::<3, 3>::new(image, stack).unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(physical_bytes(0x20_0000, 0x20_3000))
            .unwrap();
        let mut owner_allocator = map.into_allocator();
        let mut wrong_allocator = owner_allocator;
        let wrong_before = wrong_allocator;
        let allocated = plan.allocate(&mut owner_allocator).unwrap();

        let (allocated, error) = allocated
            .release(&mut wrong_allocator)
            .expect_err("frames are already free in the wrong snapshot");
        assert_eq!(error, crate::memory::AllocationError::DoubleFree);
        assert_eq!(wrong_allocator, wrong_before);
        allocated
            .release(&mut owner_allocator)
            .expect("ownership remains available after failed release");
    }

    #[test]
    fn population_clears_every_page_before_copying_initial_bytes() {
        let bytes = fixture(0x40_0000, 0x50_0000, PAGE_SIZE + 3, 1);
        let image = ElfImage::parse(&bytes).unwrap();
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 1, 1).unwrap();
        let plan = ProcessImagePlan::<3, 4>::new(image, stack).unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(physical_bytes(0x30_0000, 0x30_4000))
            .unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let mut memory = TestMemory::new(0x30_0000, 4);

        let populated = allocated
            .populate(&mut memory)
            .expect("population succeeds");
        assert!(memory.pages[0].iter().all(|byte| *byte == 0xa5));
        assert_eq!(&memory.pages[1][..3], &[0xa5; 3]);
        assert!(memory.pages[1][3..].iter().all(|byte| *byte == 0));
        assert_eq!(memory.pages[2][0], 0x5a);
        assert!(memory.pages[2][1..].iter().all(|byte| *byte == 0));
        assert!(memory.pages[3].iter().all(|byte| *byte == 0));
        populated.release(&mut allocator).unwrap();
    }

    #[test]
    fn population_failure_reports_stage_and_returns_retryable_ownership() {
        let bytes = fixture(0x40_0000, 0x50_0000, 1, 1);
        let image = ElfImage::parse(&bytes).unwrap();
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 1, 1).unwrap();
        let plan = ProcessImagePlan::<3, 3>::new(image, stack).unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(physical_bytes(0x40_0000, 0x40_3000))
            .unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let mut failing = TestMemory::new(0x40_0000, 3);
        failing.fail_write_for = Some(1);

        let error = allocated
            .populate(&mut failing)
            .expect_err("second page write fails");
        assert_eq!(error.page_index(), 1);
        assert_eq!(error.stage(), PopulationStage::Initialize);
        assert_eq!(error.error(), &TestMemoryError::Injected);
        let (allocated, backend_error) = error.into_parts();
        assert_eq!(backend_error, TestMemoryError::Injected);

        let mut retry = TestMemory::new(0x40_0000, 3);
        let populated = allocated
            .populate(&mut retry)
            .expect("retry clears all pages");
        assert_eq!(retry.clear_count, 3);
        populated.release(&mut allocator).unwrap();
    }

    fn physical_bytes(start: usize, end: usize) -> AddressRange<PhysAddr> {
        AddressRange::new(PhysAddr::new(start), PhysAddr::new(end)).unwrap()
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestMemoryError {
        UnknownFrame,
        InvalidWrite,
        Injected,
    }

    struct TestMemory {
        physical_base: usize,
        pages: Vec<Vec<u8>>,
        clear_count: usize,
        fail_write_for: Option<usize>,
    }

    impl TestMemory {
        fn new(physical_base: usize, page_count: usize) -> Self {
            Self {
                physical_base,
                pages: vec![vec![0xcc; PAGE_SIZE]; page_count],
                clear_count: 0,
                fail_write_for: None,
            }
        }

        fn index(&self, frame: crate::memory::PageFrame) -> Result<usize, TestMemoryError> {
            let offset = frame
                .start_address()
                .as_usize()
                .checked_sub(self.physical_base)
                .ok_or(TestMemoryError::UnknownFrame)?;
            if !offset.is_multiple_of(PAGE_SIZE) {
                return Err(TestMemoryError::UnknownFrame);
            }
            let index = offset / PAGE_SIZE;
            if index < self.pages.len() {
                Ok(index)
            } else {
                Err(TestMemoryError::UnknownFrame)
            }
        }
    }

    impl ProcessImageMemory for TestMemory {
        type Error = TestMemoryError;

        fn clear_frame(&mut self, frame: crate::memory::PageFrame) -> Result<(), Self::Error> {
            let index = self.index(frame)?;
            self.pages[index].fill(0);
            self.clear_count += 1;
            Ok(())
        }

        fn write_frame(
            &mut self,
            frame: crate::memory::PageFrame,
            offset: usize,
            bytes: &[u8],
        ) -> Result<(), Self::Error> {
            let index = self.index(frame)?;
            if self.fail_write_for == Some(index) {
                return Err(TestMemoryError::Injected);
            }
            let end = offset
                .checked_add(bytes.len())
                .filter(|end| *end <= PAGE_SIZE)
                .ok_or(TestMemoryError::InvalidWrite)?;
            self.pages[index][offset..end].copy_from_slice(bytes);
            Ok(())
        }
    }

    fn fixture(
        text_virtual: usize,
        data_virtual: usize,
        text_file_size: usize,
        data_file_size: usize,
    ) -> Vec<u8> {
        let text_offset = PAGE_SIZE;
        let data_offset = 4 * PAGE_SIZE;
        let data_memory_size = data_file_size.max(1);
        let mut bytes = vec![0_u8; data_offset + data_file_size];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[7] = 0;
        put_u16(&mut bytes, 16, 2);
        put_u16(&mut bytes, 18, 183);
        put_u32(&mut bytes, 20, 1);
        put_u64(&mut bytes, 24, text_virtual as u64);
        put_u64(&mut bytes, 32, 64);
        put_u16(&mut bytes, 52, 64);
        put_u16(&mut bytes, 54, 56);
        put_u16(&mut bytes, 56, 2);
        program_header(
            &mut bytes,
            TEXT_HEADER,
            5,
            text_offset,
            text_virtual,
            text_file_size,
            text_file_size,
        );
        program_header(
            &mut bytes,
            DATA_HEADER,
            6,
            data_offset,
            data_virtual,
            data_file_size,
            data_memory_size,
        );
        bytes[text_offset..text_offset + text_file_size].fill(0xa5);
        bytes[data_offset..data_offset + data_file_size].fill(0x5a);
        bytes
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "ELF fixture fields are clearer when explicit"
    )]
    fn program_header(
        bytes: &mut [u8],
        offset: usize,
        flags: u32,
        file_offset: usize,
        virtual_address: usize,
        file_size: usize,
        memory_size: usize,
    ) {
        put_u32(bytes, offset, 1);
        put_u32(bytes, offset + 4, flags);
        put_u64(bytes, offset + 8, file_offset as u64);
        put_u64(bytes, offset + 16, virtual_address as u64);
        put_u64(bytes, offset + 32, file_size as u64);
        put_u64(bytes, offset + 40, memory_size as u64);
        put_u64(bytes, offset + 48, PAGE_SIZE as u64);
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
