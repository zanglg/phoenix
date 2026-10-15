//! Strict, allocation-free ELF64 executable parsing for initial userspace.

use crate::memory::PAGE_SIZE;
use crate::user::{
    USER_MIN_MAPPABLE_ADDRESS, UserAddr, UserAddressError, UserPermissions, UserRange,
};

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const MAX_PROGRAM_HEADERS: usize = 128;
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LITTLE_ENDIAN: u8 = 1;
const ELF_VERSION_CURRENT: u8 = 1;
const ELF_OSABI_SYSV: u8 = 0;
const ELF_TYPE_EXECUTABLE: u16 = 2;
const ELF_MACHINE_AARCH64: u16 = 183;
const PROGRAM_TYPE_LOAD: u32 = 1;
const PROGRAM_FLAG_EXECUTE: u32 = 1;
const PROGRAM_FLAG_WRITE: u32 = 2;
const PROGRAM_FLAG_READ: u32 = 4;

/// Reason one loadable program header was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfSegmentError {
    /// Segment flags contain bits outside ELF `PF_R`, `PF_W`, and `PF_X`.
    UnknownFlags(u32),
    /// The file-backed portion is larger than the memory image.
    FileLargerThanMemory,
    /// The loadable segment has no memory extent.
    Empty,
    /// File offset or size cannot be represented or lies outside the image.
    FileRange,
    /// Virtual address or size cannot be represented or lies outside userspace.
    VirtualRange(UserAddressError),
    /// Segment alignment is smaller than one page or not a power of two.
    InvalidAlignment(u64),
    /// File offset and virtual address do not have equal alignment residues.
    IncongruentAlignment,
    /// The initial loader requires both file and virtual starts to be page aligned.
    NotPageAligned,
    /// Every loadable segment must be readable.
    NotReadable,
    /// User permissions are invalid, including writable-executable segments.
    InvalidPermissions(UserAddressError),
    /// The page-rounded virtual extent overlaps another loadable segment.
    Overlap,
}

/// Failure while validating an initial Phoenix userspace ELF image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    /// The byte slice is shorter than the fixed ELF64 header.
    HeaderTooShort,
    /// ELF magic bytes are absent.
    BadMagic,
    /// Only ELF64 is accepted.
    UnsupportedClass(u8),
    /// Only little-endian input is accepted.
    UnsupportedEndianness(u8),
    /// Identification version is not current.
    UnsupportedIdentificationVersion(u8),
    /// Only the System V ABI identifier is accepted initially.
    UnsupportedOsAbi(u8),
    /// Only fixed-address executable files are accepted initially.
    UnsupportedFileType(u16),
    /// The executable is not for AArch64.
    UnsupportedMachine(u16),
    /// ELF header version is not current.
    UnsupportedFileVersion(u32),
    /// The encoded ELF header size differs from the ELF64 size.
    InvalidHeaderSize(u16),
    /// The encoded program-header entry size differs from the ELF64 size.
    InvalidProgramHeaderSize(u16),
    /// No program headers are present.
    NoProgramHeaders,
    /// The program-header count exceeds the parser's fixed validation bound.
    TooManyProgramHeaders(u16),
    /// Program-header table arithmetic overflowed or extends beyond the image.
    ProgramHeaderTableOutOfBounds,
    /// A loadable program header is invalid.
    InvalidSegment {
        /// Program-header index.
        index: u16,
        /// Specific rejection reason.
        reason: ElfSegmentError,
    },
    /// The image contains no loadable segment.
    NoLoadSegments,
    /// The entry point is not a valid user address.
    InvalidEntry(UserAddressError),
    /// The entry point does not lie in an executable loadable segment.
    EntryNotExecutable,
}

/// One validated `PT_LOAD` segment borrowed from an ELF image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadSegment<'a> {
    virtual_range: UserRange,
    memory_size: usize,
    data: &'a [u8],
    permissions: UserPermissions,
}

impl<'a> LoadSegment<'a> {
    /// Return the page-rounded virtual range that needs mappings.
    pub const fn virtual_range(self) -> UserRange {
        self.virtual_range
    }

    /// Return the exact initialized-plus-zero-fill byte length.
    pub const fn memory_size(self) -> usize {
        self.memory_size
    }

    /// Return the initialized bytes borrowed from the ELF image.
    pub const fn data(self) -> &'a [u8] {
        self.data
    }

    /// Return the number of zero bytes following the initialized data.
    pub const fn zero_fill_size(self) -> usize {
        self.memory_size - self.data.len()
    }

    /// Return validated EL0 permissions.
    pub const fn permissions(self) -> UserPermissions {
        self.permissions
    }
}

/// Validated ELF64/AArch64 fixed-address executable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElfImage<'a> {
    bytes: &'a [u8],
    entry: UserAddr,
    program_header_offset: usize,
    program_header_count: u16,
    load_segment_count: u16,
}

impl<'a> ElfImage<'a> {
    /// Validate an ELF image without allocation or mutation.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() < ELF_HEADER_SIZE {
            return Err(ElfError::HeaderTooShort);
        }
        if bytes[..4] != *b"\x7fELF" {
            return Err(ElfError::BadMagic);
        }
        if bytes[4] != ELF_CLASS_64 {
            return Err(ElfError::UnsupportedClass(bytes[4]));
        }
        if bytes[5] != ELF_DATA_LITTLE_ENDIAN {
            return Err(ElfError::UnsupportedEndianness(bytes[5]));
        }
        if bytes[6] != ELF_VERSION_CURRENT {
            return Err(ElfError::UnsupportedIdentificationVersion(bytes[6]));
        }
        if bytes[7] != ELF_OSABI_SYSV {
            return Err(ElfError::UnsupportedOsAbi(bytes[7]));
        }

        let file_type = read_u16(bytes, 16);
        if file_type != ELF_TYPE_EXECUTABLE {
            return Err(ElfError::UnsupportedFileType(file_type));
        }
        let machine = read_u16(bytes, 18);
        if machine != ELF_MACHINE_AARCH64 {
            return Err(ElfError::UnsupportedMachine(machine));
        }
        let version = read_u32(bytes, 20);
        if version != u32::from(ELF_VERSION_CURRENT) {
            return Err(ElfError::UnsupportedFileVersion(version));
        }
        let header_size = read_u16(bytes, 52);
        if usize::from(header_size) != ELF_HEADER_SIZE {
            return Err(ElfError::InvalidHeaderSize(header_size));
        }
        let program_header_size = read_u16(bytes, 54);
        if usize::from(program_header_size) != PROGRAM_HEADER_SIZE {
            return Err(ElfError::InvalidProgramHeaderSize(program_header_size));
        }
        let program_header_count = read_u16(bytes, 56);
        if program_header_count == 0 {
            return Err(ElfError::NoProgramHeaders);
        }
        if usize::from(program_header_count) > MAX_PROGRAM_HEADERS {
            return Err(ElfError::TooManyProgramHeaders(program_header_count));
        }
        let program_header_offset = usize::try_from(read_u64(bytes, 32))
            .map_err(|_| ElfError::ProgramHeaderTableOutOfBounds)?;
        let table_size = usize::from(program_header_count)
            .checked_mul(PROGRAM_HEADER_SIZE)
            .ok_or(ElfError::ProgramHeaderTableOutOfBounds)?;
        let table_end = program_header_offset
            .checked_add(table_size)
            .ok_or(ElfError::ProgramHeaderTableOutOfBounds)?;
        if program_header_offset < ELF_HEADER_SIZE || table_end > bytes.len() {
            return Err(ElfError::ProgramHeaderTableOutOfBounds);
        }

        let entry_raw = usize::try_from(read_u64(bytes, 24)).map_err(|_| {
            ElfError::InvalidEntry(UserAddressError::OutsideUserRegion {
                address: usize::MAX,
            })
        })?;
        let entry = UserAddr::new(entry_raw).map_err(ElfError::InvalidEntry)?;
        let mut loaded_ranges = [None; MAX_PROGRAM_HEADERS];
        let mut load_segment_count = 0_usize;
        let mut entry_is_executable = false;

        for index in 0..program_header_count {
            let Some(segment) =
                parse_load_segment(bytes, program_header_offset, index, program_header_count)?
            else {
                continue;
            };
            if loaded_ranges[..load_segment_count]
                .iter()
                .flatten()
                .any(|range: &UserRange| range.overlaps(segment.virtual_range))
            {
                return Err(ElfError::InvalidSegment {
                    index,
                    reason: ElfSegmentError::Overlap,
                });
            }
            loaded_ranges[load_segment_count] = Some(segment.virtual_range);
            load_segment_count += 1;
            if segment.permissions.executable() && segment.virtual_range.contains(entry) {
                entry_is_executable = true;
            }
        }

        if load_segment_count == 0 {
            return Err(ElfError::NoLoadSegments);
        }
        if !entry_is_executable {
            return Err(ElfError::EntryNotExecutable);
        }
        Ok(Self {
            bytes,
            entry,
            program_header_offset,
            program_header_count,
            load_segment_count: load_segment_count as u16,
        })
    }

    /// Return the validated EL0 entry point.
    pub const fn entry(self) -> UserAddr {
        self.entry
    }

    /// Return the number of loadable segments.
    pub const fn load_segment_count(self) -> u16 {
        self.load_segment_count
    }

    /// Iterate over validated loadable segments in program-header order.
    pub const fn load_segments(&self) -> LoadSegments<'a> {
        LoadSegments {
            bytes: self.bytes,
            program_header_offset: self.program_header_offset,
            program_header_count: self.program_header_count,
            next_index: 0,
        }
    }
}

/// Iterator over validated `PT_LOAD` entries.
pub struct LoadSegments<'a> {
    bytes: &'a [u8],
    program_header_offset: usize,
    program_header_count: u16,
    next_index: u16,
}

impl<'a> Iterator for LoadSegments<'a> {
    type Item = Result<LoadSegment<'a>, ElfError>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next_index < self.program_header_count {
            let index = self.next_index;
            self.next_index += 1;
            match parse_load_segment(
                self.bytes,
                self.program_header_offset,
                index,
                self.program_header_count,
            ) {
                Ok(Some(segment)) => return Some(Ok(segment)),
                Ok(None) => {}
                Err(error) => return Some(Err(error)),
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let maximum = usize::from(self.program_header_count - self.next_index);
        (0, Some(maximum))
    }
}

fn parse_load_segment<'a>(
    bytes: &'a [u8],
    table_offset: usize,
    index: u16,
    validated_count: u16,
) -> Result<Option<LoadSegment<'a>>, ElfError> {
    debug_assert!(index < validated_count);
    let header = table_offset + usize::from(index) * PROGRAM_HEADER_SIZE;
    if read_u32(bytes, header) != PROGRAM_TYPE_LOAD {
        return Ok(None);
    }
    let invalid = |reason| ElfError::InvalidSegment { index, reason };
    let flags = read_u32(bytes, header + 4);
    if flags & !(PROGRAM_FLAG_READ | PROGRAM_FLAG_WRITE | PROGRAM_FLAG_EXECUTE) != 0 {
        return Err(invalid(ElfSegmentError::UnknownFlags(flags)));
    }
    if flags & PROGRAM_FLAG_READ == 0 {
        return Err(invalid(ElfSegmentError::NotReadable));
    }

    let file_size_u64 = read_u64(bytes, header + 32);
    let memory_size_u64 = read_u64(bytes, header + 40);
    if file_size_u64 > memory_size_u64 {
        return Err(invalid(ElfSegmentError::FileLargerThanMemory));
    }
    if memory_size_u64 == 0 {
        return Err(invalid(ElfSegmentError::Empty));
    }
    let alignment = read_u64(bytes, header + 48);
    if alignment < PAGE_SIZE as u64 || !alignment.is_power_of_two() {
        return Err(invalid(ElfSegmentError::InvalidAlignment(alignment)));
    }

    let file_offset_u64 = read_u64(bytes, header + 8);
    let virtual_start_u64 = read_u64(bytes, header + 16);
    if file_offset_u64 % alignment != virtual_start_u64 % alignment {
        return Err(invalid(ElfSegmentError::IncongruentAlignment));
    }
    let file_offset =
        usize::try_from(file_offset_u64).map_err(|_| invalid(ElfSegmentError::FileRange))?;
    let file_size =
        usize::try_from(file_size_u64).map_err(|_| invalid(ElfSegmentError::FileRange))?;
    let memory_size = usize::try_from(memory_size_u64).map_err(|_| {
        invalid(ElfSegmentError::VirtualRange(
            UserAddressError::AddressOverflow,
        ))
    })?;
    let virtual_start_raw = usize::try_from(virtual_start_u64).map_err(|_| {
        invalid(ElfSegmentError::VirtualRange(
            UserAddressError::OutsideUserRegion {
                address: usize::MAX,
            },
        ))
    })?;
    if virtual_start_raw < USER_MIN_MAPPABLE_ADDRESS {
        return Err(invalid(ElfSegmentError::VirtualRange(
            UserAddressError::NullPageMapping,
        )));
    }
    if !file_offset.is_multiple_of(PAGE_SIZE) || !virtual_start_raw.is_multiple_of(PAGE_SIZE) {
        return Err(invalid(ElfSegmentError::NotPageAligned));
    }
    let file_end = file_offset
        .checked_add(file_size)
        .ok_or_else(|| invalid(ElfSegmentError::FileRange))?;
    let data = bytes
        .get(file_offset..file_end)
        .ok_or_else(|| invalid(ElfSegmentError::FileRange))?;
    let virtual_start = UserAddr::new(virtual_start_raw)
        .map_err(|error| invalid(ElfSegmentError::VirtualRange(error)))?;
    let mapped_size = align_up(memory_size, PAGE_SIZE).ok_or_else(|| {
        invalid(ElfSegmentError::VirtualRange(
            UserAddressError::AddressOverflow,
        ))
    })?;
    let virtual_range = UserRange::from_start_len(virtual_start, mapped_size)
        .map_err(|error| invalid(ElfSegmentError::VirtualRange(error)))?;
    let permissions = UserPermissions::new(
        true,
        flags & PROGRAM_FLAG_WRITE != 0,
        flags & PROGRAM_FLAG_EXECUTE != 0,
    )
    .map_err(|error| invalid(ElfSegmentError::InvalidPermissions(error)))?;

    Ok(Some(LoadSegment {
        virtual_range,
        memory_size,
        data,
        permissions,
    }))
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|sum| sum & !(alignment - 1))
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use super::{ElfError, ElfImage, ElfSegmentError};
    use crate::memory::PAGE_SIZE;
    use crate::user::{UserAddr, UserAddressError, UserPermissions};

    const TEXT_HEADER: usize = 64;
    const DATA_HEADER: usize = 64 + 56;

    #[test]
    fn parses_a_strict_two_segment_aarch64_executable() {
        let bytes = fixture();
        let image = ElfImage::parse(&bytes).expect("valid fixture");
        let segments: Vec<_> = image
            .load_segments()
            .collect::<Result<Vec<_>, _>>()
            .expect("validated segments remain valid");

        assert_eq!(image.entry(), UserAddr::new(0x40_0000).unwrap());
        assert_eq!(image.load_segment_count(), 2);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].data(), b"TEXT");
        assert_eq!(segments[0].virtual_range().byte_len(), PAGE_SIZE);
        assert_eq!(segments[0].permissions(), UserPermissions::read_execute());
        assert_eq!(segments[1].data(), b"DATA");
        assert_eq!(segments[1].memory_size(), 0x20);
        assert_eq!(segments[1].zero_fill_size(), 0x1c);
        assert_eq!(segments[1].permissions(), UserPermissions::read_write());
    }

    #[test]
    fn rejects_truncated_or_wrong_identity_headers() {
        assert_eq!(ElfImage::parse(&[]), Err(ElfError::HeaderTooShort));
        let mut bytes = fixture();
        bytes[0] = 0;
        assert_eq!(ElfImage::parse(&bytes), Err(ElfError::BadMagic));
        let mut bytes = fixture();
        bytes[5] = 2;
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::UnsupportedEndianness(2))
        );
    }

    #[test]
    fn rejects_wrong_machine_type_or_program_header_shape() {
        let mut bytes = fixture();
        put_u16(&mut bytes, 18, 62);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::UnsupportedMachine(62))
        );

        let mut bytes = fixture();
        put_u16(&mut bytes, 54, 0);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidProgramHeaderSize(0))
        );
    }

    #[test]
    fn rejects_file_larger_than_memory_and_out_of_bounds_data() {
        let mut bytes = fixture();
        put_u64(&mut bytes, TEXT_HEADER + 32, 8);
        put_u64(&mut bytes, TEXT_HEADER + 40, 4);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidSegment {
                index: 0,
                reason: ElfSegmentError::FileLargerThanMemory,
            })
        );

        let mut bytes = fixture();
        let file_end = bytes.len() as u64;
        put_u64(&mut bytes, DATA_HEADER + 8, file_end);
        put_u64(&mut bytes, DATA_HEADER + 16, 0x500000);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidSegment {
                index: 1,
                reason: ElfSegmentError::FileRange,
            })
        );
    }

    #[test]
    fn rejects_writable_executable_or_unreadable_segments() {
        let mut bytes = fixture();
        put_u32(&mut bytes, TEXT_HEADER + 4, 7);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidSegment {
                index: 0,
                reason: ElfSegmentError::InvalidPermissions(UserAddressError::WritableExecutable),
            })
        );

        let mut bytes = fixture();
        put_u32(&mut bytes, TEXT_HEADER + 4, 1);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidSegment {
                index: 0,
                reason: ElfSegmentError::NotReadable,
            })
        );
    }

    #[test]
    fn rejects_page_overlap_and_an_entry_outside_executable_memory() {
        let mut bytes = fixture();
        put_u64(&mut bytes, DATA_HEADER + 16, 0x40_0000);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidSegment {
                index: 1,
                reason: ElfSegmentError::Overlap,
            })
        );

        let mut bytes = fixture();
        put_u64(&mut bytes, 24, 0x41_0000);
        assert_eq!(ElfImage::parse(&bytes), Err(ElfError::EntryNotExecutable));
    }

    #[test]
    fn rejects_null_page_and_unaligned_loads() {
        let mut bytes = fixture();
        put_u64(&mut bytes, TEXT_HEADER + 16, 0);
        put_u64(&mut bytes, 24, 0);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidSegment {
                index: 0,
                reason: ElfSegmentError::VirtualRange(UserAddressError::NullPageMapping),
            })
        );

        let mut bytes = fixture();
        put_u64(&mut bytes, TEXT_HEADER + 8, 0x1001);
        put_u64(&mut bytes, TEXT_HEADER + 16, 0x40_0001);
        put_u64(&mut bytes, 24, 0x40_0001);
        assert_eq!(
            ElfImage::parse(&bytes),
            Err(ElfError::InvalidSegment {
                index: 0,
                reason: ElfSegmentError::NotPageAligned,
            })
        );
    }

    fn fixture() -> Vec<u8> {
        let mut bytes = vec![0_u8; 0x3000];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[7] = 0;
        put_u16(&mut bytes, 16, 2);
        put_u16(&mut bytes, 18, 183);
        put_u32(&mut bytes, 20, 1);
        put_u64(&mut bytes, 24, 0x40_0000);
        put_u64(&mut bytes, 32, 64);
        put_u16(&mut bytes, 52, 64);
        put_u16(&mut bytes, 54, 56);
        put_u16(&mut bytes, 56, 2);

        program_header(&mut bytes, TEXT_HEADER, 5, 0x1000, 0x40_0000, 4, 4);
        program_header(&mut bytes, DATA_HEADER, 6, 0x2000, 0x41_0000, 4, 0x20);
        bytes[0x1000..0x1004].copy_from_slice(b"TEXT");
        bytes[0x2000..0x2004].copy_from_slice(b"DATA");
        bytes
    }

    fn program_header(
        bytes: &mut [u8],
        header: usize,
        flags: u32,
        offset: u64,
        virtual_address: u64,
        file_size: u64,
        memory_size: u64,
    ) {
        put_u32(bytes, header, 1);
        put_u32(bytes, header + 4, flags);
        put_u64(bytes, header + 8, offset);
        put_u64(bytes, header + 16, virtual_address);
        put_u64(bytes, header + 24, 0);
        put_u64(bytes, header + 32, file_size);
        put_u64(bytes, header + 40, memory_size);
        put_u64(bytes, header + 48, PAGE_SIZE as u64);
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
