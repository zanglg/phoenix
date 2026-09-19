//! Guarded virtual slots and physical ownership for runtime kernel stacks.

use super::{AddressRange, AllocationError, FrameAllocator, PAGE_SIZE, PageFrame, VirtAddr};

/// Intended execution role of an owned kernel stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelStackKind {
    /// Stack used while a schedulable kernel thread executes.
    Thread,
    /// Stack reserved for architecture exception entry.
    Exception,
}

/// Failure while constructing a fixed virtual stack-slot pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelStackPoolError {
    /// Compile-time slot capacity is zero.
    NoSlots,
    /// Compile-time usable-page count is zero.
    NoUsablePages,
    /// The arena start is not base-page aligned.
    MisalignedArena,
    /// Slot-size or arena-range arithmetic overflowed.
    AddressOverflow,
}

/// Access needed before private frames can become stack pages.
pub trait KernelStackMemory {
    /// Backend-specific failure.
    type Error;

    /// Clear all `PAGE_SIZE` bytes in one private stack frame.
    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error>;
}

/// Failure while assigning a virtual slot and its physical frames.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelStackAllocationError<E> {
    /// No reusable virtual stack slot remains.
    SlotsExhausted,
    /// The physical allocator has fewer frames than one complete stack needs.
    OutOfMemory,
    /// The physical allocator rejected a request.
    Allocator(AllocationError),
    /// Clearing a private stack frame failed before publication.
    ClearFrame {
        /// Zero-based usable page index.
        page_index: usize,
        /// Backend-specific failure.
        error: E,
    },
}

/// Failure while returning an unpublished stack to its owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelStackReleaseError {
    /// The owner does not belong to this pool configuration.
    ForeignPool,
    /// The slot is not allocated with the owner's generation.
    InvalidOwner,
    /// The physical allocator rejected one of the owned frames.
    Allocator(AllocationError),
}

/// Exact virtual layout of one downward-growing guarded stack slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelStackLayout {
    lower_guard: AddressRange<VirtAddr>,
    usable: AddressRange<VirtAddr>,
    upper_guard: AddressRange<VirtAddr>,
}

impl KernelStackLayout {
    /// Return the mandatory unmapped page below the usable stack.
    pub const fn lower_guard(self) -> AddressRange<VirtAddr> {
        self.lower_guard
    }

    /// Return the page-aligned virtual range backed by private frames.
    pub const fn usable(self) -> AddressRange<VirtAddr> {
        self.usable
    }

    /// Return the mandatory unmapped page above the initial stack pointer.
    pub const fn upper_guard(self) -> AddressRange<VirtAddr> {
        self.upper_guard
    }

    /// Return the initial aligned stack pointer at the exclusive usable end.
    pub const fn initial_stack_pointer(self) -> VirtAddr {
        self.usable.end()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PoolIdentity {
    arena_start: usize,
    slot_size: usize,
    slot_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotState {
    Free { generation: u64 },
    Allocated { generation: u64 },
    Retired,
}

/// Fixed virtual arena that assigns guarded stack slots without allocating metadata.
#[derive(Debug, Eq, PartialEq)]
pub struct KernelStackPool<const SLOTS: usize, const STACK_PAGES: usize> {
    identity: PoolIdentity,
    slots: [SlotState; SLOTS],
}

impl<const SLOTS: usize, const STACK_PAGES: usize> KernelStackPool<SLOTS, STACK_PAGES> {
    /// Validate an arena containing one lower and one upper guard per slot.
    pub fn new(arena_start: VirtAddr) -> Result<Self, KernelStackPoolError> {
        if SLOTS == 0 {
            return Err(KernelStackPoolError::NoSlots);
        }
        if STACK_PAGES == 0 {
            return Err(KernelStackPoolError::NoUsablePages);
        }
        if !arena_start
            .is_aligned(PAGE_SIZE)
            .map_err(|_| KernelStackPoolError::MisalignedArena)?
        {
            return Err(KernelStackPoolError::MisalignedArena);
        }
        let slot_pages = STACK_PAGES
            .checked_add(2)
            .ok_or(KernelStackPoolError::AddressOverflow)?;
        let slot_size = slot_pages
            .checked_mul(PAGE_SIZE)
            .ok_or(KernelStackPoolError::AddressOverflow)?;
        let arena_size = slot_size
            .checked_mul(SLOTS)
            .ok_or(KernelStackPoolError::AddressOverflow)?;
        arena_start
            .checked_add(arena_size)
            .map_err(|_| KernelStackPoolError::AddressOverflow)?;
        Ok(Self {
            identity: PoolIdentity {
                arena_start: arena_start.as_usize(),
                slot_size,
                slot_count: SLOTS,
            },
            slots: [SlotState::Free { generation: 1 }; SLOTS],
        })
    }

    /// Return the complete reserved virtual arena.
    pub fn arena(&self) -> AddressRange<VirtAddr> {
        let end = self
            .identity
            .arena_start
            .checked_add(self.identity.slot_size * SLOTS)
            .expect("validated stack arena");
        AddressRange::new(VirtAddr::new(self.identity.arena_start), VirtAddr::new(end))
            .expect("validated stack arena order")
    }

    /// Return one slot layout, including both deliberately unmapped guards.
    pub fn layout(&self, slot: usize) -> Option<KernelStackLayout> {
        if slot >= SLOTS {
            return None;
        }
        Some(layout_for(self.identity, slot))
    }

    /// Transactionally reserve the lowest free slot, acquire frames, and clear them.
    pub fn allocate<const RANGES: usize, M: KernelStackMemory>(
        &mut self,
        allocator: &mut FrameAllocator<RANGES>,
        memory: &mut M,
        kind: KernelStackKind,
    ) -> Result<OwnedKernelStack<STACK_PAGES>, KernelStackAllocationError<M::Error>> {
        let Some((slot, generation)) = self.slots.iter().enumerate().find_map(|(index, state)| {
            if let SlotState::Free { generation } = state {
                Some((index, *generation))
            } else {
                None
            }
        }) else {
            return Err(KernelStackAllocationError::SlotsExhausted);
        };

        let mut candidate_allocator = *allocator;
        let mut frames = [None; STACK_PAGES];
        for frame_slot in &mut frames {
            let frame = candidate_allocator
                .allocate()
                .map_err(KernelStackAllocationError::Allocator)?
                .ok_or(KernelStackAllocationError::OutOfMemory)?;
            *frame_slot = Some(frame);
        }
        for (page_index, frame) in frames.iter().flatten().copied().enumerate() {
            if let Err(error) = memory.clear_frame(frame) {
                return Err(KernelStackAllocationError::ClearFrame { page_index, error });
            }
        }

        self.slots[slot] = SlotState::Allocated { generation };
        *allocator = candidate_allocator;
        Ok(OwnedKernelStack {
            pool: self.identity,
            slot,
            generation,
            kind,
            layout: layout_for(self.identity, slot),
            frames,
        })
    }

    /// Return the number of slots currently retaining frame ownership.
    pub fn allocated_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|state| matches!(state, SlotState::Allocated { .. }))
            .count()
    }
}

/// Private frames and virtual layout of one stack that has not been published.
///
/// Mapping publication must consume this owner into a future mapped-stack type.
/// Until that type exists, only an unpublished owner can be released safely.
#[derive(Debug, Eq, PartialEq)]
pub struct OwnedKernelStack<const STACK_PAGES: usize> {
    pool: PoolIdentity,
    slot: usize,
    generation: u64,
    kind: KernelStackKind,
    layout: KernelStackLayout,
    frames: [Option<PageFrame>; STACK_PAGES],
}

impl<const STACK_PAGES: usize> OwnedKernelStack<STACK_PAGES> {
    /// Return the intended execution role.
    pub const fn kind(&self) -> KernelStackKind {
        self.kind
    }

    /// Return the guarded virtual layout.
    pub const fn layout(&self) -> KernelStackLayout {
        self.layout
    }

    /// Return physical frames in increasing stack virtual-address order.
    pub fn frames(&self) -> impl ExactSizeIterator<Item = PageFrame> + DoubleEndedIterator + '_ {
        self.frames
            .iter()
            .map(|frame| frame.expect("complete owned kernel stack"))
    }

    /// Pair every usable virtual page with its owned physical frame.
    pub fn mappings(
        &self,
    ) -> impl ExactSizeIterator<Item = (VirtAddr, PageFrame)> + DoubleEndedIterator + '_ {
        let usable_start = self.layout.usable.start().as_usize();
        self.frames().enumerate().map(move |(index, frame)| {
            let offset = index
                .checked_mul(PAGE_SIZE)
                .expect("stack page index is bounded");
            (
                VirtAddr::new(
                    usable_start
                        .checked_add(offset)
                        .expect("validated stack virtual range"),
                ),
                frame,
            )
        })
    }

    /// Return unpublished frames and the virtual slot to their owners transactionally.
    pub fn release_unpublished<const SLOTS: usize, const RANGES: usize>(
        self,
        pool: &mut KernelStackPool<SLOTS, STACK_PAGES>,
        allocator: &mut FrameAllocator<RANGES>,
    ) -> Result<(), (Self, KernelStackReleaseError)> {
        if pool.identity != self.pool {
            return Err((self, KernelStackReleaseError::ForeignPool));
        }
        if pool.slots.get(self.slot)
            != Some(&SlotState::Allocated {
                generation: self.generation,
            })
        {
            return Err((self, KernelStackReleaseError::InvalidOwner));
        }

        let mut candidate_allocator = *allocator;
        for frame in self.frames.iter().rev().flatten() {
            if let Err(error) = candidate_allocator.deallocate(*frame) {
                return Err((self, KernelStackReleaseError::Allocator(error)));
            }
        }
        *allocator = candidate_allocator;
        pool.slots[self.slot] = match self.generation.checked_add(1) {
            Some(generation) => SlotState::Free { generation },
            None => SlotState::Retired,
        };
        Ok(())
    }
}

fn layout_for(identity: PoolIdentity, slot: usize) -> KernelStackLayout {
    let slot_start = identity
        .arena_start
        .checked_add(
            slot.checked_mul(identity.slot_size)
                .expect("validated slot"),
        )
        .expect("validated stack arena");
    let usable_start = slot_start
        .checked_add(PAGE_SIZE)
        .expect("validated lower guard");
    let upper_guard_start = slot_start
        .checked_add(identity.slot_size - PAGE_SIZE)
        .expect("validated upper guard");
    let slot_end = slot_start
        .checked_add(identity.slot_size)
        .expect("validated slot end");
    KernelStackLayout {
        lower_guard: AddressRange::new(VirtAddr::new(slot_start), VirtAddr::new(usable_start))
            .expect("ordered lower guard"),
        usable: AddressRange::new(
            VirtAddr::new(usable_start),
            VirtAddr::new(upper_guard_start),
        )
        .expect("ordered usable stack"),
        upper_guard: AddressRange::new(VirtAddr::new(upper_guard_start), VirtAddr::new(slot_end))
            .expect("ordered upper guard"),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::{
        KernelStackAllocationError, KernelStackKind, KernelStackMemory, KernelStackPool,
        KernelStackPoolError, KernelStackReleaseError,
    };
    use crate::arch::aarch64::kernel_page_table::KernelPageTablePlan;
    use crate::arch::aarch64::paging::MappingAttributes;
    use crate::memory::{AddressRange, MemoryMap, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct ClearError;

    struct TestMemory {
        cleared: Vec<PageFrame>,
        fail_at: Option<usize>,
    }

    impl KernelStackMemory for TestMemory {
        type Error = ClearError;

        fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
            if self.fail_at == Some(self.cleared.len()) {
                return Err(ClearError);
            }
            self.cleared.push(frame);
            Ok(())
        }
    }

    fn allocator<const RANGES: usize>(frames: usize) -> crate::memory::FrameAllocator<RANGES> {
        let mut map = MemoryMap::<RANGES>::new();
        map.add_usable(
            AddressRange::new(
                PhysAddr::new(PAGE_SIZE),
                PhysAddr::new((frames + 1) * PAGE_SIZE),
            )
            .expect("ordered bytes"),
        )
        .expect("usable memory");
        map.into_allocator()
    }

    #[test]
    fn pool_validates_capacity_alignment_and_overflow() {
        assert_eq!(
            KernelStackPool::<0, 2>::new(VirtAddr::new(0x1000)),
            Err(KernelStackPoolError::NoSlots)
        );
        assert_eq!(
            KernelStackPool::<2, 0>::new(VirtAddr::new(0x1000)),
            Err(KernelStackPoolError::NoUsablePages)
        );
        assert_eq!(
            KernelStackPool::<2, 2>::new(VirtAddr::new(0x1001)),
            Err(KernelStackPoolError::MisalignedArena)
        );
        assert_eq!(
            KernelStackPool::<2, 2>::new(VirtAddr::new(usize::MAX & !(PAGE_SIZE - 1))),
            Err(KernelStackPoolError::AddressOverflow)
        );
    }

    #[test]
    fn layouts_have_two_guards_and_aligned_initial_stack_pointer() {
        let pool = KernelStackPool::<2, 3>::new(VirtAddr::new(0x10000)).expect("pool");
        let first = pool.layout(0).expect("first");
        let second = pool.layout(1).expect("second");

        assert_eq!(first.lower_guard().len(), PAGE_SIZE);
        assert_eq!(first.usable().len(), 3 * PAGE_SIZE);
        assert_eq!(first.upper_guard().len(), PAGE_SIZE);
        assert_eq!(first.initial_stack_pointer(), first.usable().end());
        assert_eq!(first.upper_guard().end(), second.lower_guard().start());
        assert_eq!(pool.arena().len(), 2 * 5 * PAGE_SIZE);
        assert!(pool.layout(2).is_none());
    }

    #[test]
    fn allocation_owns_cleared_frames_and_exhausts_slots_explicitly() {
        let mut pool = KernelStackPool::<1, 2>::new(VirtAddr::new(0x20000)).expect("pool");
        let mut allocator = allocator::<2>(3);
        let mut memory = TestMemory {
            cleared: Vec::new(),
            fail_at: None,
        };

        let stack = pool
            .allocate(&mut allocator, &mut memory, KernelStackKind::Thread)
            .expect("stack");
        assert_eq!(stack.kind(), KernelStackKind::Thread);
        assert_eq!(stack.frames().count(), 2);
        assert_eq!(memory.cleared.len(), 2);
        assert_eq!(allocator.total_free_frames(), 1);
        assert_eq!(pool.allocated_slots(), 1);
        assert_eq!(
            pool.allocate(&mut allocator, &mut memory, KernelStackKind::Exception),
            Err(KernelStackAllocationError::SlotsExhausted)
        );
    }

    #[test]
    fn physical_oom_and_clear_failure_are_transactional() {
        let mut pool = KernelStackPool::<1, 2>::new(VirtAddr::new(0x30000)).expect("pool");
        let mut scarce_allocator = allocator::<2>(1);
        let allocator_before = scarce_allocator;
        let mut memory = TestMemory {
            cleared: Vec::new(),
            fail_at: None,
        };
        assert_eq!(
            pool.allocate(&mut scarce_allocator, &mut memory, KernelStackKind::Thread),
            Err(KernelStackAllocationError::OutOfMemory)
        );
        assert_eq!(scarce_allocator, allocator_before);
        assert_eq!(pool.allocated_slots(), 0);

        let mut allocator = allocator::<2>(2);
        let allocator_before = allocator;
        memory.cleared.clear();
        memory.fail_at = Some(1);
        assert_eq!(
            pool.allocate(&mut allocator, &mut memory, KernelStackKind::Thread),
            Err(KernelStackAllocationError::ClearFrame {
                page_index: 1,
                error: ClearError,
            })
        );
        assert_eq!(allocator, allocator_before);
        assert_eq!(pool.allocated_slots(), 0);
    }

    #[test]
    fn owned_stack_pages_map_rw_nx_while_guards_stay_absent() {
        let arena = VirtAddr::new(0xffff_ff90_0000_0000);
        let mut pool = KernelStackPool::<1, 2>::new(arena).expect("pool");
        let mut allocator = allocator::<4>(8);
        let mut memory = TestMemory {
            cleared: Vec::new(),
            fail_at: None,
        };
        let stack = pool
            .allocate(&mut allocator, &mut memory, KernelStackKind::Thread)
            .expect("stack");
        let mut plan = KernelPageTablePlan::<4, 4>::new().expect("plan");
        for (virtual_page, frame) in stack.mappings() {
            plan.map_range(
                virtual_page,
                frame.start_address(),
                PAGE_SIZE,
                MappingAttributes::kernel_data(),
            )
            .expect("stack page mapping");
        }

        let layout = stack.layout();
        assert_eq!(plan.translate(layout.lower_guard().start()), Ok(None));
        assert_eq!(plan.translate(layout.upper_guard().start()), Ok(None));
        for (virtual_page, frame) in stack.mappings() {
            let translation = plan
                .translate(virtual_page)
                .expect("translation result")
                .expect("mapped stack page");
            assert_eq!(translation.physical_address, frame.start_address());
            assert_eq!(translation.attributes, MappingAttributes::kernel_data());
        }
    }

    #[test]
    fn unpublished_release_restores_frames_and_reuses_slot_generation() {
        let mut pool = KernelStackPool::<1, 2>::new(VirtAddr::new(0x40000)).expect("pool");
        let mut allocator = allocator::<2>(2);
        let mut memory = TestMemory {
            cleared: Vec::new(),
            fail_at: None,
        };
        let first = pool
            .allocate(&mut allocator, &mut memory, KernelStackKind::Thread)
            .expect("first");
        first
            .release_unpublished(&mut pool, &mut allocator)
            .expect("release");
        assert_eq!(allocator.total_free_frames(), 2);
        assert_eq!(pool.allocated_slots(), 0);

        let second = pool
            .allocate(&mut allocator, &mut memory, KernelStackKind::Exception)
            .expect("second");
        assert_eq!(second.kind(), KernelStackKind::Exception);
    }

    #[test]
    fn release_through_foreign_pool_retains_owner_and_allocators() {
        let mut pool = KernelStackPool::<1, 1>::new(VirtAddr::new(0x50000)).expect("pool");
        let mut foreign = KernelStackPool::<1, 1>::new(VirtAddr::new(0x60000)).expect("foreign");
        let mut owning_allocator = allocator::<2>(1);
        let allocator_before_allocation = owning_allocator;
        let mut memory = TestMemory {
            cleared: Vec::new(),
            fail_at: None,
        };
        let stack = pool
            .allocate(&mut owning_allocator, &mut memory, KernelStackKind::Thread)
            .expect("stack");
        let allocator_after_allocation = owning_allocator;

        let (stack, error) = stack
            .release_unpublished(&mut foreign, &mut owning_allocator)
            .expect_err("foreign release");
        assert_eq!(error, KernelStackReleaseError::ForeignPool);
        assert_eq!(owning_allocator, allocator_after_allocation);
        assert_eq!(pool.allocated_slots(), 1);
        assert_eq!(foreign.allocated_slots(), 0);

        let mut wrong_allocator = allocator::<2>(1);
        let wrong_before = wrong_allocator;
        let (stack, error) = stack
            .release_unpublished(&mut pool, &mut wrong_allocator)
            .expect_err("wrong allocator release");
        assert_eq!(
            error,
            KernelStackReleaseError::Allocator(crate::memory::AllocationError::DoubleFree)
        );
        assert_eq!(wrong_allocator, wrong_before);
        assert_eq!(pool.allocated_slots(), 1);

        stack
            .release_unpublished(&mut pool, &mut owning_allocator)
            .expect("owning release");
        assert_eq!(owning_allocator, allocator_before_allocation);
    }
}
