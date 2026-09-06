//! Architecture-neutral memory addresses, pages, frames, and ranges.

use core::cmp::{max, min};

mod physical;

pub use physical::{AllocationError, FrameAllocator, FrameRange, MemoryMap, MemoryMapError};

/// Phoenix's base page size in bytes.
pub const PAGE_SIZE: usize = 4096;

const _: () = assert!(PAGE_SIZE.is_power_of_two());

/// Failure while constructing or manipulating an address value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressError {
    /// Addition or alignment exceeded the address space.
    Overflow,
    /// An alignment was zero or was not a power of two.
    InvalidAlignment {
        /// Rejected alignment in bytes.
        alignment: usize,
    },
    /// An address did not satisfy the required alignment.
    Misaligned {
        /// Rejected raw address.
        address: usize,
        /// Required alignment in bytes.
        alignment: usize,
    },
    /// A half-open range ended before it started.
    ReversedRange {
        /// Inclusive range start.
        start: usize,
        /// Exclusive range end.
        end: usize,
    },
}

mod sealed {
    pub trait Sealed {}
}

/// Common behavior of strongly typed kernel addresses.
pub trait Address: sealed::Sealed + Copy + Ord {
    /// Construct an address from its raw integer representation.
    fn from_usize(value: usize) -> Self;

    /// Return the raw integer representation.
    fn as_usize(self) -> usize;
}

macro_rules! address_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(usize);

        impl $name {
            /// Construct an address from its raw integer representation.
            pub const fn new(value: usize) -> Self {
                Self(value)
            }

            /// Return the raw integer representation.
            pub const fn as_usize(self) -> usize {
                self.0
            }

            /// Add a byte offset, returning an error on overflow.
            pub const fn checked_add(self, offset: usize) -> Result<Self, AddressError> {
                match self.0.checked_add(offset) {
                    Some(value) => Ok(Self(value)),
                    None => Err(AddressError::Overflow),
                }
            }

            /// Subtract a byte offset, returning an error on underflow.
            pub const fn checked_sub(self, offset: usize) -> Result<Self, AddressError> {
                match self.0.checked_sub(offset) {
                    Some(value) => Ok(Self(value)),
                    None => Err(AddressError::Overflow),
                }
            }

            /// Return whether this address has the requested alignment.
            pub fn is_aligned(self, alignment: usize) -> Result<bool, AddressError> {
                validate_alignment(alignment)?;
                Ok(self.0 & (alignment - 1) == 0)
            }

            /// Round down to the requested power-of-two alignment.
            pub fn align_down(self, alignment: usize) -> Result<Self, AddressError> {
                validate_alignment(alignment)?;
                Ok(Self(self.0 & !(alignment - 1)))
            }

            /// Round up to the requested power-of-two alignment.
            pub fn checked_align_up(
                self,
                alignment: usize,
            ) -> Result<Self, AddressError> {
                validate_alignment(alignment)?;
                let mask = alignment - 1;
                match self.0.checked_add(mask) {
                    Some(value) => Ok(Self(value & !mask)),
                    None => Err(AddressError::Overflow),
                }
            }
        }

        impl sealed::Sealed for $name {}

        impl Address for $name {
            fn from_usize(value: usize) -> Self {
                Self(value)
            }

            fn as_usize(self) -> usize {
                self.0
            }
        }
    };
}

address_type!(
    /// A physical address. It is not implicitly interchangeable with a virtual address.
    PhysAddr
);

address_type!(
    /// A virtual address. It is not implicitly interchangeable with a physical address.
    VirtAddr
);

const fn validate_alignment(alignment: usize) -> Result<(), AddressError> {
    if alignment.is_power_of_two() {
        Ok(())
    } else {
        Err(AddressError::InvalidAlignment { alignment })
    }
}

/// A half-open address range `[start, end)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressRange<A> {
    start: A,
    end: A,
}

impl<A: Address> AddressRange<A> {
    /// Construct a range, rejecting an end below the start.
    pub fn new(start: A, end: A) -> Result<Self, AddressError> {
        if end < start {
            return Err(AddressError::ReversedRange {
                start: start.as_usize(),
                end: end.as_usize(),
            });
        }
        Ok(Self { start, end })
    }

    /// Construct a range from its start and byte length.
    pub fn from_start_and_len(start: A, len: usize) -> Result<Self, AddressError> {
        let end = start
            .as_usize()
            .checked_add(len)
            .ok_or(AddressError::Overflow)?;
        Self::new(start, A::from_usize(end))
    }

    /// Return the inclusive start address.
    pub const fn start(self) -> A {
        self.start
    }

    /// Return the exclusive end address.
    pub const fn end(self) -> A {
        self.end
    }

    /// Return the range length in bytes.
    pub fn len(self) -> usize {
        self.end.as_usize() - self.start.as_usize()
    }

    /// Return whether the range contains no addresses.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Return whether the address belongs to the range.
    pub fn contains(self, address: A) -> bool {
        self.start <= address && address < self.end
    }

    /// Return whether this range fully contains another range.
    pub fn contains_range(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    /// Return whether this range and another non-empty range overlap.
    pub fn overlaps(self, other: Self) -> bool {
        !self.is_empty() && !other.is_empty() && self.start < other.end && other.start < self.end
    }

    /// Return the non-empty intersection of two ranges.
    pub fn intersection(self, other: Self) -> Option<Self> {
        let start = max(self.start, other.start);
        let end = min(self.end, other.end);
        (start < end).then_some(Self { start, end })
    }
}

/// One base-page-sized physical frame.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PageFrame {
    start: PhysAddr,
}

impl PageFrame {
    /// Construct a frame from an aligned physical start address.
    pub fn from_start(start: PhysAddr) -> Result<Self, AddressError> {
        if start.is_aligned(PAGE_SIZE)? {
            Ok(Self { start })
        } else {
            Err(AddressError::Misaligned {
                address: start.as_usize(),
                alignment: PAGE_SIZE,
            })
        }
    }

    /// Return the frame containing an arbitrary physical address.
    pub fn containing(address: PhysAddr) -> Self {
        Self {
            start: PhysAddr::new(address.as_usize() & !(PAGE_SIZE - 1)),
        }
    }

    /// Return the frame's aligned physical start address.
    pub const fn start_address(self) -> PhysAddr {
        self.start
    }

    /// Return the zero-based physical frame number.
    pub const fn number(self) -> usize {
        self.start.as_usize() / PAGE_SIZE
    }

    /// Return the following physical frame, or an error at the address-space limit.
    pub fn checked_next(self) -> Result<Self, AddressError> {
        Self::from_start(self.start.checked_add(PAGE_SIZE)?)
    }
}

/// One base-page-sized virtual page.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Page {
    start: VirtAddr,
}

impl Page {
    /// Construct a page from an aligned virtual start address.
    pub fn from_start(start: VirtAddr) -> Result<Self, AddressError> {
        if start.is_aligned(PAGE_SIZE)? {
            Ok(Self { start })
        } else {
            Err(AddressError::Misaligned {
                address: start.as_usize(),
                alignment: PAGE_SIZE,
            })
        }
    }

    /// Return the page containing an arbitrary virtual address.
    pub fn containing(address: VirtAddr) -> Self {
        Self {
            start: VirtAddr::new(address.as_usize() & !(PAGE_SIZE - 1)),
        }
    }

    /// Return the page's aligned virtual start address.
    pub const fn start_address(self) -> VirtAddr {
        self.start
    }

    /// Return the zero-based virtual page number.
    pub const fn number(self) -> usize {
        self.start.as_usize() / PAGE_SIZE
    }

    /// Return the following virtual page, or an error at the address-space limit.
    pub fn checked_next(self) -> Result<Self, AddressError> {
        Self::from_start(self.start.checked_add(PAGE_SIZE)?)
    }
}

#[cfg(test)]
mod tests {
    use super::{AddressError, AddressRange, PAGE_SIZE, Page, PageFrame, PhysAddr, VirtAddr};

    #[test]
    fn physical_and_virtual_addresses_are_checked_independently() {
        let physical = PhysAddr::new(0x1234);
        let virtual_address = VirtAddr::new(0x1234);

        assert_eq!(physical.as_usize(), virtual_address.as_usize());
        assert_eq!(physical.checked_add(4), Ok(PhysAddr::new(0x1238)));
        assert_eq!(virtual_address.checked_sub(4), Ok(VirtAddr::new(0x1230)));
    }

    #[test]
    fn address_arithmetic_rejects_overflow_and_underflow() {
        assert_eq!(
            PhysAddr::new(usize::MAX).checked_add(1),
            Err(AddressError::Overflow)
        );
        assert_eq!(VirtAddr::new(0).checked_sub(1), Err(AddressError::Overflow));
    }

    #[test]
    fn alignment_operations_cover_exact_and_rounded_addresses() {
        let address = PhysAddr::new(0x2345);

        assert_eq!(address.align_down(PAGE_SIZE), Ok(PhysAddr::new(0x2000)));
        assert_eq!(
            address.checked_align_up(PAGE_SIZE),
            Ok(PhysAddr::new(0x3000))
        );
        assert_eq!(
            PhysAddr::new(0x3000).checked_align_up(PAGE_SIZE),
            Ok(PhysAddr::new(0x3000))
        );
    }

    #[test]
    fn alignment_rejects_invalid_values_and_overflow() {
        assert_eq!(
            PhysAddr::new(1).align_down(0),
            Err(AddressError::InvalidAlignment { alignment: 0 })
        );
        assert_eq!(
            PhysAddr::new(1).align_down(3),
            Err(AddressError::InvalidAlignment { alignment: 3 })
        );
        assert_eq!(
            PhysAddr::new(usize::MAX).checked_align_up(PAGE_SIZE),
            Err(AddressError::Overflow)
        );
    }

    #[test]
    fn page_and_frame_construction_enforces_alignment() {
        assert_eq!(
            PageFrame::from_start(PhysAddr::new(0x1001)),
            Err(AddressError::Misaligned {
                address: 0x1001,
                alignment: PAGE_SIZE,
            })
        );
        assert_eq!(
            Page::from_start(VirtAddr::new(0x2000))
                .expect("aligned page")
                .number(),
            2
        );
    }

    #[test]
    fn containing_rounds_down_to_the_correct_page_or_frame() {
        assert_eq!(
            PageFrame::containing(PhysAddr::new(0x2fff)).start_address(),
            PhysAddr::new(0x2000)
        );
        assert_eq!(
            Page::containing(VirtAddr::new(0x3000)).start_address(),
            VirtAddr::new(0x3000)
        );
    }

    #[test]
    fn next_page_checks_the_address_space_limit() {
        let last = Page::from_start(VirtAddr::new(usize::MAX & !(PAGE_SIZE - 1)))
            .expect("last aligned page");

        assert_eq!(last.checked_next(), Err(AddressError::Overflow));
    }

    #[test]
    fn ranges_are_half_open() {
        let range =
            AddressRange::new(PhysAddr::new(0x1000), PhysAddr::new(0x2000)).expect("ordered range");

        assert!(range.contains(PhysAddr::new(0x1000)));
        assert!(range.contains(PhysAddr::new(0x1fff)));
        assert!(!range.contains(PhysAddr::new(0x2000)));
        assert_eq!(range.len(), PAGE_SIZE);
    }

    #[test]
    fn ranges_reject_reverse_order_and_length_overflow() {
        assert_eq!(
            AddressRange::new(VirtAddr::new(9), VirtAddr::new(8)),
            Err(AddressError::ReversedRange { start: 9, end: 8 })
        );
        assert_eq!(
            AddressRange::from_start_and_len(PhysAddr::new(usize::MAX), 1),
            Err(AddressError::Overflow)
        );
    }

    #[test]
    fn empty_ranges_are_valid_but_do_not_overlap() {
        let empty = AddressRange::new(PhysAddr::new(5), PhysAddr::new(5)).expect("empty range");
        let populated =
            AddressRange::new(PhysAddr::new(4), PhysAddr::new(6)).expect("populated range");

        assert!(empty.is_empty());
        assert!(populated.contains_range(empty));
        assert!(!empty.overlaps(populated));
        assert_eq!(empty.intersection(populated), None);
    }

    #[test]
    fn range_intersection_excludes_touching_edges() {
        let left = AddressRange::new(PhysAddr::new(0), PhysAddr::new(10)).expect("left range");
        let middle = AddressRange::new(PhysAddr::new(5), PhysAddr::new(15)).expect("middle range");
        let right = AddressRange::new(PhysAddr::new(10), PhysAddr::new(20)).expect("right range");

        assert_eq!(
            left.intersection(middle),
            Some(AddressRange::new(PhysAddr::new(5), PhysAddr::new(10)).expect("intersection"))
        );
        assert!(!left.overlaps(right));
        assert_eq!(left.intersection(right), None);
    }
}
