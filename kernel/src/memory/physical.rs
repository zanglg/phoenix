//! Fixed-capacity physical-memory normalization and frame allocation.

use super::{AddressRange, PAGE_SIZE, PageFrame, PhysAddr};

/// Failure while constructing or updating a physical memory map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryMapError {
    /// Fixed metadata capacity cannot represent another disjoint range.
    CapacityExceeded,
    /// A frame count overflowed the representable frame-number space.
    FrameCountOverflow,
}

/// Failure while allocating or releasing physical frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationError {
    /// A zero count or non-power-of-two frame alignment was requested.
    InvalidRequest,
    /// Free-list metadata cannot represent the resulting fragmentation.
    MetadataExhausted,
    /// A released frame is outside the memory managed by this allocator.
    NotManaged,
    /// A released frame is already free.
    DoubleFree,
}

/// A half-open range of base-page frame numbers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRange {
    start: usize,
    end: usize,
}

impl FrameRange {
    const EMPTY: Self = Self { start: 0, end: 0 };

    /// Construct a range from its first frame and number of frames.
    pub fn new(start: PageFrame, frame_count: usize) -> Result<Self, MemoryMapError> {
        let start = start.number();
        let end = start
            .checked_add(frame_count)
            .ok_or(MemoryMapError::FrameCountOverflow)?;
        Ok(Self { start, end })
    }

    /// Convert usable bytes to the completely covered frames.
    pub fn from_usable_bytes(bytes: AddressRange<PhysAddr>) -> Self {
        let start = bytes.start().as_usize().div_ceil(PAGE_SIZE);
        let end = bytes.end().as_usize() / PAGE_SIZE;
        if start < end {
            Self { start, end }
        } else {
            Self::EMPTY
        }
    }

    /// Convert reserved bytes to every frame touched by the range.
    pub fn covering_bytes(bytes: AddressRange<PhysAddr>) -> Self {
        if bytes.is_empty() {
            return Self::EMPTY;
        }
        let start = bytes.start().as_usize() / PAGE_SIZE;
        let end = bytes.end().as_usize().div_ceil(PAGE_SIZE);
        Self { start, end }
    }

    /// Return the inclusive first frame number.
    pub const fn start_frame_number(self) -> usize {
        self.start
    }

    /// Return the exclusive final frame number.
    pub const fn end_frame_number(self) -> usize {
        self.end
    }

    /// Return the number of frames in the range.
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Return whether the range contains no frames.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Return whether a frame belongs to this range.
    pub const fn contains(self, frame: PageFrame) -> bool {
        self.start <= frame.number() && frame.number() < self.end
    }

    fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegionSet<const CAPACITY: usize> {
    ranges: [FrameRange; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> RegionSet<CAPACITY> {
    const fn new() -> Self {
        Self {
            ranges: [FrameRange::EMPTY; CAPACITY],
            len: 0,
        }
    }

    fn as_slice(&self) -> &[FrameRange] {
        &self.ranges[..self.len]
    }

    fn total_frames(&self) -> usize {
        self.as_slice().iter().map(|range| range.len()).sum()
    }

    fn contains(&self, frame: PageFrame) -> bool {
        self.as_slice().iter().any(|range| range.contains(frame))
    }

    fn insert(&mut self, range: FrameRange) -> Result<(), MemoryMapError> {
        if range.is_empty() {
            return Ok(());
        }

        let mut output = [FrameRange::EMPTY; CAPACITY];
        let mut output_len = 0;
        let mut inserted = false;
        for current in self.as_slice().iter().copied() {
            if !inserted && range.start < current.start {
                push_merged(&mut output, &mut output_len, range)?;
                inserted = true;
            }
            push_merged(&mut output, &mut output_len, current)?;
        }
        if !inserted {
            push_merged(&mut output, &mut output_len, range)?;
        }
        self.ranges = output;
        self.len = output_len;
        Ok(())
    }

    fn remove(&mut self, removed: FrameRange) -> Result<(), MemoryMapError> {
        if removed.is_empty() {
            return Ok(());
        }

        let mut output = [FrameRange::EMPTY; CAPACITY];
        let mut output_len = 0;
        for current in self.as_slice().iter().copied() {
            if !current.overlaps(removed) {
                push_unmerged(&mut output, &mut output_len, current)?;
                continue;
            }
            if current.start < removed.start {
                push_unmerged(
                    &mut output,
                    &mut output_len,
                    FrameRange {
                        start: current.start,
                        end: removed.start.min(current.end),
                    },
                )?;
            }
            if removed.end < current.end {
                push_unmerged(
                    &mut output,
                    &mut output_len,
                    FrameRange {
                        start: removed.end.max(current.start),
                        end: current.end,
                    },
                )?;
            }
        }
        self.ranges = output;
        self.len = output_len;
        Ok(())
    }

    fn take(
        &mut self,
        count: usize,
        alignment: usize,
    ) -> Result<Option<FrameRange>, AllocationError> {
        if count == 0 || !alignment.is_power_of_two() {
            return Err(AllocationError::InvalidRequest);
        }

        for available in self.as_slice().iter().copied() {
            let Some(start) = available.start.checked_add(alignment - 1) else {
                continue;
            };
            let start = start & !(alignment - 1);
            let Some(end) = start.checked_add(count) else {
                continue;
            };
            if end > available.end {
                continue;
            }
            let allocated = FrameRange { start, end };
            self.remove(allocated)
                .map_err(|_| AllocationError::MetadataExhausted)?;
            return Ok(Some(allocated));
        }
        Ok(None)
    }
}

fn push_merged<const CAPACITY: usize>(
    output: &mut [FrameRange; CAPACITY],
    len: &mut usize,
    range: FrameRange,
) -> Result<(), MemoryMapError> {
    if let Some(previous) = len.checked_sub(1).map(|index| &mut output[index])
        && range.start <= previous.end
    {
        previous.end = previous.end.max(range.end);
        return Ok(());
    }
    push_unmerged(output, len, range)
}

fn push_unmerged<const CAPACITY: usize>(
    output: &mut [FrameRange; CAPACITY],
    len: &mut usize,
    range: FrameRange,
) -> Result<(), MemoryMapError> {
    let slot = output
        .get_mut(*len)
        .ok_or(MemoryMapError::CapacityExceeded)?;
    *slot = range;
    *len += 1;
    Ok(())
}

/// A normalized, fixed-capacity map of allocatable physical frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryMap<const CAPACITY: usize> {
    free: RegionSet<CAPACITY>,
}

impl<const CAPACITY: usize> MemoryMap<CAPACITY> {
    /// Construct an empty physical memory map.
    pub const fn new() -> Self {
        Self {
            free: RegionSet::new(),
        }
    }

    /// Add usable bytes, retaining only complete base pages and merging neighbors.
    pub fn add_usable(&mut self, bytes: AddressRange<PhysAddr>) -> Result<(), MemoryMapError> {
        self.free.insert(FrameRange::from_usable_bytes(bytes))
    }

    /// Conservatively remove every frame touched by a reserved byte range.
    pub fn reserve(&mut self, bytes: AddressRange<PhysAddr>) -> Result<(), MemoryMapError> {
        self.free.remove(FrameRange::covering_bytes(bytes))
    }

    /// Return normalized free frame ranges in ascending order.
    pub fn free_ranges(&self) -> &[FrameRange] {
        self.free.as_slice()
    }

    /// Return the total number of currently free base pages.
    pub fn total_free_frames(&self) -> usize {
        self.free.total_frames()
    }

    /// Freeze this map as the ownership boundary of a frame allocator.
    pub const fn into_allocator(self) -> FrameAllocator<CAPACITY> {
        FrameAllocator {
            managed: self.free,
            free: self.free,
        }
    }
}

impl<const CAPACITY: usize> Default for MemoryMap<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// A fixed-capacity first-fit allocator for base-page physical frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameAllocator<const CAPACITY: usize> {
    managed: RegionSet<CAPACITY>,
    free: RegionSet<CAPACITY>,
}

impl<const CAPACITY: usize> FrameAllocator<CAPACITY> {
    /// Allocate one physical frame using deterministic first fit.
    pub fn allocate(&mut self) -> Result<Option<PageFrame>, AllocationError> {
        Ok(self
            .free
            .take(1, 1)?
            .map(|range| frame_from_number(range.start)))
    }

    /// Allocate a contiguous range with power-of-two alignment measured in frames.
    pub fn allocate_contiguous(
        &mut self,
        count: usize,
        alignment_in_frames: usize,
    ) -> Result<Option<FrameRange>, AllocationError> {
        self.free.take(count, alignment_in_frames)
    }

    /// Release a previously allocated frame.
    pub fn deallocate(&mut self, frame: PageFrame) -> Result<(), AllocationError> {
        if !self.managed.contains(frame) {
            return Err(AllocationError::NotManaged);
        }
        if self.free.contains(frame) {
            return Err(AllocationError::DoubleFree);
        }
        self.free
            .insert(FrameRange {
                start: frame.number(),
                end: frame.number() + 1,
            })
            .map_err(|_| AllocationError::MetadataExhausted)
    }

    /// Return currently free normalized frame ranges.
    pub fn free_ranges(&self) -> &[FrameRange] {
        self.free.as_slice()
    }

    /// Return the number of currently free frames.
    pub fn total_free_frames(&self) -> usize {
        self.free.total_frames()
    }
}

fn frame_from_number(number: usize) -> PageFrame {
    let address = number
        .checked_mul(PAGE_SIZE)
        .expect("managed frame numbers originate from physical addresses");
    PageFrame::from_start(PhysAddr::new(address)).expect("frame number is page aligned")
}

#[cfg(test)]
mod tests {
    use super::{AllocationError, FrameRange, MemoryMap, MemoryMapError};
    use crate::memory::{AddressRange, PAGE_SIZE, PageFrame, PhysAddr};

    fn bytes(start: usize, end: usize) -> AddressRange<PhysAddr> {
        AddressRange::new(PhysAddr::new(start), PhysAddr::new(end)).expect("ordered byte range")
    }

    fn frame(number: usize) -> PageFrame {
        PageFrame::from_start(PhysAddr::new(number * PAGE_SIZE)).expect("aligned frame")
    }

    #[test]
    fn usable_regions_round_inward_sort_and_merge() {
        let mut map = MemoryMap::<4>::new();
        map.add_usable(bytes(0x5001, 0x9001)).expect("first region");
        map.add_usable(bytes(0x2000, 0x6000))
            .expect("second region");

        assert_eq!(map.free_ranges(), &[FrameRange { start: 2, end: 9 }]);
        assert_eq!(map.total_free_frames(), 7);
    }

    #[test]
    fn reservations_round_outward_and_split_regions() {
        let mut map = MemoryMap::<4>::new();
        map.add_usable(bytes(0x1000, 0x9000))
            .expect("usable region");
        map.reserve(bytes(0x3001, 0x5fff)).expect("reservation");

        assert_eq!(
            map.free_ranges(),
            &[
                FrameRange { start: 1, end: 3 },
                FrameRange { start: 6, end: 9 },
            ]
        );
    }

    #[test]
    fn empty_reservation_changes_nothing() {
        let mut map = MemoryMap::<1>::new();
        map.add_usable(bytes(0x1000, 0x3000))
            .expect("usable region");
        let before = map;
        map.reserve(bytes(0x2000, 0x2000))
            .expect("empty reservation");

        assert_eq!(map, before);
    }

    #[test]
    fn metadata_exhaustion_is_transactional() {
        let mut map = MemoryMap::<1>::new();
        map.add_usable(bytes(0, 3 * PAGE_SIZE))
            .expect("usable region");
        let before = map;

        assert_eq!(
            map.reserve(bytes(PAGE_SIZE, 2 * PAGE_SIZE)),
            Err(MemoryMapError::CapacityExceeded)
        );
        assert_eq!(map, before);
    }

    #[test]
    fn disjoint_usable_ranges_obey_fixed_capacity() {
        let mut map = MemoryMap::<1>::new();
        map.add_usable(bytes(0, PAGE_SIZE)).expect("first region");

        assert_eq!(
            map.add_usable(bytes(2 * PAGE_SIZE, 3 * PAGE_SIZE)),
            Err(MemoryMapError::CapacityExceeded)
        );
        assert_eq!(map.free_ranges(), &[FrameRange { start: 0, end: 1 }]);
    }

    #[test]
    fn allocator_returns_frames_in_ascending_order() {
        let mut map = MemoryMap::<2>::new();
        map.add_usable(bytes(2 * PAGE_SIZE, 5 * PAGE_SIZE))
            .expect("usable region");
        let mut allocator = map.into_allocator();

        assert_eq!(allocator.allocate(), Ok(Some(frame(2))));
        assert_eq!(allocator.allocate(), Ok(Some(frame(3))));
        assert_eq!(allocator.allocate(), Ok(Some(frame(4))));
        assert_eq!(allocator.allocate(), Ok(None));
    }

    #[test]
    fn contiguous_allocation_honors_frame_alignment() {
        let mut map = MemoryMap::<3>::new();
        map.add_usable(bytes(PAGE_SIZE, 10 * PAGE_SIZE))
            .expect("usable region");
        let mut allocator = map.into_allocator();

        assert_eq!(
            allocator.allocate_contiguous(2, 4),
            Ok(Some(FrameRange { start: 4, end: 6 }))
        );
        assert_eq!(
            allocator.free_ranges(),
            &[
                FrameRange { start: 1, end: 4 },
                FrameRange { start: 6, end: 10 },
            ]
        );
    }

    #[test]
    fn allocator_rejects_invalid_requests() {
        let map = MemoryMap::<1>::new();
        let mut allocator = map.into_allocator();

        assert_eq!(
            allocator.allocate_contiguous(0, 1),
            Err(AllocationError::InvalidRequest)
        );
        assert_eq!(
            allocator.allocate_contiguous(1, 3),
            Err(AllocationError::InvalidRequest)
        );
    }

    #[test]
    fn deallocation_rejects_foreign_and_duplicate_frames() {
        let mut map = MemoryMap::<2>::new();
        map.add_usable(bytes(PAGE_SIZE, 3 * PAGE_SIZE))
            .expect("usable region");
        let mut allocator = map.into_allocator();

        assert_eq!(
            allocator.deallocate(frame(9)),
            Err(AllocationError::NotManaged)
        );
        assert_eq!(
            allocator.deallocate(frame(1)),
            Err(AllocationError::DoubleFree)
        );
    }

    #[test]
    fn releasing_allocated_frames_coalesces_free_ranges() {
        let mut map = MemoryMap::<3>::new();
        map.add_usable(bytes(0, 3 * PAGE_SIZE))
            .expect("usable region");
        let mut allocator = map.into_allocator();
        let first = allocator
            .allocate()
            .expect("allocation result")
            .expect("frame");
        let second = allocator
            .allocate()
            .expect("allocation result")
            .expect("frame");

        allocator.deallocate(first).expect("release first");
        allocator.deallocate(second).expect("release second");

        assert_eq!(allocator.free_ranges(), &[FrameRange { start: 0, end: 3 }]);
        assert_eq!(allocator.total_free_frames(), 3);
    }
}
