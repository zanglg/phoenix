//! Transactional physical memory-map construction from validated boot information.

use crate::dtb::{BootInfo, Error as DeviceTreeError, Reservation};

use super::{AddressRange, MemoryMap, MemoryMapError, PhysAddr};

/// Failure while deriving allocatable frames from firmware boot information.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootMemoryError {
    /// Re-reading a validated DTB memory entry unexpectedly failed.
    DeviceTree(DeviceTreeError),
    /// Fixed physical-range metadata cannot represent the normalized result.
    MemoryMap(MemoryMapError),
    /// The supplied loaded kernel range is empty.
    EmptyKernelImage,
    /// The DTB physical start plus its validated size overflowed.
    DeviceTreeRangeOverflow,
    /// A firmware reservation cannot fit the target physical-address width.
    ReservationOutOfRange {
        /// Firmware-provided reservation address.
        address: u64,
        /// Firmware-provided reservation size.
        size: u64,
    },
    /// All complete usable frames were removed by reservations.
    NoUsableFrames,
}

/// Build the first physical memory map without mutating external state.
///
/// Usable DTB memory ranges are added first. Firmware reservations, the full
/// loaded kernel image, and the DTB blob are then removed conservatively at
/// page granularity. A result is returned only when at least one frame remains.
pub fn memory_map_from_boot_info<const CAPACITY: usize>(
    info: BootInfo<'_>,
    kernel_image: AddressRange<PhysAddr>,
    device_tree_start: PhysAddr,
) -> Result<MemoryMap<CAPACITY>, BootMemoryError> {
    if kernel_image.is_empty() {
        return Err(BootMemoryError::EmptyKernelImage);
    }
    let device_tree_end = device_tree_start
        .checked_add(info.device_tree_size())
        .map_err(|_| BootMemoryError::DeviceTreeRangeOverflow)?;
    let device_tree = AddressRange::new(device_tree_start, device_tree_end)
        .map_err(|_| BootMemoryError::DeviceTreeRangeOverflow)?;
    let mut map = MemoryMap::new();

    for memory in info.memory_regions() {
        map.add_usable(memory.map_err(BootMemoryError::DeviceTree)?)
            .map_err(BootMemoryError::MemoryMap)?;
    }
    for reservation in info.reservations() {
        map.reserve(reservation_range(reservation)?)
            .map_err(BootMemoryError::MemoryMap)?;
    }
    for reservation in info.reserved_memory_regions() {
        map.reserve(reservation.map_err(BootMemoryError::DeviceTree)?)
            .map_err(BootMemoryError::MemoryMap)?;
    }
    map.reserve(kernel_image)
        .map_err(BootMemoryError::MemoryMap)?;
    map.reserve(device_tree)
        .map_err(BootMemoryError::MemoryMap)?;

    if map.total_free_frames() == 0 {
        return Err(BootMemoryError::NoUsableFrames);
    }
    Ok(map)
}

fn reservation_range(reservation: Reservation) -> Result<AddressRange<PhysAddr>, BootMemoryError> {
    let end = reservation.address.checked_add(reservation.size).ok_or(
        BootMemoryError::ReservationOutOfRange {
            address: reservation.address,
            size: reservation.size,
        },
    )?;
    let start = usize::try_from(reservation.address).map_err(|_| {
        BootMemoryError::ReservationOutOfRange {
            address: reservation.address,
            size: reservation.size,
        }
    })?;
    let end = usize::try_from(end).map_err(|_| BootMemoryError::ReservationOutOfRange {
        address: reservation.address,
        size: reservation.size,
    })?;
    AddressRange::new(PhysAddr::new(start), PhysAddr::new(end)).map_err(|_| {
        BootMemoryError::ReservationOutOfRange {
            address: reservation.address,
            size: reservation.size,
        }
    })
}
