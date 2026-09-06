//! Boot-oriented information extracted from a validated device tree.

use core::str;

use crate::memory::{AddressRange, PhysAddr};

use super::{DeviceTree, Error, Event, Events, Reservations};

const DEFAULT_ADDRESS_CELLS: u32 = 2;
const DEFAULT_SIZE_CELLS: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CellCounts {
    address: u32,
    size: u32,
}

/// Allocation-free boot information extracted from a device tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootInfo<'a> {
    tree: DeviceTree<'a>,
    cells: CellCounts,
    bootargs: Option<&'a str>,
}

impl<'a> BootInfo<'a> {
    pub(super) fn from_tree(tree: DeviceTree<'a>) -> Result<Self, Error> {
        let cells = root_cell_counts(tree)?;
        let bootargs = chosen_bootargs(tree)?;
        let info = Self {
            tree,
            cells,
            bootargs,
        };

        let mut found_memory = false;
        for region in info.memory_regions() {
            if !region?.is_empty() {
                found_memory = true;
            }
        }
        if !found_memory {
            return Err(Error::MissingMemory);
        }
        Ok(info)
    }

    /// Return the physical CPU ID selected by firmware for boot.
    pub const fn boot_cpu_id(self) -> u32 {
        self.tree.boot_cpu_id()
    }

    /// Return `/chosen/bootargs` without its terminating zero byte, when present.
    pub const fn bootargs(self) -> Option<&'a str> {
        self.bootargs
    }

    /// Return the validated byte length of the complete source DTB blob.
    pub const fn device_tree_size(self) -> usize {
        self.tree.total_size()
    }

    /// Iterate over usable physical ranges declared by root memory nodes.
    pub fn memory_regions(self) -> MemoryRegions<'a> {
        MemoryRegions::new(self.tree, self.cells)
    }

    /// Iterate over physical reservations declared in the DTB reservation map.
    pub fn reservations(self) -> Reservations<'a> {
        self.tree.reservations()
    }
}

/// Iterator over physical ranges in root-level `memory` nodes.
#[derive(Clone, Debug)]
pub struct MemoryRegions<'a> {
    events: Events<'a>,
    cells: CellCounts,
    depth: usize,
    memory_depth: Option<usize>,
    pending: Option<RegEntries<'a>>,
}

impl<'a> MemoryRegions<'a> {
    fn new(tree: DeviceTree<'a>, cells: CellCounts) -> Self {
        Self {
            events: tree.events(),
            cells,
            depth: 0,
            memory_depth: None,
            pending: None,
        }
    }
}

impl<'a> Iterator for MemoryRegions<'a> {
    type Item = Result<AddressRange<PhysAddr>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(entries) = &mut self.pending {
                if let Some(region) = entries.next() {
                    return Some(region);
                }
                self.pending = None;
            }

            let event = match self.events.next()? {
                Ok(event) => event,
                Err(error) => return Some(Err(error)),
            };
            match event {
                Event::BeginNode { name } => {
                    self.depth += 1;
                    if self.depth == 2 && base_name(name) == "memory" {
                        self.memory_depth = Some(self.depth);
                    }
                }
                Event::EndNode => {
                    if self.memory_depth == Some(self.depth) {
                        self.memory_depth = None;
                    }
                    self.depth -= 1;
                }
                Event::Property { name: "reg", value } if self.memory_depth == Some(self.depth) => {
                    match RegEntries::new(value, self.cells) {
                        Ok(entries) => self.pending = Some(entries),
                        Err(error) => return Some(Err(error)),
                    }
                }
                Event::Property { .. } | Event::Nop | Event::End => {}
            }
        }
    }
}

#[derive(Clone, Debug)]
struct RegEntries<'a> {
    value: &'a [u8],
    cells: CellCounts,
    entry_size: usize,
    offset: usize,
}

impl<'a> RegEntries<'a> {
    fn new(value: &'a [u8], cells: CellCounts) -> Result<Self, Error> {
        let entry_size = (cells.address as usize + cells.size as usize) * 4;
        if !value.len().is_multiple_of(entry_size) {
            return Err(Error::MalformedReg {
                length: value.len(),
                entry_size,
            });
        }
        Ok(Self {
            value,
            cells,
            entry_size,
            offset: 0,
        })
    }
}

impl Iterator for RegEntries<'_> {
    type Item = Result<AddressRange<PhysAddr>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.value.get(self.offset..self.offset + self.entry_size)?;
        self.offset += self.entry_size;

        let address_bytes = self.cells.address as usize * 4;
        let address = read_cells(&entry[..address_bytes], self.cells.address);
        let size = read_cells(&entry[address_bytes..], self.cells.size);
        let end = match address.checked_add(size) {
            Some(end) => end,
            None => return Some(Err(Error::InvalidPhysicalRange { address, size })),
        };
        let (Ok(start), Ok(end)) = (usize::try_from(address), usize::try_from(end)) else {
            return Some(Err(Error::InvalidPhysicalRange { address, size }));
        };

        Some(
            AddressRange::new(PhysAddr::new(start), PhysAddr::new(end))
                .map_err(|_| Error::InvalidPhysicalRange { address, size }),
        )
    }
}

fn root_cell_counts(tree: DeviceTree<'_>) -> Result<CellCounts, Error> {
    let mut cells = CellCounts {
        address: DEFAULT_ADDRESS_CELLS,
        size: DEFAULT_SIZE_CELLS,
    };
    let mut depth = 0_usize;

    for event in tree.events() {
        match event? {
            Event::BeginNode { .. } => {
                depth += 1;
                if depth > 1 {
                    break;
                }
            }
            Event::Property {
                name: "#address-cells",
                value,
            } if depth == 1 => {
                cells.address = scalar_u32("#address-cells", value)?;
            }
            Event::Property {
                name: "#size-cells",
                value,
            } if depth == 1 => {
                cells.size = scalar_u32("#size-cells", value)?;
            }
            Event::EndNode | Event::Property { .. } | Event::Nop | Event::End => {}
        }
    }

    validate_cell_count("#address-cells", cells.address)?;
    validate_cell_count("#size-cells", cells.size)?;
    Ok(cells)
}

fn chosen_bootargs<'a>(tree: DeviceTree<'a>) -> Result<Option<&'a str>, Error> {
    let mut depth = 0_usize;
    let mut chosen_depth = None;

    for event in tree.events() {
        match event? {
            Event::BeginNode { name } => {
                depth += 1;
                if depth == 2 && base_name(name) == "chosen" {
                    chosen_depth = Some(depth);
                }
            }
            Event::EndNode => {
                if chosen_depth == Some(depth) {
                    chosen_depth = None;
                }
                depth -= 1;
            }
            Event::Property {
                name: "bootargs",
                value,
            } if chosen_depth == Some(depth) => {
                return parse_string("bootargs", value).map(Some);
            }
            Event::Property { .. } | Event::Nop | Event::End => {}
        }
    }
    Ok(None)
}

fn scalar_u32(name: &'static str, value: &[u8]) -> Result<u32, Error> {
    let bytes: [u8; 4] = value
        .try_into()
        .map_err(|_| Error::MalformedProperty { name })?;
    Ok(u32::from_be_bytes(bytes))
}

fn validate_cell_count(name: &'static str, count: u32) -> Result<(), Error> {
    if matches!(count, 1 | 2) {
        Ok(())
    } else {
        Err(Error::UnsupportedCellCount { name, count })
    }
}

fn read_cells(value: &[u8], count: u32) -> u64 {
    value
        .chunks_exact(4)
        .take(count as usize)
        .fold(0, |result, cell| {
            let bytes: [u8; 4] = cell.try_into().expect("cell has four bytes");
            (result << 32) | u64::from(u32::from_be_bytes(bytes))
        })
}

fn parse_string<'a>(name: &'static str, value: &'a [u8]) -> Result<&'a str, Error> {
    let Some((terminator, text)) = value.split_last() else {
        return Err(Error::MalformedProperty { name });
    };
    if *terminator != 0 || text.contains(&0) {
        return Err(Error::MalformedProperty { name });
    }
    str::from_utf8(text).map_err(|_| Error::InvalidUtf8)
}

fn base_name(name: &str) -> &str {
    name.split_once('@').map_or(name, |(base, _)| base)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use crate::dtb::{
        DeviceTree, Error, FDT_BEGIN_NODE, FDT_END, FDT_END_NODE, FDT_MAGIC, FDT_PROP,
    };
    use crate::memory::{
        AddressRange, BootMemoryError, MemoryMapError, PhysAddr, memory_map_from_boot_info,
    };

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn pad_4(bytes: &mut Vec<u8>) {
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
    }

    fn begin_node(structure: &mut Vec<u8>, name: &str) {
        push_u32(structure, FDT_BEGIN_NODE);
        structure.extend_from_slice(name.as_bytes());
        structure.push(0);
        pad_4(structure);
    }

    fn property(structure: &mut Vec<u8>, name_offset: u32, value: &[u8]) {
        push_u32(structure, FDT_PROP);
        push_u32(structure, value.len() as u32);
        push_u32(structure, name_offset);
        structure.extend_from_slice(value);
        pad_4(structure);
    }

    fn boot_fixture(address_cells: u32, size_cells: u32, reg: &[u8]) -> Vec<u8> {
        const ADDRESS_CELLS: u32 = 0;
        const SIZE_CELLS: u32 = 15;
        const DEVICE_TYPE: u32 = 27;
        const REG: u32 = 39;
        const BOOTARGS: u32 = 43;
        let strings = b"#address-cells\0#size-cells\0device_type\0reg\0bootargs\0";

        let mut structure = Vec::new();
        begin_node(&mut structure, "");
        property(&mut structure, ADDRESS_CELLS, &address_cells.to_be_bytes());
        property(&mut structure, SIZE_CELLS, &size_cells.to_be_bytes());
        begin_node(&mut structure, "memory@40000000");
        property(&mut structure, DEVICE_TYPE, b"memory\0");
        property(&mut structure, REG, reg);
        push_u32(&mut structure, FDT_END_NODE);
        begin_node(&mut structure, "chosen");
        property(&mut structure, BOOTARGS, b"console=ttyAMA0\0");
        push_u32(&mut structure, FDT_END_NODE);
        push_u32(&mut structure, FDT_END_NODE);
        push_u32(&mut structure, FDT_END);

        let mut reservations = Vec::new();
        push_u64(&mut reservations, 0x4800_0000);
        push_u64(&mut reservations, 0x1000);
        push_u64(&mut reservations, 0);
        push_u64(&mut reservations, 0);

        let reservations_offset = 40_usize;
        let structure_offset = reservations_offset + reservations.len();
        let strings_offset = structure_offset + structure.len();
        let total_size = strings_offset + strings.len();

        let mut blob = Vec::new();
        for field in [
            FDT_MAGIC,
            total_size as u32,
            structure_offset as u32,
            strings_offset as u32,
            reservations_offset as u32,
            17,
            16,
            3,
            strings.len() as u32,
            structure.len() as u32,
        ] {
            push_u32(&mut blob, field);
        }
        blob.extend_from_slice(&reservations);
        blob.extend_from_slice(&structure);
        blob.extend_from_slice(strings);
        blob
    }

    fn two_cell_reg(entries: &[(u64, u64)]) -> Vec<u8> {
        let mut value = Vec::new();
        for (address, size) in entries {
            push_u32(&mut value, (address >> 32) as u32);
            push_u32(&mut value, *address as u32);
            push_u32(&mut value, (size >> 32) as u32);
            push_u32(&mut value, *size as u32);
        }
        value
    }

    #[test]
    fn extracts_boot_arguments_memory_and_reservations() {
        let reg = two_cell_reg(&[(0x4000_0000, 0x2000_0000), (0x8000_0000, 0x1000_0000)]);
        let blob = boot_fixture(2, 2, &reg);
        let info = DeviceTree::from_bytes(&blob)
            .expect("valid tree")
            .boot_info()
            .expect("valid boot information");

        assert_eq!(info.boot_cpu_id(), 3);
        assert_eq!(info.bootargs(), Some("console=ttyAMA0"));
        assert_eq!(info.device_tree_size(), blob.len());
        assert_eq!(
            info.memory_regions().collect::<Result<Vec<_>, _>>(),
            Ok(vec![
                AddressRange::new(PhysAddr::new(0x4000_0000), PhysAddr::new(0x6000_0000))
                    .expect("first region"),
                AddressRange::new(PhysAddr::new(0x8000_0000), PhysAddr::new(0x9000_0000))
                    .expect("second region"),
            ])
        );
        assert_eq!(info.reservations().count(), 1);
    }

    #[test]
    fn boot_memory_map_reserves_firmware_kernel_and_device_tree() {
        let reg = two_cell_reg(&[(0x4000_0000, 0x1000_0000)]);
        let blob = boot_fixture(2, 2, &reg);
        let info = DeviceTree::from_bytes(&blob).unwrap().boot_info().unwrap();
        let kernel =
            AddressRange::new(PhysAddr::new(0x4008_0000), PhysAddr::new(0x4009_0000)).unwrap();
        let map = memory_map_from_boot_info::<4>(info, kernel, PhysAddr::new(0x4010_0000))
            .expect("four normalized free ranges");
        let ranges: Vec<_> = map
            .free_ranges()
            .iter()
            .map(|range| (range.start_frame_number(), range.end_frame_number()))
            .collect();

        assert_eq!(
            ranges,
            [
                (0x40000, 0x40080),
                (0x40090, 0x40100),
                (0x40101, 0x48000),
                (0x48001, 0x50000),
            ]
        );
        assert_eq!(
            memory_map_from_boot_info::<3>(info, kernel, PhysAddr::new(0x4010_0000)),
            Err(BootMemoryError::MemoryMap(MemoryMapError::CapacityExceeded))
        );
    }

    #[test]
    fn boot_memory_map_rejects_empty_kernel_and_exhausted_memory() {
        let reg = two_cell_reg(&[(0x4008_0000, 0x1_0000)]);
        let blob = boot_fixture(2, 2, &reg);
        let info = DeviceTree::from_bytes(&blob).unwrap().boot_info().unwrap();
        let kernel =
            AddressRange::new(PhysAddr::new(0x4008_0000), PhysAddr::new(0x4009_0000)).unwrap();
        let empty = AddressRange::new(PhysAddr::new(1), PhysAddr::new(1)).unwrap();

        assert_eq!(
            memory_map_from_boot_info::<2>(info, empty, PhysAddr::new(0x5000_0000)),
            Err(BootMemoryError::EmptyKernelImage)
        );
        assert_eq!(
            memory_map_from_boot_info::<2>(info, kernel, PhysAddr::new(0x5000_0000)),
            Err(BootMemoryError::NoUsableFrames)
        );
    }

    #[test]
    fn supports_one_cell_addresses_and_sizes() {
        let mut reg = Vec::new();
        push_u32(&mut reg, 0x4000_0000);
        push_u32(&mut reg, 0x0100_0000);
        let blob = boot_fixture(1, 1, &reg);
        let info = DeviceTree::from_bytes(&blob)
            .expect("valid tree")
            .boot_info()
            .expect("valid boot information");

        assert_eq!(
            info.memory_regions().next(),
            Some(Ok(AddressRange::new(
                PhysAddr::new(0x4000_0000),
                PhysAddr::new(0x4100_0000),
            )
            .expect("memory region")))
        );
    }

    #[test]
    fn rejects_unsupported_cell_counts() {
        let blob = boot_fixture(3, 2, &[]);
        let tree = DeviceTree::from_bytes(&blob).expect("structurally valid tree");

        assert_eq!(
            tree.boot_info(),
            Err(Error::UnsupportedCellCount {
                name: "#address-cells",
                count: 3,
            })
        );
    }

    #[test]
    fn rejects_partial_reg_entries() {
        let blob = boot_fixture(2, 2, &[0; 12]);
        let tree = DeviceTree::from_bytes(&blob).expect("structurally valid tree");

        assert_eq!(
            tree.boot_info(),
            Err(Error::MalformedReg {
                length: 12,
                entry_size: 16,
            })
        );
    }

    #[test]
    fn requires_at_least_one_nonempty_memory_range() {
        let reg = two_cell_reg(&[(0x4000_0000, 0)]);
        let blob = boot_fixture(2, 2, &reg);
        let tree = DeviceTree::from_bytes(&blob).expect("structurally valid tree");

        assert_eq!(tree.boot_info(), Err(Error::MissingMemory));
    }

    #[test]
    fn rejects_physical_range_overflow() {
        let reg = two_cell_reg(&[(u64::MAX - 0xfff, 0x2000)]);
        let blob = boot_fixture(2, 2, &reg);
        let tree = DeviceTree::from_bytes(&blob).expect("structurally valid tree");

        assert_eq!(
            tree.boot_info(),
            Err(Error::InvalidPhysicalRange {
                address: u64::MAX - 0xfff,
                size: 0x2000,
            })
        );
    }
}
