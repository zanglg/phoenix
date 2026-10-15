//! Architecture-neutral user virtual-address and mapping plans.

use crate::memory::PAGE_SIZE;

/// Number of virtual-address bits available to EL0 in the initial regime.
pub const USER_VIRTUAL_ADDRESS_BITS: u32 = 39;
/// First address outside the initial EL0 virtual-address region.
pub const USER_VIRTUAL_LIMIT: usize = 1_usize << USER_VIRTUAL_ADDRESS_BITS;
/// Lowest address Phoenix permits a user mapping to occupy.
pub const USER_MIN_MAPPABLE_ADDRESS: usize = PAGE_SIZE;
/// Initial top of the first user stack, leaving the highest user page unmapped.
pub const DEFAULT_USER_STACK_TOP: usize = USER_VIRTUAL_LIMIT - PAGE_SIZE;

/// Failure while validating a user address or mapping plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserAddressError {
    /// An address is outside the initial lower 39-bit region.
    OutsideUserRegion {
        /// Rejected address.
        address: usize,
    },
    /// Address arithmetic overflowed.
    AddressOverflow,
    /// A required address is not aligned to 4 KiB.
    Misaligned {
        /// Rejected address.
        address: usize,
    },
    /// A range is empty or reversed.
    EmptyOrReversedRange,
    /// A mapping would include the deliberately unmapped null page.
    NullPageMapping,
    /// A mapping was requested without read, write, or execute access.
    NoPermissions,
    /// A user mapping was both writable and executable.
    WritableExecutable,
    /// A mapping overlaps an existing plan entry.
    Overlap,
    /// A fixed-capacity plan has no remaining metadata entry.
    CapacityExceeded,
    /// A stack has no usable or guard page.
    InvalidStackPageCount,
}

/// Valid lower-half user virtual address.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct UserAddr(usize);

impl UserAddr {
    /// Validate a user virtual address.
    pub const fn new(address: usize) -> Result<Self, UserAddressError> {
        if address < USER_VIRTUAL_LIMIT {
            Ok(Self(address))
        } else {
            Err(UserAddressError::OutsideUserRegion { address })
        }
    }

    /// Return the raw address.
    pub const fn as_usize(self) -> usize {
        self.0
    }

    /// Add an offset without wrapping or leaving the user region.
    pub const fn checked_add(self, offset: usize) -> Result<Self, UserAddressError> {
        let Some(address) = self.0.checked_add(offset) else {
            return Err(UserAddressError::AddressOverflow);
        };
        Self::new(address)
    }

    /// Return whether the address is aligned to the base page size.
    pub const fn is_page_aligned(self) -> bool {
        self.0.is_multiple_of(PAGE_SIZE)
    }
}

/// Non-empty half-open user virtual-address range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserRange {
    start: UserAddr,
    end_exclusive: usize,
}

impl UserRange {
    /// Validate `[start, end_exclusive)` within the user region.
    ///
    /// The exclusive end may equal `USER_VIRTUAL_LIMIT`, even though that
    /// sentinel is not itself a dereferenceable `UserAddr`.
    pub const fn new(start: UserAddr, end_exclusive: usize) -> Result<Self, UserAddressError> {
        if end_exclusive > USER_VIRTUAL_LIMIT {
            return Err(UserAddressError::OutsideUserRegion {
                address: end_exclusive,
            });
        }
        if start.as_usize() >= end_exclusive {
            return Err(UserAddressError::EmptyOrReversedRange);
        }
        Ok(Self {
            start,
            end_exclusive,
        })
    }

    /// Construct a range from a checked start and byte length.
    pub const fn from_start_len(start: UserAddr, length: usize) -> Result<Self, UserAddressError> {
        let Some(end) = start.as_usize().checked_add(length) else {
            return Err(UserAddressError::AddressOverflow);
        };
        Self::new(start, end)
    }

    /// Return the first address in the range.
    pub const fn start(self) -> UserAddr {
        self.start
    }

    /// Return the exclusive raw end, which may equal the user-region limit.
    pub const fn end_exclusive(self) -> usize {
        self.end_exclusive
    }

    /// Return the range length in bytes.
    pub const fn byte_len(self) -> usize {
        self.end_exclusive - self.start.as_usize()
    }

    /// Return whether an address lies within the range.
    pub const fn contains(self, address: UserAddr) -> bool {
        self.start.as_usize() <= address.as_usize() && address.as_usize() < self.end_exclusive
    }

    /// Return whether both bounds are base-page aligned.
    pub const fn is_page_aligned(self) -> bool {
        self.start.is_page_aligned() && self.end_exclusive.is_multiple_of(PAGE_SIZE)
    }

    /// Return whether this range intersects another half-open range.
    pub const fn overlaps(self, other: Self) -> bool {
        self.start.as_usize() < other.end_exclusive && other.start.as_usize() < self.end_exclusive
    }
}

/// Validated user mapping permissions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserPermissions {
    read: bool,
    write: bool,
    execute: bool,
}

impl UserPermissions {
    /// Validate an explicit permission combination.
    pub const fn new(read: bool, write: bool, execute: bool) -> Result<Self, UserAddressError> {
        if !read && !write && !execute {
            return Err(UserAddressError::NoPermissions);
        }
        if write && execute {
            return Err(UserAddressError::WritableExecutable);
        }
        Ok(Self {
            read,
            write,
            execute,
        })
    }

    /// Read-only executable program text.
    pub const fn read_execute() -> Self {
        Self {
            read: true,
            write: false,
            execute: true,
        }
    }

    /// Read-only user data.
    pub const fn read_only() -> Self {
        Self {
            read: true,
            write: false,
            execute: false,
        }
    }

    /// Read/write, non-executable user data.
    pub const fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            execute: false,
        }
    }

    /// Return whether EL0 may read the mapping.
    pub const fn readable(self) -> bool {
        self.read
    }

    /// Return whether EL0 may write the mapping.
    pub const fn writable(self) -> bool {
        self.write
    }

    /// Return whether EL0 may execute the mapping.
    pub const fn executable(self) -> bool {
        self.execute
    }
}

/// Purpose of a planned user mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserMappingKind {
    /// Loadable executable segment.
    Program,
    /// Initial user stack.
    Stack,
    /// Explicit mapping whose higher-level owner is not encoded here.
    Other,
}

/// One validated, page-aligned user mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserMapping {
    range: UserRange,
    permissions: UserPermissions,
    kind: UserMappingKind,
}

impl UserMapping {
    const EMPTY: Self = Self {
        range: UserRange {
            start: UserAddr(1),
            end_exclusive: 2,
        },
        permissions: UserPermissions::read_only(),
        kind: UserMappingKind::Other,
    };

    /// Return the mapped virtual range.
    pub const fn range(self) -> UserRange {
        self.range
    }

    /// Return the user permissions.
    pub const fn permissions(self) -> UserPermissions {
        self.permissions
    }

    /// Return the mapping purpose.
    pub const fn kind(self) -> UserMappingKind {
        self.kind
    }
}

/// Fixed-capacity, allocation-free user mapping plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserAddressSpacePlan<const CAPACITY: usize> {
    mappings: [UserMapping; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> UserAddressSpacePlan<CAPACITY> {
    /// Construct an empty user address-space plan.
    pub const fn new() -> Self {
        Self {
            mappings: [UserMapping::EMPTY; CAPACITY],
            len: 0,
        }
    }

    /// Add one page-aligned mapping while preserving address order.
    pub fn map(
        &mut self,
        range: UserRange,
        permissions: UserPermissions,
        kind: UserMappingKind,
    ) -> Result<(), UserAddressError> {
        if !range.is_page_aligned() {
            return Err(UserAddressError::Misaligned {
                address: if !range.start().is_page_aligned() {
                    range.start().as_usize()
                } else {
                    range.end_exclusive()
                },
            });
        }
        if range.start().as_usize() < USER_MIN_MAPPABLE_ADDRESS {
            return Err(UserAddressError::NullPageMapping);
        }

        let insertion = self.mappings[..self.len]
            .partition_point(|mapping| mapping.range.start() < range.start());
        if insertion > 0 && self.mappings[insertion - 1].range.overlaps(range)
            || insertion < self.len && self.mappings[insertion].range.overlaps(range)
        {
            return Err(UserAddressError::Overlap);
        }
        if self.len == CAPACITY {
            return Err(UserAddressError::CapacityExceeded);
        }

        self.mappings
            .copy_within(insertion..self.len, insertion + 1);
        self.mappings[insertion] = UserMapping {
            range,
            permissions,
            kind,
        };
        self.len += 1;
        Ok(())
    }

    /// Return mappings sorted by virtual address.
    pub fn mappings(&self) -> &[UserMapping] {
        &self.mappings[..self.len]
    }

    /// Find the mapping containing a user address.
    pub fn mapping_for(&self, address: UserAddr) -> Option<UserMapping> {
        self.mappings()
            .iter()
            .copied()
            .find(|mapping| mapping.range.contains(address))
    }
}

impl<const CAPACITY: usize> Default for UserAddressSpacePlan<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Guarded initial stack placement without allocating its physical pages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserStackLayout {
    guard: UserRange,
    usable: UserRange,
    initial_stack_pointer: UserAddr,
}

impl UserStackLayout {
    /// Place a guarded stack immediately below an aligned top address.
    pub const fn new(
        top: UserAddr,
        usable_pages: usize,
        guard_pages: usize,
    ) -> Result<Self, UserAddressError> {
        if usable_pages == 0 || guard_pages == 0 {
            return Err(UserAddressError::InvalidStackPageCount);
        }
        if !top.is_page_aligned() {
            return Err(UserAddressError::Misaligned {
                address: top.as_usize(),
            });
        }
        let Some(usable_size) = usable_pages.checked_mul(PAGE_SIZE) else {
            return Err(UserAddressError::AddressOverflow);
        };
        let Some(guard_size) = guard_pages.checked_mul(PAGE_SIZE) else {
            return Err(UserAddressError::AddressOverflow);
        };
        let Some(usable_start_raw) = top.as_usize().checked_sub(usable_size) else {
            return Err(UserAddressError::AddressOverflow);
        };
        let Some(guard_start_raw) = usable_start_raw.checked_sub(guard_size) else {
            return Err(UserAddressError::AddressOverflow);
        };
        if guard_start_raw < USER_MIN_MAPPABLE_ADDRESS {
            return Err(UserAddressError::NullPageMapping);
        }
        let Ok(usable_start) = UserAddr::new(usable_start_raw) else {
            return Err(UserAddressError::OutsideUserRegion {
                address: usable_start_raw,
            });
        };
        let Ok(guard_start) = UserAddr::new(guard_start_raw) else {
            return Err(UserAddressError::OutsideUserRegion {
                address: guard_start_raw,
            });
        };
        let Ok(guard) = UserRange::new(guard_start, usable_start_raw) else {
            return Err(UserAddressError::EmptyOrReversedRange);
        };
        let Ok(usable) = UserRange::new(usable_start, top.as_usize()) else {
            return Err(UserAddressError::EmptyOrReversedRange);
        };
        Ok(Self {
            guard,
            usable,
            initial_stack_pointer: top,
        })
    }

    /// Return the unmapped guard range below the stack.
    pub const fn guard_range(self) -> UserRange {
        self.guard
    }

    /// Return the mapped stack range.
    pub const fn usable_range(self) -> UserRange {
        self.usable
    }

    /// Return the initial, 16-byte-aligned stack pointer.
    pub const fn initial_stack_pointer(self) -> UserAddr {
        self.initial_stack_pointer
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_USER_STACK_TOP, USER_MIN_MAPPABLE_ADDRESS, USER_VIRTUAL_LIMIT, UserAddr,
        UserAddressError, UserAddressSpacePlan, UserMappingKind, UserPermissions, UserRange,
        UserStackLayout,
    };
    use crate::memory::PAGE_SIZE;

    #[test]
    fn user_addresses_reject_the_canonical_gap_and_overflow() {
        let highest = UserAddr::new(USER_VIRTUAL_LIMIT - 1).unwrap();
        assert_eq!(highest.as_usize(), USER_VIRTUAL_LIMIT - 1);
        assert_eq!(
            UserAddr::new(USER_VIRTUAL_LIMIT),
            Err(UserAddressError::OutsideUserRegion {
                address: USER_VIRTUAL_LIMIT
            })
        );
        assert_eq!(
            highest.checked_add(1),
            Err(UserAddressError::OutsideUserRegion {
                address: USER_VIRTUAL_LIMIT
            })
        );
    }

    #[test]
    fn range_may_end_at_the_user_limit_but_is_never_empty() {
        let range = UserRange::new(
            UserAddr::new(USER_VIRTUAL_LIMIT - PAGE_SIZE).unwrap(),
            USER_VIRTUAL_LIMIT,
        )
        .unwrap();
        assert_eq!(range.byte_len(), PAGE_SIZE);
        assert!(range.contains(UserAddr::new(USER_VIRTUAL_LIMIT - 1).unwrap()));
        assert_eq!(
            UserRange::new(UserAddr::new(PAGE_SIZE).unwrap(), PAGE_SIZE),
            Err(UserAddressError::EmptyOrReversedRange)
        );
    }

    #[test]
    fn permissions_enforce_w_xor_x_and_nonempty_access() {
        assert_eq!(
            UserPermissions::new(true, true, true),
            Err(UserAddressError::WritableExecutable)
        );
        assert_eq!(
            UserPermissions::new(false, false, false),
            Err(UserAddressError::NoPermissions)
        );
        assert!(UserPermissions::read_execute().executable());
        assert!(UserPermissions::read_write().writable());
    }

    #[test]
    fn plan_sorts_finds_and_rejects_overlapping_mappings() {
        let text = UserRange::new(UserAddr::new(0x40_0000).unwrap(), 0x41_0000).unwrap();
        let data = UserRange::new(UserAddr::new(0x50_0000).unwrap(), 0x51_0000).unwrap();
        let overlap = UserRange::new(UserAddr::new(0x40_f000).unwrap(), 0x42_0000).unwrap();
        let mut plan = UserAddressSpacePlan::<2>::new();
        plan.map(
            data,
            UserPermissions::read_write(),
            UserMappingKind::Program,
        )
        .unwrap();
        plan.map(
            text,
            UserPermissions::read_execute(),
            UserMappingKind::Program,
        )
        .unwrap();

        assert_eq!(plan.mappings()[0].range(), text);
        assert_eq!(
            plan.mapping_for(UserAddr::new(0x40_1234).unwrap())
                .unwrap()
                .permissions(),
            UserPermissions::read_execute()
        );
        assert_eq!(
            plan.map(
                overlap,
                UserPermissions::read_only(),
                UserMappingKind::Other
            ),
            Err(UserAddressError::Overlap)
        );
    }

    #[test]
    fn plan_rejects_null_unaligned_and_capacity_exhaustion() {
        let mut plan = UserAddressSpacePlan::<1>::new();
        let null = UserRange::new(UserAddr::new(0).unwrap(), PAGE_SIZE).unwrap();
        assert_eq!(
            plan.map(null, UserPermissions::read_only(), UserMappingKind::Other),
            Err(UserAddressError::NullPageMapping)
        );

        let unaligned =
            UserRange::new(UserAddr::new(PAGE_SIZE + 1).unwrap(), 2 * PAGE_SIZE).unwrap();
        assert!(matches!(
            plan.map(
                unaligned,
                UserPermissions::read_only(),
                UserMappingKind::Other
            ),
            Err(UserAddressError::Misaligned { .. })
        ));

        let first = UserRange::new(
            UserAddr::new(USER_MIN_MAPPABLE_ADDRESS).unwrap(),
            2 * PAGE_SIZE,
        )
        .unwrap();
        plan.map(first, UserPermissions::read_only(), UserMappingKind::Other)
            .unwrap();
        let second = UserRange::new(UserAddr::new(3 * PAGE_SIZE).unwrap(), 4 * PAGE_SIZE).unwrap();
        assert_eq!(
            plan.map(second, UserPermissions::read_only(), UserMappingKind::Other),
            Err(UserAddressError::CapacityExceeded)
        );
    }

    #[test]
    fn guarded_stack_layout_is_aligned_and_non_overlapping() {
        let top = UserAddr::new(DEFAULT_USER_STACK_TOP).unwrap();
        let stack = UserStackLayout::new(top, 16, 1).unwrap();

        assert_eq!(stack.initial_stack_pointer(), top);
        assert_eq!(stack.usable_range().byte_len(), 16 * PAGE_SIZE);
        assert_eq!(stack.guard_range().byte_len(), PAGE_SIZE);
        assert_eq!(
            stack.guard_range().end_exclusive(),
            stack.usable_range().start().as_usize()
        );
        assert!(!stack.guard_range().overlaps(stack.usable_range()));
    }

    #[test]
    fn guarded_stack_rejects_bad_counts_alignment_and_underflow() {
        assert_eq!(
            UserStackLayout::new(UserAddr::new(DEFAULT_USER_STACK_TOP).unwrap(), 0, 1),
            Err(UserAddressError::InvalidStackPageCount)
        );
        assert!(matches!(
            UserStackLayout::new(UserAddr::new(DEFAULT_USER_STACK_TOP - 1).unwrap(), 1, 1),
            Err(UserAddressError::Misaligned { .. })
        ));
        assert_eq!(
            UserStackLayout::new(UserAddr::new(2 * PAGE_SIZE).unwrap(), 1, 1),
            Err(UserAddressError::NullPageMapping)
        );
    }
}
