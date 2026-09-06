//! Strict, allocation-free Flattened Device Tree parsing.

use core::str;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_HEADER_SIZE: usize = 40;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;
const SUPPORTED_LAST_COMPATIBLE_VERSION: u32 = 17;

/// A named region of a DTB used in parse errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Block {
    /// Fixed-size DTB header.
    Header,
    /// Memory reservation entries.
    Reservations,
    /// Tokenized node and property structure.
    Structure,
    /// Property-name string table.
    Strings,
}

/// Failure while validating or traversing a DTB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The input is shorter than the fixed DTB header.
    TooShort,
    /// The header magic is not the flattened-device-tree magic.
    BadMagic {
        /// Magic value found in the input.
        found: u32,
    },
    /// The format version cannot be consumed by this parser.
    UnsupportedVersion {
        /// Format version supplied by the producer.
        version: u32,
        /// Oldest format version with which the blob is compatible.
        last_compatible: u32,
    },
    /// The declared total size does not fit inside the supplied bytes.
    InvalidTotalSize {
        /// Declared blob size.
        declared: usize,
        /// Number of supplied bytes.
        available: usize,
    },
    /// A block offset does not meet the format's alignment requirement.
    MisalignedBlock {
        /// Block with the invalid offset.
        block: Block,
        /// Rejected byte offset.
        offset: usize,
        /// Required alignment in bytes.
        alignment: usize,
    },
    /// A declared block lies outside the DTB or overlaps an earlier block.
    InvalidBlockRange {
        /// Block with the invalid range.
        block: Block,
        /// Block byte offset.
        offset: usize,
        /// Block byte size.
        size: usize,
    },
    /// The reservation list has no zero-address, zero-size terminator.
    UnterminatedReservations,
    /// A reservation's address plus size exceeds 64-bit physical space.
    InvalidReservationRange {
        /// Physical reservation start.
        address: u64,
        /// Reservation length in bytes.
        size: u64,
    },
    /// Two non-empty firmware reservations overlap.
    OverlappingReservations {
        /// Start address of the earlier reservation entry.
        first_address: u64,
        /// Start address of the later reservation entry.
        second_address: u64,
    },
    /// A structure token or its payload ends outside the structure block.
    TruncatedStructure {
        /// Byte offset inside the structure block.
        offset: usize,
    },
    /// A node name has no terminating zero byte.
    UnterminatedNodeName {
        /// Byte offset inside the structure block.
        offset: usize,
    },
    /// A node name or property name is not UTF-8.
    InvalidUtf8,
    /// A property name offset lies outside the strings block.
    InvalidPropertyName {
        /// Byte offset inside the strings block.
        offset: usize,
    },
    /// A property name has no terminating zero byte.
    UnterminatedPropertyName {
        /// Byte offset inside the strings block.
        offset: usize,
    },
    /// The structure block contains an unknown token.
    UnknownToken {
        /// Unknown token value.
        token: u32,
        /// Byte offset of the token inside the structure block.
        offset: usize,
    },
    /// An end-node token appeared without a matching begin-node token.
    UnexpectedEndNode,
    /// A property appeared outside a node.
    PropertyOutsideNode,
    /// The structure contains zero or multiple root nodes.
    InvalidRootCount,
    /// The structure ended while nodes remained open.
    UnclosedNodes,
    /// The structure block has no final end token.
    MissingEndToken,
    /// Bytes remain in the declared structure block after its end token.
    TrailingStructureData {
        /// Offset immediately after the end token.
        offset: usize,
    },
    /// Node nesting exceeded the representable depth.
    DepthOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Header {
    total_size: usize,
    structure_offset: usize,
    strings_offset: usize,
    reservations_offset: usize,
    structure_size: usize,
    strings_size: usize,
    boot_cpu_id: u32,
}

/// A validated, borrowed flattened device tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceTree<'a> {
    blob: &'a [u8],
    header: Header,
}

impl<'a> DeviceTree<'a> {
    /// Validate a DTB and borrow it without allocation or copying.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, Error> {
        let header = parse_header(bytes)?;
        validate_header_layout(header)?;
        let blob = &bytes[..header.total_size];
        validate_reservations(blob, header)?;

        let tree = Self { blob, header };
        tree.validate_structure()?;
        Ok(tree)
    }

    /// Return the total validated blob size.
    pub const fn total_size(self) -> usize {
        self.header.total_size
    }

    /// Return the physical ID of the boot CPU recorded by the blob producer.
    pub const fn boot_cpu_id(self) -> u32 {
        self.header.boot_cpu_id
    }

    /// Iterate over memory reservations, excluding the terminating entry.
    pub fn reservations(self) -> Reservations<'a> {
        Reservations {
            bytes: &self.blob[self.header.reservations_offset..self.header.structure_offset],
            offset: 0,
            finished: false,
        }
    }

    /// Traverse structure events in source order.
    pub fn events(self) -> Events<'a> {
        Events {
            structure: &self.blob[self.header.structure_offset
                ..self.header.structure_offset + self.header.structure_size],
            strings: &self.blob
                [self.header.strings_offset..self.header.strings_offset + self.header.strings_size],
            offset: 0,
            finished: false,
        }
    }

    fn validate_structure(self) -> Result<(), Error> {
        let mut depth = 0_usize;
        let mut roots = 0_usize;
        let mut saw_end = false;

        let mut events = self.events();
        while let Some(event) = events.next() {
            match event? {
                Event::BeginNode { .. } => {
                    if depth == 0 {
                        roots = roots.checked_add(1).ok_or(Error::DepthOverflow)?;
                    }
                    depth = depth.checked_add(1).ok_or(Error::DepthOverflow)?;
                }
                Event::EndNode => {
                    depth = depth.checked_sub(1).ok_or(Error::UnexpectedEndNode)?;
                }
                Event::Property { .. } if depth == 0 => return Err(Error::PropertyOutsideNode),
                Event::Property { .. } | Event::Nop => {}
                Event::End => {
                    saw_end = true;
                    if events.offset != events.structure.len() {
                        return Err(Error::TrailingStructureData {
                            offset: events.offset,
                        });
                    }
                    break;
                }
            }
        }

        if !saw_end {
            return Err(Error::MissingEndToken);
        }
        if depth != 0 {
            return Err(Error::UnclosedNodes);
        }
        if roots != 1 {
            return Err(Error::InvalidRootCount);
        }
        Ok(())
    }
}

/// One firmware-reserved physical address range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reservation {
    /// Physical start address.
    pub address: u64,
    /// Reserved length in bytes.
    pub size: u64,
}

/// Iterator over a validated DTB memory reservation list.
#[derive(Clone, Debug)]
pub struct Reservations<'a> {
    bytes: &'a [u8],
    offset: usize,
    finished: bool,
}

impl Iterator for Reservations<'_> {
    type Item = Reservation;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let address = read_be_u64(self.bytes, self.offset).expect("reservation map was validated");
        let size = read_be_u64(self.bytes, self.offset + 8).expect("reservation map was validated");
        self.offset += 16;
        if address == 0 && size == 0 {
            self.finished = true;
            None
        } else {
            Some(Reservation { address, size })
        }
    }
}

/// One event in the flattened tree's structure block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event<'a> {
    /// Start of a node, with its unit name.
    BeginNode {
        /// Node name without its terminating zero byte.
        name: &'a str,
    },
    /// End of the current node.
    EndNode,
    /// A named property and its uninterpreted bytes.
    Property {
        /// Property name borrowed from the strings block.
        name: &'a str,
        /// Property value borrowed from the structure block.
        value: &'a [u8],
    },
    /// Padding or producer-supplied no-operation token.
    Nop,
    /// End of the structure.
    End,
}

/// Iterator over events in a validated DTB structure block.
#[derive(Clone, Debug)]
pub struct Events<'a> {
    structure: &'a [u8],
    strings: &'a [u8],
    offset: usize,
    finished: bool,
}

impl<'a> Iterator for Events<'a> {
    type Item = Result<Event<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let token_offset = self.offset;
        let token = match read_be_u32(self.structure, self.offset) {
            Some(token) => token,
            None => {
                self.finished = true;
                return None;
            }
        };
        self.offset += 4;

        let event = match token {
            FDT_BEGIN_NODE => self.parse_begin_node(),
            FDT_END_NODE => Ok(Event::EndNode),
            FDT_PROP => self.parse_property(),
            FDT_NOP => Ok(Event::Nop),
            FDT_END => {
                self.finished = true;
                Ok(Event::End)
            }
            token => Err(Error::UnknownToken {
                token,
                offset: token_offset,
            }),
        };
        if event.is_err() {
            self.finished = true;
        }
        Some(event)
    }
}

impl<'a> Events<'a> {
    fn parse_begin_node(&mut self) -> Result<Event<'a>, Error> {
        let name_offset = self.offset;
        let remaining = self
            .structure
            .get(name_offset..)
            .ok_or(Error::TruncatedStructure {
                offset: name_offset,
            })?;
        let length =
            remaining
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(Error::UnterminatedNodeName {
                    offset: name_offset,
                })?;
        let name = str::from_utf8(&remaining[..length]).map_err(|_| Error::InvalidUtf8)?;
        self.offset = align_4(name_offset + length + 1).ok_or(Error::TruncatedStructure {
            offset: name_offset,
        })?;
        if self.offset > self.structure.len() {
            return Err(Error::TruncatedStructure {
                offset: name_offset,
            });
        }
        Ok(Event::BeginNode { name })
    }

    fn parse_property(&mut self) -> Result<Event<'a>, Error> {
        let header_offset = self.offset;
        let length =
            read_be_u32(self.structure, header_offset).ok_or(Error::TruncatedStructure {
                offset: header_offset,
            })? as usize;
        let name_offset =
            read_be_u32(self.structure, header_offset + 4).ok_or(Error::TruncatedStructure {
                offset: header_offset,
            })? as usize;
        let value_offset = header_offset + 8;
        let value_end = value_offset
            .checked_add(length)
            .ok_or(Error::TruncatedStructure {
                offset: value_offset,
            })?;
        let value =
            self.structure
                .get(value_offset..value_end)
                .ok_or(Error::TruncatedStructure {
                    offset: value_offset,
                })?;
        self.offset = align_4(value_end).ok_or(Error::TruncatedStructure {
            offset: value_offset,
        })?;
        if self.offset > self.structure.len() {
            return Err(Error::TruncatedStructure {
                offset: value_offset,
            });
        }

        let name_bytes = self
            .strings
            .get(name_offset..)
            .ok_or(Error::InvalidPropertyName {
                offset: name_offset,
            })?;
        let name_length = name_bytes.iter().position(|byte| *byte == 0).ok_or(
            Error::UnterminatedPropertyName {
                offset: name_offset,
            },
        )?;
        let name = str::from_utf8(&name_bytes[..name_length]).map_err(|_| Error::InvalidUtf8)?;
        Ok(Event::Property { name, value })
    }
}

fn parse_header(bytes: &[u8]) -> Result<Header, Error> {
    if bytes.len() < FDT_HEADER_SIZE {
        return Err(Error::TooShort);
    }
    let magic = read_be_u32(bytes, 0).expect("fixed header length was checked");
    if magic != FDT_MAGIC {
        return Err(Error::BadMagic { found: magic });
    }

    let total_size = read_be_u32(bytes, 4).expect("fixed header length was checked") as usize;
    if total_size < FDT_HEADER_SIZE || total_size > bytes.len() {
        return Err(Error::InvalidTotalSize {
            declared: total_size,
            available: bytes.len(),
        });
    }

    let version = read_be_u32(bytes, 20).expect("fixed header length was checked");
    let last_compatible = read_be_u32(bytes, 24).expect("fixed header length was checked");
    if version < SUPPORTED_LAST_COMPATIBLE_VERSION
        || last_compatible > SUPPORTED_LAST_COMPATIBLE_VERSION
        || last_compatible > version
    {
        return Err(Error::UnsupportedVersion {
            version,
            last_compatible,
        });
    }

    Ok(Header {
        total_size,
        structure_offset: read_be_u32(bytes, 8).expect("fixed header length was checked") as usize,
        strings_offset: read_be_u32(bytes, 12).expect("fixed header length was checked") as usize,
        reservations_offset: read_be_u32(bytes, 16).expect("fixed header length was checked")
            as usize,
        boot_cpu_id: read_be_u32(bytes, 28).expect("fixed header length was checked"),
        strings_size: read_be_u32(bytes, 32).expect("fixed header length was checked") as usize,
        structure_size: read_be_u32(bytes, 36).expect("fixed header length was checked") as usize,
    })
}

fn validate_header_layout(header: Header) -> Result<(), Error> {
    validate_alignment(Block::Reservations, header.reservations_offset, 8)?;
    validate_alignment(Block::Structure, header.structure_offset, 4)?;
    if header.reservations_offset < FDT_HEADER_SIZE
        || header.reservations_offset >= header.structure_offset
    {
        return Err(Error::InvalidBlockRange {
            block: Block::Reservations,
            offset: header.reservations_offset,
            size: header
                .structure_offset
                .saturating_sub(header.reservations_offset),
        });
    }
    validate_block(
        Block::Structure,
        header.structure_offset,
        header.structure_size,
        header.total_size,
    )?;
    validate_block(
        Block::Strings,
        header.strings_offset,
        header.strings_size,
        header.total_size,
    )?;
    let structure_end = header
        .structure_offset
        .checked_add(header.structure_size)
        .ok_or(Error::InvalidBlockRange {
            block: Block::Structure,
            offset: header.structure_offset,
            size: header.structure_size,
        })?;
    if structure_end > header.strings_offset {
        return Err(Error::InvalidBlockRange {
            block: Block::Strings,
            offset: header.strings_offset,
            size: header.strings_size,
        });
    }
    Ok(())
}

fn validate_alignment(block: Block, offset: usize, alignment: usize) -> Result<(), Error> {
    if offset & (alignment - 1) == 0 {
        Ok(())
    } else {
        Err(Error::MisalignedBlock {
            block,
            offset,
            alignment,
        })
    }
}

fn validate_block(
    block: Block,
    offset: usize,
    size: usize,
    total_size: usize,
) -> Result<(), Error> {
    match offset.checked_add(size) {
        Some(end) if offset >= FDT_HEADER_SIZE && end <= total_size => Ok(()),
        _ => Err(Error::InvalidBlockRange {
            block,
            offset,
            size,
        }),
    }
}

fn validate_reservations(blob: &[u8], header: Header) -> Result<(), Error> {
    let bytes = blob
        .get(header.reservations_offset..header.structure_offset)
        .ok_or(Error::InvalidBlockRange {
            block: Block::Reservations,
            offset: header.reservations_offset,
            size: header
                .structure_offset
                .saturating_sub(header.reservations_offset),
        })?;
    let mut offset = 0_usize;
    while let Some(entry) = bytes.get(offset..offset.saturating_add(16)) {
        let address = read_be_u64(entry, 0).expect("reservation entry has a fixed size");
        let size = read_be_u64(entry, 8).expect("reservation entry has a fixed size");
        if address == 0 && size == 0 {
            return Ok(());
        }
        let end = address
            .checked_add(size)
            .ok_or(Error::InvalidReservationRange { address, size })?;

        let mut previous_offset = 0;
        while previous_offset < offset {
            let previous_address =
                read_be_u64(bytes, previous_offset).expect("earlier reservation was validated");
            let previous_size =
                read_be_u64(bytes, previous_offset + 8).expect("earlier reservation was validated");
            let previous_end = previous_address
                .checked_add(previous_size)
                .expect("earlier reservation range was validated");
            if address < previous_end && previous_address < end {
                return Err(Error::OverlappingReservations {
                    first_address: previous_address,
                    second_address: address,
                });
            }
            previous_offset += 16;
        }
        offset += 16;
    }
    Err(Error::UnterminatedReservations)
}

fn align_4(value: usize) -> Option<usize> {
    value.checked_add(3).map(|value| value & !3)
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(value))
}

fn read_be_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let value: [u8; 8] = bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_be_bytes(value))
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use super::{
        Block, DeviceTree, Error, Event, FDT_BEGIN_NODE, FDT_END, FDT_END_NODE, FDT_MAGIC,
        FDT_PROP, Reservation,
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

    fn fixture() -> Vec<u8> {
        let strings = b"compatible\0device_type\0";
        let mut reservations = Vec::new();
        push_u64(&mut reservations, 0x4100_0000);
        push_u64(&mut reservations, 0x2000);
        push_u64(&mut reservations, 0x4200_0000);
        push_u64(&mut reservations, 0x1000);
        push_u64(&mut reservations, 0);
        push_u64(&mut reservations, 0);

        let mut structure = Vec::new();
        push_u32(&mut structure, FDT_BEGIN_NODE);
        structure.push(0);
        pad_4(&mut structure);

        push_u32(&mut structure, FDT_PROP);
        push_u32(&mut structure, 13);
        push_u32(&mut structure, 0);
        structure.extend_from_slice(b"phoenix,test\0");
        pad_4(&mut structure);

        push_u32(&mut structure, FDT_BEGIN_NODE);
        structure.extend_from_slice(b"memory@40000000\0");
        pad_4(&mut structure);
        push_u32(&mut structure, FDT_PROP);
        push_u32(&mut structure, 7);
        push_u32(&mut structure, 11);
        structure.extend_from_slice(b"memory\0");
        pad_4(&mut structure);
        push_u32(&mut structure, FDT_END_NODE);
        push_u32(&mut structure, FDT_END_NODE);
        push_u32(&mut structure, FDT_END);

        let reservations_offset = 40_usize;
        let structure_offset = reservations_offset + reservations.len();
        let strings_offset = structure_offset + structure.len();
        let total_size = strings_offset + strings.len();

        let mut blob = Vec::new();
        push_u32(&mut blob, FDT_MAGIC);
        push_u32(&mut blob, total_size as u32);
        push_u32(&mut blob, structure_offset as u32);
        push_u32(&mut blob, strings_offset as u32);
        push_u32(&mut blob, reservations_offset as u32);
        push_u32(&mut blob, 17);
        push_u32(&mut blob, 16);
        push_u32(&mut blob, 7);
        push_u32(&mut blob, strings.len() as u32);
        push_u32(&mut blob, structure.len() as u32);
        blob.extend_from_slice(&reservations);
        blob.extend_from_slice(&structure);
        blob.extend_from_slice(strings);
        blob
    }

    fn set_u32(blob: &mut [u8], offset: usize, value: u32) {
        blob[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn parses_header_reservations_and_structure_without_allocation() {
        let blob = fixture();
        let tree = DeviceTree::from_bytes(&blob).expect("valid fixture");

        assert_eq!(tree.total_size(), blob.len());
        assert_eq!(tree.boot_cpu_id(), 7);
        assert_eq!(
            tree.reservations().collect::<Vec<_>>(),
            vec![
                Reservation {
                    address: 0x4100_0000,
                    size: 0x2000,
                },
                Reservation {
                    address: 0x4200_0000,
                    size: 0x1000,
                }
            ]
        );
        assert_eq!(
            tree.events().collect::<Result<Vec<_>, _>>(),
            Ok(vec![
                Event::BeginNode { name: "" },
                Event::Property {
                    name: "compatible",
                    value: b"phoenix,test\0",
                },
                Event::BeginNode {
                    name: "memory@40000000",
                },
                Event::Property {
                    name: "device_type",
                    value: b"memory\0",
                },
                Event::EndNode,
                Event::EndNode,
                Event::End,
            ])
        );
    }

    #[test]
    fn rejects_short_header_and_bad_magic() {
        assert_eq!(DeviceTree::from_bytes(&[]), Err(Error::TooShort));

        let mut blob = fixture();
        set_u32(&mut blob, 0, 0x1234_5678);
        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::BadMagic { found: 0x1234_5678 })
        );
    }

    #[test]
    fn rejects_total_size_outside_supplied_bytes() {
        let mut blob = fixture();
        let available = blob.len();
        set_u32(&mut blob, 4, (available + 1) as u32);

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::InvalidTotalSize {
                declared: available + 1,
                available,
            })
        );
    }

    #[test]
    fn rejects_incompatible_versions() {
        let mut blob = fixture();
        set_u32(&mut blob, 20, 16);
        set_u32(&mut blob, 24, 16);

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::UnsupportedVersion {
                version: 16,
                last_compatible: 16,
            })
        );
    }

    #[test]
    fn rejects_misaligned_structure_offset() {
        let mut blob = fixture();
        let offset = u32::from_be_bytes(blob[8..12].try_into().expect("header field"));
        set_u32(&mut blob, 8, offset + 1);

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::MisalignedBlock {
                block: Block::Structure,
                offset: offset as usize + 1,
                alignment: 4,
            })
        );
    }

    #[test]
    fn rejects_unterminated_reservation_map() {
        let mut blob = fixture();
        for byte in &mut blob[72..88] {
            *byte = 1;
        }

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::UnterminatedReservations)
        );
    }

    #[test]
    fn rejects_overlapping_reservations() {
        let mut blob = fixture();
        blob[56..64].copy_from_slice(&0x4100_1000_u64.to_be_bytes());

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::OverlappingReservations {
                first_address: 0x4100_0000,
                second_address: 0x4100_1000,
            })
        );
    }

    #[test]
    fn accepts_unaligned_strings_block() {
        let mut blob = fixture();
        let strings_offset =
            u32::from_be_bytes(blob[12..16].try_into().expect("header field")) as usize;
        blob.insert(strings_offset, 0xaa);
        let total_size = blob.len() as u32;
        set_u32(&mut blob, 4, total_size);
        set_u32(&mut blob, 12, strings_offset as u32 + 1);

        DeviceTree::from_bytes(&blob).expect("strings block has no alignment requirement");
    }

    #[test]
    fn rejects_data_after_structure_end_token() {
        let mut blob = fixture();
        let strings_offset =
            u32::from_be_bytes(blob[12..16].try_into().expect("header field")) as usize;
        let structure_size = u32::from_be_bytes(blob[36..40].try_into().expect("header field"));
        blob.splice(strings_offset..strings_offset, [0, 0, 0, 0]);
        let total_size = blob.len() as u32;
        set_u32(&mut blob, 4, total_size);
        set_u32(&mut blob, 12, strings_offset as u32 + 4);
        set_u32(&mut blob, 36, structure_size + 4);

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::TrailingStructureData {
                offset: structure_size as usize,
            })
        );
    }

    #[test]
    fn rejects_unknown_structure_tokens() {
        let mut blob = fixture();
        let structure_offset =
            u32::from_be_bytes(blob[8..12].try_into().expect("header field")) as usize;
        set_u32(&mut blob, structure_offset, 0xfeed_face);

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::UnknownToken {
                token: 0xfeed_face,
                offset: 0,
            })
        );
    }

    #[test]
    fn rejects_properties_with_names_outside_the_string_block() {
        let mut blob = fixture();
        let structure_offset =
            u32::from_be_bytes(blob[8..12].try_into().expect("header field")) as usize;
        set_u32(&mut blob, structure_offset + 16, u32::MAX);

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::InvalidPropertyName {
                offset: u32::MAX as usize,
            })
        );
    }

    #[test]
    fn rejects_structure_with_unclosed_root() {
        let mut blob = fixture();
        let structure_offset =
            u32::from_be_bytes(blob[8..12].try_into().expect("header field")) as usize;
        let structure_size =
            u32::from_be_bytes(blob[36..40].try_into().expect("header field")) as usize;
        set_u32(&mut blob, structure_offset + structure_size - 8, FDT_END);
        set_u32(&mut blob, 36, structure_size as u32 - 4);

        assert_eq!(DeviceTree::from_bytes(&blob), Err(Error::UnclosedNodes));
    }

    #[test]
    fn rejects_truncated_property_payload() {
        let mut blob = fixture();
        let structure_offset =
            u32::from_be_bytes(blob[8..12].try_into().expect("header field")) as usize;
        set_u32(&mut blob, structure_offset + 12, u32::MAX);

        assert_eq!(
            DeviceTree::from_bytes(&blob),
            Err(Error::TruncatedStructure { offset: 20 })
        );
    }
}
