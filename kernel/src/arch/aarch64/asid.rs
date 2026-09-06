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

/// Monotonic allocator that never unsafely reuses an ASID.
#[derive(Debug, Eq, PartialEq)]
pub struct AsidAllocator {
    next: u16,
}

impl AsidAllocator {
    /// Create an allocator whose first result is ASID one.
    pub const fn new() -> Self {
        Self {
            next: MIN_PROCESS_ASID as u16,
        }
    }

    /// Assign the lowest never-before-issued nonzero ASID.
    ///
    /// Exhaustion leaves the allocator unchanged. IDs are deliberately not
    /// released until an epoch and TLB-invalidation protocol exists.
    pub fn allocate(&mut self) -> Result<AddressSpaceId, AsidError> {
        if self.next > MAX_PROCESS_ASID as u16 {
            return Err(AsidError::Exhausted);
        }
        let asid = AddressSpaceId::new(self.next)?;
        self.next += 1;
        Ok(asid)
    }

    /// Return the number of ASIDs that can still be issued without reuse.
    pub const fn remaining(&self) -> u16 {
        (MAX_PROCESS_ASID as u16 + 1).saturating_sub(self.next)
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
}
