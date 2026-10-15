//! Fallible fixed-metadata allocation over uniquely owned backing frames.

use super::{
    AddressError, AllocationError, FrameAllocator, FrameRange, PAGE_SIZE, PageFrame, VirtAddr,
};

/// Access needed while turning private physical frames into heap backing.
///
/// The implementation chooses the virtual mapping and clears every frame before
/// the heap can publish allocations. It must not make the frames visible to a
/// competing owner.
pub trait HeapBackingMemory {
    /// Backend-specific failure.
    type Error;

    /// Return the virtual start of this exact contiguous backing range.
    fn virtual_start(&mut self, frames: FrameRange) -> Result<VirtAddr, Self::Error>;

    /// Clear all `PAGE_SIZE` bytes in one private backing frame.
    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error>;
}

/// Failure while acquiring and preparing heap backing frames.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeapInitializationError<E> {
    /// The heap already owns backing frames.
    AlreadyInitialized,
    /// A zero frame count or non-power-of-two frame alignment was requested.
    InvalidRequest,
    /// Compile-time allocation metadata capacity is zero.
    NoAllocationMetadata,
    /// The physical allocator has no suitable contiguous frame range.
    OutOfMemory,
    /// The physical allocator rejected the request.
    Allocator(AllocationError),
    /// Backing byte-count or virtual-range arithmetic overflowed.
    Address(AddressError),
    /// The backend could not resolve the range's virtual start.
    VirtualStart(E),
    /// The backend returned a virtual start that is not base-page aligned.
    MisalignedVirtualStart,
    /// Clearing a private frame failed before publication.
    ClearFrame {
        /// Zero-based frame index within the backing range.
        frame_index: usize,
        /// Backend-specific failure.
        error: E,
    },
    /// The heap lifecycle epoch cannot be advanced without aliasing old tokens.
    EpochExhausted,
}

/// Failure while allocating bytes from an initialized heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeapAllocationError {
    /// No backing frames have been initialized.
    NotInitialized,
    /// Zero-size allocations are rejected rather than assigned a magic address.
    ZeroSize,
    /// Alignment was zero or was not a power of two.
    InvalidAlignment,
    /// Address or extent arithmetic overflowed.
    Overflow,
    /// Free bytes exist, but the fixed live-allocation metadata is full.
    MetadataExhausted,
    /// No suitably aligned contiguous byte range can satisfy the request.
    OutOfMemory,
    /// A new allocation token cannot be issued without aliasing an old token.
    AllocationIdExhausted,
}

/// Failure while returning one heap allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeapDeallocationError {
    /// No backing frames have been initialized.
    NotInitialized,
    /// The token belongs to a different heap backing or lifecycle epoch.
    ForeignAllocation,
    /// The token's recorded extent does not match the active allocation.
    InvalidAllocation,
    /// The exact allocation token has already been released.
    DoubleFree,
}

/// Failure while returning all heap backing frames to the physical allocator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeapReleaseError {
    /// No backing frames have been initialized.
    NotInitialized,
    /// Live allocations still retain bytes in the backing range.
    Busy {
        /// Number of allocations that must be released first.
        live_allocations: usize,
    },
    /// The physical allocator rejected the owned range.
    Allocator(AllocationError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeapIdentity {
    backing_start_frame: usize,
    backing_frame_count: usize,
    virtual_start: usize,
    epoch: u64,
}

/// One allocation token issued by a [`KernelHeap`].
///
/// Tokens are copyable so stale and duplicate releases are detected explicitly.
/// They do not dereference memory; consumers must retain the token for the
/// lifetime of every reference derived from its virtual address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeapAllocation {
    identity: HeapIdentity,
    id: u64,
    address: VirtAddr,
    size: usize,
    alignment: usize,
}

impl HeapAllocation {
    /// Return the first allocated virtual byte.
    pub const fn address(self) -> VirtAddr {
        self.address
    }

    /// Return the exact requested byte count.
    pub const fn size(self) -> usize {
        self.size
    }

    /// Return the guaranteed power-of-two byte alignment.
    pub const fn alignment(self) -> usize {
        self.alignment
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AllocationRecord {
    id: u64,
    offset: usize,
    size: usize,
    alignment: usize,
}

impl AllocationRecord {
    fn end(self) -> Result<usize, HeapAllocationError> {
        self.offset
            .checked_add(self.size)
            .ok_or(HeapAllocationError::Overflow)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReadyHeap<const ALLOCATIONS: usize> {
    identity: HeapIdentity,
    backing: FrameRange,
    capacity: usize,
    records: [Option<AllocationRecord>; ALLOCATIONS],
    live: usize,
    next_id: u64,
}

/// A single-core, fixed-metadata kernel heap with explicit physical ownership.
///
/// This type deliberately does not implement Rust's infallible global allocator
/// interface. Every request reports exhaustion, and callers retain explicit
/// allocation tokens until release.
#[derive(Debug, Eq, PartialEq)]
pub struct KernelHeap<const ALLOCATIONS: usize> {
    ready: Option<ReadyHeap<ALLOCATIONS>>,
    next_epoch: u64,
}

impl<const ALLOCATIONS: usize> KernelHeap<ALLOCATIONS> {
    /// Construct an uninitialized heap without acquiring frames.
    pub const fn new() -> Self {
        Self {
            ready: None,
            next_epoch: 1,
        }
    }

    /// Return whether this heap currently owns backing frames.
    pub const fn is_initialized(&self) -> bool {
        self.ready.is_some()
    }

    /// Acquire, clear, and publish one contiguous backing range transactionally.
    ///
    /// The allocator and heap remain unchanged on every failure. Backend writes
    /// before a clear failure may have zeroed frames, but those frames are not
    /// published and remain free in the original allocator.
    pub fn initialize<const RANGES: usize, M: HeapBackingMemory>(
        &mut self,
        allocator: &mut FrameAllocator<RANGES>,
        memory: &mut M,
        frame_count: usize,
        alignment_in_frames: usize,
    ) -> Result<(), HeapInitializationError<M::Error>> {
        if self.ready.is_some() {
            return Err(HeapInitializationError::AlreadyInitialized);
        }
        if frame_count == 0 || !alignment_in_frames.is_power_of_two() {
            return Err(HeapInitializationError::InvalidRequest);
        }
        if ALLOCATIONS == 0 {
            return Err(HeapInitializationError::NoAllocationMetadata);
        }
        let epoch = self.next_epoch;
        let next_epoch = epoch
            .checked_add(1)
            .ok_or(HeapInitializationError::EpochExhausted)?;
        let capacity = frame_count
            .checked_mul(PAGE_SIZE)
            .ok_or(HeapInitializationError::Address(AddressError::Overflow))?;

        let mut candidate_allocator = *allocator;
        let backing = candidate_allocator
            .allocate_contiguous(frame_count, alignment_in_frames)
            .map_err(HeapInitializationError::Allocator)?
            .ok_or(HeapInitializationError::OutOfMemory)?;
        let virtual_start = memory
            .virtual_start(backing)
            .map_err(HeapInitializationError::VirtualStart)?;
        if !virtual_start
            .is_aligned(PAGE_SIZE)
            .map_err(HeapInitializationError::Address)?
        {
            return Err(HeapInitializationError::MisalignedVirtualStart);
        }
        virtual_start
            .checked_add(capacity)
            .map_err(HeapInitializationError::Address)?;

        for frame_index in 0..frame_count {
            let frame = frame_at(backing, frame_index);
            if let Err(error) = memory.clear_frame(frame) {
                return Err(HeapInitializationError::ClearFrame { frame_index, error });
            }
        }

        let identity = HeapIdentity {
            backing_start_frame: backing.start_frame_number(),
            backing_frame_count: backing.len(),
            virtual_start: virtual_start.as_usize(),
            epoch,
        };
        self.ready = Some(ReadyHeap {
            identity,
            backing,
            capacity,
            records: [None; ALLOCATIONS],
            live: 0,
            next_id: 1,
        });
        self.next_epoch = next_epoch;
        *allocator = candidate_allocator;
        Ok(())
    }

    /// Allocate a nonzero byte range with exact power-of-two alignment.
    pub fn allocate(
        &mut self,
        size: usize,
        alignment: usize,
    ) -> Result<HeapAllocation, HeapAllocationError> {
        let ready = self
            .ready
            .as_mut()
            .ok_or(HeapAllocationError::NotInitialized)?;
        if size == 0 {
            return Err(HeapAllocationError::ZeroSize);
        }
        if !alignment.is_power_of_two() {
            return Err(HeapAllocationError::InvalidAlignment);
        }
        if ready.live == ALLOCATIONS {
            return Err(HeapAllocationError::MetadataExhausted);
        }
        let id = ready.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or(HeapAllocationError::AllocationIdExhausted)?;
        let offset = first_fit(ready, size, alignment)?;
        let address = VirtAddr::new(
            ready
                .identity
                .virtual_start
                .checked_add(offset)
                .ok_or(HeapAllocationError::Overflow)?,
        );
        let record = AllocationRecord {
            id,
            offset,
            size,
            alignment,
        };
        insert_record(ready, record);
        ready.next_id = next_id;
        Ok(HeapAllocation {
            identity: ready.identity,
            id,
            address,
            size,
            alignment,
        })
    }

    /// Release one exact allocation token.
    pub fn deallocate(&mut self, allocation: HeapAllocation) -> Result<(), HeapDeallocationError> {
        let ready = self
            .ready
            .as_mut()
            .ok_or(HeapDeallocationError::NotInitialized)?;
        if allocation.identity != ready.identity {
            return Err(HeapDeallocationError::ForeignAllocation);
        }
        let Some(index) = ready.records[..ready.live]
            .iter()
            .position(|record| record.is_some_and(|record| record.id == allocation.id))
        else {
            return if allocation.id > 0 && allocation.id < ready.next_id {
                Err(HeapDeallocationError::DoubleFree)
            } else {
                Err(HeapDeallocationError::InvalidAllocation)
            };
        };
        let record = ready.records[index].expect("located active heap allocation");
        let expected_address = ready
            .identity
            .virtual_start
            .checked_add(record.offset)
            .expect("validated heap extent address");
        if allocation.address.as_usize() != expected_address
            || allocation.size != record.size
            || allocation.alignment != record.alignment
        {
            return Err(HeapDeallocationError::InvalidAllocation);
        }
        ready.records.copy_within(index + 1..ready.live, index);
        ready.live -= 1;
        ready.records[ready.live] = None;
        Ok(())
    }

    /// Return all backing frames after every allocation has been released.
    ///
    /// The allocator and heap remain unchanged if range release fails.
    pub fn release_backing<const RANGES: usize>(
        &mut self,
        allocator: &mut FrameAllocator<RANGES>,
    ) -> Result<(), HeapReleaseError> {
        let ready = self.ready.ok_or(HeapReleaseError::NotInitialized)?;
        if ready.live != 0 {
            return Err(HeapReleaseError::Busy {
                live_allocations: ready.live,
            });
        }
        let mut candidate_allocator = *allocator;
        candidate_allocator
            .deallocate_contiguous(ready.backing)
            .map_err(HeapReleaseError::Allocator)?;
        *allocator = candidate_allocator;
        self.ready = None;
        Ok(())
    }

    /// Return the exact number of backing bytes, or zero before initialization.
    pub const fn capacity(&self) -> usize {
        match self.ready {
            Some(ready) => ready.capacity,
            None => 0,
        }
    }

    /// Return the number of bytes not held by live allocations.
    pub fn free_bytes(&self) -> usize {
        let Some(ready) = self.ready else {
            return 0;
        };
        let used: usize = ready.records[..ready.live]
            .iter()
            .flatten()
            .map(|record| record.size)
            .sum();
        ready.capacity - used
    }

    /// Return the number of live allocation tokens.
    pub const fn live_allocations(&self) -> usize {
        match self.ready {
            Some(ready) => ready.live,
            None => 0,
        }
    }

    /// Return the uniquely owned physical backing range while initialized.
    pub const fn backing_frames(&self) -> Option<FrameRange> {
        match self.ready {
            Some(ready) => Some(ready.backing),
            None => None,
        }
    }
}

impl<const ALLOCATIONS: usize> Default for KernelHeap<ALLOCATIONS> {
    fn default() -> Self {
        Self::new()
    }
}

fn first_fit<const ALLOCATIONS: usize>(
    ready: &ReadyHeap<ALLOCATIONS>,
    size: usize,
    alignment: usize,
) -> Result<usize, HeapAllocationError> {
    let base = ready.identity.virtual_start;
    let mut cursor = 0;
    for record in ready.records[..ready.live].iter().flatten().copied() {
        let start = align_offset(base, cursor, alignment)?;
        let end = start
            .checked_add(size)
            .ok_or(HeapAllocationError::Overflow)?;
        if end <= record.offset {
            return Ok(start);
        }
        cursor = record.end()?;
    }
    let start = align_offset(base, cursor, alignment)?;
    let end = start
        .checked_add(size)
        .ok_or(HeapAllocationError::Overflow)?;
    if end <= ready.capacity {
        Ok(start)
    } else {
        Err(HeapAllocationError::OutOfMemory)
    }
}

fn align_offset(
    base: usize,
    offset: usize,
    alignment: usize,
) -> Result<usize, HeapAllocationError> {
    let address = base
        .checked_add(offset)
        .ok_or(HeapAllocationError::Overflow)?;
    let mask = alignment - 1;
    let aligned = address
        .checked_add(mask)
        .ok_or(HeapAllocationError::Overflow)?
        & !mask;
    aligned
        .checked_sub(base)
        .ok_or(HeapAllocationError::Overflow)
}

fn insert_record<const ALLOCATIONS: usize>(
    ready: &mut ReadyHeap<ALLOCATIONS>,
    record: AllocationRecord,
) {
    let index = ready.records[..ready.live]
        .iter()
        .position(|current| current.is_some_and(|current| record.offset < current.offset))
        .unwrap_or(ready.live);
    ready.records.copy_within(index..ready.live, index + 1);
    ready.records[index] = Some(record);
    ready.live += 1;
}

fn frame_at(frames: FrameRange, index: usize) -> PageFrame {
    let number = frames
        .start_frame_number()
        .checked_add(index)
        .expect("frame index is bounded by an allocated range");
    let address = number
        .checked_mul(PAGE_SIZE)
        .expect("allocated frame numbers originate from physical addresses");
    PageFrame::from_start(super::PhysAddr::new(address)).expect("frame number is page aligned")
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use super::{
        HeapAllocationError, HeapBackingMemory, HeapDeallocationError, HeapInitializationError,
        HeapReleaseError, KernelHeap,
    };
    use crate::memory::{AddressRange, MemoryMap, PAGE_SIZE, PageFrame, PhysAddr, VirtAddr};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum MemoryError {
        VirtualStart,
        Clear,
    }

    struct TestMemory {
        virtual_start: Result<VirtAddr, MemoryError>,
        fail_clear: Option<usize>,
        cleared: Vec<PageFrame>,
    }

    impl TestMemory {
        fn direct_mapped(virtual_start: usize) -> Self {
            Self {
                virtual_start: Ok(VirtAddr::new(virtual_start)),
                fail_clear: None,
                cleared: Vec::new(),
            }
        }
    }

    impl HeapBackingMemory for TestMemory {
        type Error = MemoryError;

        fn virtual_start(
            &mut self,
            _frames: crate::memory::FrameRange,
        ) -> Result<VirtAddr, Self::Error> {
            self.virtual_start
        }

        fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
            let index = self.cleared.len();
            if self.fail_clear == Some(index) {
                return Err(MemoryError::Clear);
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
            .expect("ordered range"),
        )
        .expect("usable memory");
        map.into_allocator()
    }

    #[test]
    fn initialization_acquires_and_clears_unique_backing() {
        let mut allocator = allocator::<2>(8);
        let mut memory = TestMemory::direct_mapped(0xffff_8000_0000_1000);
        let mut heap = KernelHeap::<8>::new();

        heap.initialize(&mut allocator, &mut memory, 3, 2)
            .expect("initialize heap");

        assert!(heap.is_initialized());
        assert_eq!(heap.capacity(), 3 * PAGE_SIZE);
        assert_eq!(heap.free_bytes(), 3 * PAGE_SIZE);
        assert_eq!(
            heap.backing_frames().expect("backing").start_frame_number(),
            2
        );
        assert_eq!(memory.cleared.len(), 3);
        assert_eq!(allocator.total_free_frames(), 5);
    }

    #[test]
    fn initialization_failures_are_transactional_and_retryable() {
        let mut allocator = allocator::<2>(4);
        let allocator_before = allocator;
        let mut heap = KernelHeap::<4>::new();
        let mut memory = TestMemory::direct_mapped(0xffff_8000_0000_1000);
        memory.fail_clear = Some(1);

        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 2, 1),
            Err(HeapInitializationError::ClearFrame {
                frame_index: 1,
                error: MemoryError::Clear,
            })
        );
        assert_eq!(allocator, allocator_before);
        assert!(!heap.is_initialized());

        memory.fail_clear = None;
        heap.initialize(&mut allocator, &mut memory, 2, 1)
            .expect("retry initialization");
        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 1, 1),
            Err(HeapInitializationError::AlreadyInitialized)
        );
    }

    #[test]
    fn initialization_rejects_invalid_exhausted_and_bad_virtual_ranges() {
        let mut allocator = allocator::<2>(2);
        let mut heap = KernelHeap::<2>::new();
        let mut memory = TestMemory::direct_mapped(0x1000);

        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 0, 1),
            Err(HeapInitializationError::InvalidRequest)
        );
        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 1, 3),
            Err(HeapInitializationError::InvalidRequest)
        );
        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 3, 1),
            Err(HeapInitializationError::OutOfMemory)
        );

        memory.virtual_start = Err(MemoryError::VirtualStart);
        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 1, 1),
            Err(HeapInitializationError::VirtualStart(
                MemoryError::VirtualStart
            ))
        );
        memory.virtual_start = Ok(VirtAddr::new(0x1001));
        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 1, 1),
            Err(HeapInitializationError::MisalignedVirtualStart)
        );
        memory.virtual_start = Ok(VirtAddr::new(usize::MAX & !(PAGE_SIZE - 1)));
        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 1, 1),
            Err(HeapInitializationError::Address(
                crate::memory::AddressError::Overflow
            ))
        );
    }

    #[test]
    fn zero_metadata_capacity_is_an_explicit_configuration_error() {
        let mut allocator = allocator::<1>(1);
        let mut memory = TestMemory::direct_mapped(0x1000);
        let mut heap = KernelHeap::<0>::new();

        assert_eq!(
            heap.initialize(&mut allocator, &mut memory, 1, 1),
            Err(HeapInitializationError::NoAllocationMetadata)
        );
    }

    #[test]
    fn allocation_honors_alignment_and_reports_request_errors() {
        let mut allocator = allocator::<2>(2);
        let mut memory = TestMemory::direct_mapped(0x18000);
        let mut heap = KernelHeap::<4>::new();

        assert_eq!(
            heap.allocate(1, 1),
            Err(HeapAllocationError::NotInitialized)
        );
        heap.initialize(&mut allocator, &mut memory, 2, 1)
            .expect("initialize");
        assert_eq!(heap.allocate(0, 1), Err(HeapAllocationError::ZeroSize));
        assert_eq!(
            heap.allocate(1, 0),
            Err(HeapAllocationError::InvalidAlignment)
        );
        assert_eq!(
            heap.allocate(1, 3),
            Err(HeapAllocationError::InvalidAlignment)
        );

        let byte = heap.allocate(1, 1).expect("byte");
        let aligned = heap.allocate(32, 64).expect("aligned block");
        assert_eq!(byte.address().as_usize(), 0x18000);
        assert_eq!(aligned.address().as_usize() % 64, 0);
        assert_eq!(aligned.size(), 32);
        assert_eq!(aligned.alignment(), 64);
    }

    #[test]
    fn fragmentation_exhaustion_and_coalescing_are_deterministic() {
        let mut allocator = allocator::<2>(1);
        let mut memory = TestMemory::direct_mapped(0x20000);
        let mut heap = KernelHeap::<8>::new();
        heap.initialize(&mut allocator, &mut memory, 1, 1)
            .expect("initialize");

        let first = heap.allocate(1024, 1).expect("first");
        let middle = heap.allocate(2048, 1).expect("middle");
        let last = heap.allocate(1024, 1).expect("last");
        assert_eq!(heap.allocate(1, 1), Err(HeapAllocationError::OutOfMemory));

        heap.deallocate(first).expect("free first");
        heap.deallocate(last).expect("free last");
        assert_eq!(heap.free_bytes(), 2048);
        assert_eq!(
            heap.allocate(1536, 1),
            Err(HeapAllocationError::OutOfMemory)
        );

        heap.deallocate(middle).expect("free middle and coalesce");
        let whole = heap.allocate(PAGE_SIZE, PAGE_SIZE).expect("whole page");
        assert_eq!(whole.address().as_usize(), 0x20000);
        assert_eq!(whole.id, last.id + 1, "failed requests do not consume ids");
    }

    #[test]
    fn metadata_exhaustion_is_distinct_from_byte_exhaustion() {
        let mut allocator = allocator::<2>(1);
        let mut memory = TestMemory::direct_mapped(0x30000);
        let mut heap = KernelHeap::<2>::new();
        heap.initialize(&mut allocator, &mut memory, 1, 1)
            .expect("initialize");

        let _first = heap.allocate(1, 1).expect("first");
        let _second = heap.allocate(1, 1).expect("second");
        assert_eq!(
            heap.allocate(1, 1),
            Err(HeapAllocationError::MetadataExhausted)
        );
        assert_eq!(heap.free_bytes(), PAGE_SIZE - 2);
    }

    #[test]
    fn duplicate_foreign_and_modified_tokens_are_rejected() {
        let mut first_allocator = allocator::<2>(2);
        let mut second_allocator = allocator::<2>(2);
        let mut first_memory = TestMemory::direct_mapped(0x40000);
        let mut second_memory = TestMemory::direct_mapped(0x50000);
        let mut first_heap = KernelHeap::<4>::new();
        let mut second_heap = KernelHeap::<4>::new();
        first_heap
            .initialize(&mut first_allocator, &mut first_memory, 1, 1)
            .expect("first heap");
        second_heap
            .initialize(&mut second_allocator, &mut second_memory, 1, 1)
            .expect("second heap");
        let token = first_heap.allocate(32, 8).expect("allocation");

        assert_eq!(
            second_heap.deallocate(token),
            Err(HeapDeallocationError::ForeignAllocation)
        );
        let mut modified = token;
        modified.size += 1;
        assert_eq!(
            first_heap.deallocate(modified),
            Err(HeapDeallocationError::InvalidAllocation)
        );
        first_heap.deallocate(token).expect("release");
        assert_eq!(
            first_heap.deallocate(token),
            Err(HeapDeallocationError::DoubleFree)
        );
    }

    #[test]
    fn backing_release_requires_empty_heap_and_invalidates_old_epoch() {
        let mut allocator = allocator::<2>(2);
        let mut memory = TestMemory::direct_mapped(0x60000);
        let mut heap = KernelHeap::<4>::new();

        assert_eq!(
            heap.release_backing(&mut allocator),
            Err(HeapReleaseError::NotInitialized)
        );
        heap.initialize(&mut allocator, &mut memory, 2, 1)
            .expect("initialize");
        let old = heap.allocate(16, 8).expect("allocation");
        assert_eq!(
            heap.release_backing(&mut allocator),
            Err(HeapReleaseError::Busy {
                live_allocations: 1
            })
        );
        heap.deallocate(old).expect("release allocation");
        heap.release_backing(&mut allocator)
            .expect("release backing");
        assert_eq!(allocator.total_free_frames(), 2);

        heap.initialize(&mut allocator, &mut memory, 2, 1)
            .expect("reinitialize");
        assert_eq!(
            heap.deallocate(old),
            Err(HeapDeallocationError::ForeignAllocation)
        );
    }

    #[test]
    fn backing_release_failure_retains_heap_and_both_allocators() {
        let mut owning_allocator = allocator::<2>(2);
        let mut foreign_allocator = allocator::<2>(2);
        let foreign_before = foreign_allocator;
        let mut memory = TestMemory::direct_mapped(0x68000);
        let mut heap = KernelHeap::<2>::new();
        heap.initialize(&mut owning_allocator, &mut memory, 2, 1)
            .expect("initialize");
        let backing = heap.backing_frames();

        assert_eq!(
            heap.release_backing(&mut foreign_allocator),
            Err(HeapReleaseError::Allocator(
                crate::memory::AllocationError::DoubleFree
            ))
        );
        assert_eq!(foreign_allocator, foreign_before);
        assert_eq!(heap.backing_frames(), backing);

        heap.release_backing(&mut owning_allocator)
            .expect("release through owning allocator");
    }

    #[test]
    fn host_storage_can_observe_full_backing_clear() {
        struct Storage {
            bytes: Vec<u8>,
            frames: Vec<PageFrame>,
        }

        impl HeapBackingMemory for Storage {
            type Error = ();

            fn virtual_start(
                &mut self,
                _frames: crate::memory::FrameRange,
            ) -> Result<VirtAddr, Self::Error> {
                Ok(VirtAddr::new(0x70000))
            }

            fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
                self.frames.push(frame);
                let start = (self.frames.len() - 1) * PAGE_SIZE;
                self.bytes[start..start + PAGE_SIZE].fill(0);
                Ok(())
            }
        }

        let mut allocator = allocator::<2>(2);
        let mut storage = Storage {
            bytes: vec![0xa5; 2 * PAGE_SIZE],
            frames: Vec::new(),
        };
        let mut heap = KernelHeap::<2>::new();
        heap.initialize(&mut allocator, &mut storage, 2, 1)
            .expect("initialize");

        assert!(storage.bytes.iter().all(|byte| *byte == 0));
        assert_eq!(storage.frames.len(), 2);
    }
}
