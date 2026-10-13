//! Nonzero address-space identifiers for the initial AArch64 regime.

use core::num::NonZeroU8;

use crate::memory::PageFrame;

/// Number of ASID bits selected by the current bootstrap `TCR_EL1` value.
pub const ASID_BITS: u8 = 8;
/// Lowest process ASID; zero remains reserved for bootstrap translations.
pub const MIN_PROCESS_ASID: u8 = 1;
/// Highest process ASID in the current 8-bit regime.
pub const MAX_PROCESS_ASID: u8 = u8::MAX;
const TTBR_ASID_SHIFT: u32 = 48;
const TTBR_BADDR_MASK: u64 = 0x0000_ffff_ffff_f000;

/// Failure while validating or assigning an address-space identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsidError {
    /// ASID zero is reserved for the bootstrap regime.
    ReservedZero,
    /// The value exceeds the active 8-bit hardware field.
    OutOfRange {
        /// Rejected raw identifier.
        value: u16,
    },
    /// Every nonzero identifier has already been issued.
    Exhausted,
    /// Retirement was attempted for an identifier that is not owned.
    NotAllocated,
}

/// Failure while composing an AArch64 translation-table base value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ttbr0Error {
    /// The root frame cannot fit in the implemented 48-bit BADDR field.
    RootAddressTooWide {
        /// Rejected physical root address.
        address: usize,
    },
}

/// One nonzero AArch64 address-space identifier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AddressSpaceId(NonZeroU8);

impl AddressSpaceId {
    /// Validate a raw value against the current 8-bit nonzero policy.
    pub const fn new(raw: u16) -> Result<Self, AsidError> {
        if raw == 0 {
            return Err(AsidError::ReservedZero);
        }
        if raw > u8::MAX as u16 {
            return Err(AsidError::OutOfRange { value: raw });
        }
        let Some(raw) = NonZeroU8::new(raw as u8) else {
            return Err(AsidError::ReservedZero);
        };
        Ok(Self(raw))
    }

    /// Return the raw nonzero 8-bit value.
    pub const fn raw(self) -> u8 {
        self.0.get()
    }
}

/// Lowest-free allocator; reuse requires an explicit unsafe retirement proof.
#[derive(Debug, Eq, PartialEq)]
pub struct AsidAllocator {
    used: [u64; 4],
}

impl AsidAllocator {
    /// Create an allocator whose first result is ASID one.
    pub const fn new() -> Self {
        Self { used: [1, 0, 0, 0] }
    }

    /// Assign the lowest currently unowned nonzero ASID.
    ///
    /// Exhaustion leaves the allocator unchanged.
    pub fn allocate(&mut self) -> Result<AddressSpaceId, AsidError> {
        for index in 1..=255usize {
            let mask = 1u64 << (index % 64);
            if self.used[index / 64] & mask == 0 {
                self.used[index / 64] |= mask;
                return AddressSpaceId::new(index as u16);
            }
        }
        Err(AsidError::Exhausted)
    }

    /// Retire an identifier after its old translations can no longer be used.
    ///
    /// # Safety
    /// Caller uniquely owns this allocation. Its root is no longer current on
    /// any CPU, all translations have been invalidated with completion barriers
    /// (or the root was never installed), and no stale owner may activate it again.
    pub unsafe fn release_retired(&mut self, asid: AddressSpaceId) -> Result<(), AsidError> {
        let index = usize::from(asid.raw());
        let mask = 1u64 << (index % 64);
        if self.used[index / 64] & mask == 0 {
            return Err(AsidError::NotAllocated);
        }
        self.used[index / 64] &= !mask;
        Ok(())
    }

    /// Return the number of unowned nonzero ASIDs.
    pub const fn remaining(&self) -> u16 {
        256 - (self.used[0].count_ones()
            + self.used[1].count_ones()
            + self.used[2].count_ones()
            + self.used[3].count_ones()) as u16
    }
}

impl Default for AsidAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Compose the value written to `TTBR0_EL1` for one root and ASID.
pub fn ttbr0_value(root: PageFrame, asid: AddressSpaceId) -> Result<u64, Ttbr0Error> {
    let address = root.start_address().as_usize();
    let address_u64 = address as u64;
    if address_u64 & !TTBR_BADDR_MASK != 0 {
        return Err(Ttbr0Error::RootAddressTooWide { address });
    }
    Ok(u64::from(asid.raw()) << TTBR_ASID_SHIFT | address_u64)
}

#[cfg(test)]
mod tests {
    use super::{
        AddressSpaceId, AsidAllocator, AsidError, MAX_PROCESS_ASID, Ttbr0Error, ttbr0_value,
    };
    use crate::memory::{PageFrame, PhysAddr};

    #[test]
    fn rejects_reserved_and_out_of_range_identifiers() {
        assert_eq!(AddressSpaceId::new(0), Err(AsidError::ReservedZero));
        assert_eq!(AddressSpaceId::new(1).unwrap().raw(), 1);
        assert_eq!(
            AddressSpaceId::new(256),
            Err(AsidError::OutOfRange { value: 256 })
        );
    }

    #[test]
    fn allocator_is_monotonic_and_exhaustion_is_sticky() {
        let mut allocator = AsidAllocator::new();
        for expected in 1..=MAX_PROCESS_ASID {
            assert_eq!(allocator.allocate().unwrap().raw(), expected);
        }
        assert_eq!(allocator.remaining(), 0);
        assert_eq!(allocator.allocate(), Err(AsidError::Exhausted));
        assert_eq!(allocator.allocate(), Err(AsidError::Exhausted));
    }

    #[test]
    fn ttbr_value_keeps_root_and_asid_fields_disjoint() {
        let root = PageFrame::from_start(PhysAddr::new(0x1234_5000)).unwrap();
        let value = ttbr0_value(root, AddressSpaceId::new(0xab).unwrap()).unwrap();
        assert_eq!(value, 0x00ab_0000_1234_5000);

        let wide = PageFrame::from_start(PhysAddr::new(0x0001_0000_0000_0000)).unwrap();
        assert_eq!(
            ttbr0_value(wide, AddressSpaceId::new(1).unwrap()),
            Err(Ttbr0Error::RootAddressTooWide {
                address: 0x0001_0000_0000_0000,
            })
        );
    }

    #[test]
    fn retired_id_is_reusable_and_double_retirement_is_rejected() {
        let mut ids = AsidAllocator::new();
        let parent = ids.allocate().unwrap();
        for _ in 0..512 {
            let child = ids.allocate().unwrap();
            assert_eq!(child.raw(), 2);
            // SAFETY: Host-only identities were never installed in hardware.
            unsafe { ids.release_retired(child) }.unwrap();
            // SAFETY: No hardware root or new owner exists; exercise rejection.
            assert_eq!(
                unsafe { ids.release_retired(child) },
                Err(AsidError::NotAllocated)
            );
        }
        assert_eq!(parent.raw(), 1);
        assert_eq!(ids.remaining(), 254);
    }
}
