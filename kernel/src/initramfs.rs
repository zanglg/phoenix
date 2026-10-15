//! Strict, allocation-free parser for Phoenix's initial `newc` initramfs subset.

use core::str;

const HEADER_SIZE: usize = 110;
const FIELD_SIZE: usize = 8;
const MAGIC: &[u8; 6] = b"070701";
const TRAILER: &[u8] = b"TRAILER!!!";
const MAX_PATH_BYTES: usize = 4096;
const MAX_ENTRIES: usize = 128;
const FILE_TYPE_MASK: u32 = 0o170_000;
const REGULAR_FILE: u32 = 0o100_000;
const DIRECTORY: u32 = 0o040_000;

/// Rejected path shape in the strict initial archive contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathError {
    /// The path is empty.
    Empty,
    /// Archive paths must be relative to the future root.
    Absolute,
    /// The path ends in a slash.
    TrailingSlash,
    /// The path contains an empty component between two slashes.
    EmptyComponent,
    /// The path contains a current-directory component.
    CurrentDirectory,
    /// The path contains a parent-directory component.
    ParentDirectory,
    /// The path contains a NUL byte and cannot name an archive object.
    InteriorNul,
}

/// Failure while validating a complete uncompressed `newc` archive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitramfsError {
    /// A header, name, data region, or padding region extends past the input.
    Truncated {
        /// Offset at which the region was expected to start.
        offset: usize,
    },
    /// A record does not use the checksum-free `070701` encoding.
    UnsupportedMagic {
        /// Header offset containing the rejected magic.
        offset: usize,
    },
    /// One fixed-width header field is not hexadecimal ASCII.
    InvalidHex {
        /// Field offset containing the rejected byte.
        offset: usize,
    },
    /// Checked offset or alignment arithmetic overflowed.
    OffsetOverflow,
    /// The checksum field is nonzero in a checksum-free record.
    NonZeroChecksum {
        /// Header offset containing the rejected checksum field.
        offset: usize,
    },
    /// The declared name length is empty or exceeds Phoenix's bound.
    InvalidNameSize {
        /// Rejected size, including the required terminator.
        size: usize,
    },
    /// A name does not end with exactly one NUL byte.
    InvalidNameTermination {
        /// Header offset of the record containing the name.
        offset: usize,
    },
    /// A name is not valid UTF-8.
    InvalidNameEncoding {
        /// Header offset of the record containing the name.
        offset: usize,
    },
    /// A name could escape or ambiguously address the future root.
    InvalidPath {
        /// Header offset of the record containing the path.
        offset: usize,
        /// Rejected path property.
        reason: PathError,
    },
    /// Alignment or trailing padding contains a nonzero byte.
    NonZeroPadding {
        /// Offset of the first nonzero padding byte.
        offset: usize,
    },
    /// The initial subset does not support this file type.
    UnsupportedFileType {
        /// Header offset of the record.
        offset: usize,
        /// Complete mode value from the archive.
        mode: u32,
    },
    /// A directory carries a data payload.
    DirectoryHasData {
        /// Header offset of the directory record.
        offset: usize,
    },
    /// Hard-link reconstruction is outside the initial subset.
    UnsupportedLinkCount {
        /// Header offset of the record.
        offset: usize,
        /// Rejected link count.
        count: u32,
    },
    /// A canonical path appears more than once.
    DuplicatePath {
        /// Header offset of the later record.
        offset: usize,
    },
    /// More entries exist than the parser can validate without allocation.
    EntryLimitExceeded,
    /// The archive ended without the mandatory `TRAILER!!!` record.
    MissingTrailer,
    /// The trailer incorrectly declares a data payload.
    TrailerHasData {
        /// Header offset of the trailer.
        offset: usize,
    },
}

/// Supported object type in the initial initramfs subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitramfsEntryKind {
    /// A regular file with an inline byte payload.
    RegularFile,
    /// A directory with no payload.
    Directory,
}

/// One fully validated borrowed initramfs entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InitramfsEntry<'a> {
    path: &'a str,
    data: &'a [u8],
    mode: u32,
    kind: InitramfsEntryKind,
}

impl<'a> InitramfsEntry<'a> {
    /// Return the canonical path relative to the future root.
    pub const fn path(self) -> &'a str {
        self.path
    }

    /// Return the inline data payload.
    pub const fn data(self) -> &'a [u8] {
        self.data
    }

    /// Return the complete `newc` mode value.
    pub const fn mode(self) -> u32 {
        self.mode
    }

    /// Return the validated object kind.
    pub const fn kind(self) -> InitramfsEntryKind {
        self.kind
    }

    /// Return only the Unix permission bits.
    pub const fn permissions(self) -> u16 {
        (self.mode & 0o777) as u16
    }
}

/// Lookup failure against a fully validated archive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitramfsLookupError {
    /// No entry has the requested exact canonical path.
    NotFound,
    /// The requested entry is not a regular file.
    NotARegularFile,
    /// The requested regular file has no execute permission bit.
    NotExecutable,
}

/// One complete, checksum-free, uncompressed `newc` archive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Initramfs<'a> {
    bytes: &'a [u8],
    entry_count: usize,
}

impl<'a> Initramfs<'a> {
    /// Validate the complete archive before exposing any entry.
    ///
    /// Phoenix initially accepts one uncompressed `070701` archive with a
    /// mandatory trailer, zero padding, canonical UTF-8 paths, regular files,
    /// directories, and no hard links.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, InitramfsError> {
        let mut paths = [None; MAX_ENTRIES];
        let mut entry_count = 0;
        let mut offset = 0;

        loop {
            if offset == bytes.len() {
                return Err(InitramfsError::MissingTrailer);
            }
            let record = parse_record(bytes, offset)?;
            match record {
                ParsedRecord::Entry { entry, next } => {
                    if entry_count == MAX_ENTRIES {
                        return Err(InitramfsError::EntryLimitExceeded);
                    }
                    if paths[..entry_count]
                        .iter()
                        .any(|path| *path == Some(entry.path()))
                    {
                        return Err(InitramfsError::DuplicatePath { offset });
                    }
                    paths[entry_count] = Some(entry.path());
                    entry_count += 1;
                    offset = next;
                }
                ParsedRecord::Trailer { next } => {
                    validate_zero_padding(bytes, next, bytes.len())?;
                    return Ok(Self { bytes, entry_count });
                }
            }
        }
    }

    /// Return the number of non-trailer entries.
    pub const fn len(self) -> usize {
        self.entry_count
    }

    /// Return whether the archive has no non-trailer entries.
    pub const fn is_empty(self) -> bool {
        self.entry_count == 0
    }

    /// Iterate in archive order over already validated entries.
    pub const fn entries(self) -> InitramfsEntries<'a> {
        InitramfsEntries {
            bytes: self.bytes,
            offset: 0,
            remaining: self.entry_count,
        }
    }

    /// Find one entry by exact canonical path.
    pub fn find(self, path: &str) -> Option<InitramfsEntry<'a>> {
        self.entries().find(|entry| entry.path() == path)
    }

    /// Return one regular file by exact canonical path.
    pub fn regular_file(self, path: &str) -> Result<&'a [u8], InitramfsLookupError> {
        let entry = self.find(path).ok_or(InitramfsLookupError::NotFound)?;
        if entry.kind() != InitramfsEntryKind::RegularFile {
            return Err(InitramfsLookupError::NotARegularFile);
        }
        Ok(entry.data())
    }

    /// Return one executable regular file by exact canonical path.
    pub fn executable_file(self, path: &str) -> Result<&'a [u8], InitramfsLookupError> {
        let entry = self.find(path).ok_or(InitramfsLookupError::NotFound)?;
        if entry.kind() != InitramfsEntryKind::RegularFile {
            return Err(InitramfsLookupError::NotARegularFile);
        }
        if entry.permissions() & 0o111 == 0 {
            return Err(InitramfsLookupError::NotExecutable);
        }
        Ok(entry.data())
    }
}

/// Iterator over entries in a validated archive.
pub struct InitramfsEntries<'a> {
    bytes: &'a [u8],
    offset: usize,
    remaining: usize,
}

impl<'a> Iterator for InitramfsEntries<'a> {
    type Item = InitramfsEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let ParsedRecord::Entry { entry, next } =
            parse_record(self.bytes, self.offset).expect("validated initramfs entry")
        else {
            unreachable!("trailer cannot occur before validated entry count")
        };
        self.offset = next;
        self.remaining -= 1;
        Some(entry)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for InitramfsEntries<'_> {}

enum ParsedRecord<'a> {
    Entry {
        entry: InitramfsEntry<'a>,
        next: usize,
    },
    Trailer {
        next: usize,
    },
}

fn parse_record(bytes: &[u8], offset: usize) -> Result<ParsedRecord<'_>, InitramfsError> {
    let header_end = offset
        .checked_add(HEADER_SIZE)
        .ok_or(InitramfsError::OffsetOverflow)?;
    let header = bytes
        .get(offset..header_end)
        .ok_or(InitramfsError::Truncated { offset })?;
    if header.get(..MAGIC.len()) != Some(MAGIC) {
        return Err(InitramfsError::UnsupportedMagic { offset });
    }

    let mut fields = [0_u32; 13];
    for (index, field) in fields.iter_mut().enumerate() {
        *field = parse_field(header, offset, index)?;
    }
    let mode = fields[1];
    let link_count = fields[4];
    let file_size = fields[6] as usize;
    let name_size = fields[11] as usize;
    let checksum = fields[12];
    if checksum != 0 {
        return Err(InitramfsError::NonZeroChecksum {
            offset: offset + 102,
        });
    }
    if name_size == 0 || name_size > MAX_PATH_BYTES {
        return Err(InitramfsError::InvalidNameSize { size: name_size });
    }

    let name_end = header_end
        .checked_add(name_size)
        .ok_or(InitramfsError::OffsetOverflow)?;
    let encoded_name = bytes
        .get(header_end..name_end)
        .ok_or(InitramfsError::Truncated { offset: header_end })?;
    if encoded_name.last() != Some(&0) || encoded_name[..name_size - 1].contains(&0) {
        return Err(InitramfsError::InvalidNameTermination { offset });
    }
    let name_bytes = &encoded_name[..name_size - 1];

    let data_start = align_four(name_end)?;
    validate_zero_padding(bytes, name_end, data_start)?;
    let data_end = data_start
        .checked_add(file_size)
        .ok_or(InitramfsError::OffsetOverflow)?;
    let data = bytes
        .get(data_start..data_end)
        .ok_or(InitramfsError::Truncated { offset: data_start })?;
    let next = align_four(data_end)?;
    validate_zero_padding(bytes, data_end, next)?;

    if name_bytes == TRAILER {
        if file_size != 0 {
            return Err(InitramfsError::TrailerHasData { offset });
        }
        return Ok(ParsedRecord::Trailer { next });
    }

    let path =
        str::from_utf8(name_bytes).map_err(|_| InitramfsError::InvalidNameEncoding { offset })?;
    validate_canonical_path(path)
        .map_err(|reason| InitramfsError::InvalidPath { offset, reason })?;

    let kind = match mode & FILE_TYPE_MASK {
        REGULAR_FILE => {
            if link_count != 1 {
                return Err(InitramfsError::UnsupportedLinkCount {
                    offset,
                    count: link_count,
                });
            }
            InitramfsEntryKind::RegularFile
        }
        DIRECTORY => {
            if file_size != 0 {
                return Err(InitramfsError::DirectoryHasData { offset });
            }
            if link_count == 0 {
                return Err(InitramfsError::UnsupportedLinkCount {
                    offset,
                    count: link_count,
                });
            }
            InitramfsEntryKind::Directory
        }
        _ => return Err(InitramfsError::UnsupportedFileType { offset, mode }),
    };

    Ok(ParsedRecord::Entry {
        entry: InitramfsEntry {
            path,
            data,
            mode,
            kind,
        },
        next,
    })
}

fn parse_field(header: &[u8], record_offset: usize, index: usize) -> Result<u32, InitramfsError> {
    let start = MAGIC.len() + index * FIELD_SIZE;
    let mut value = 0_u32;
    for (byte_index, byte) in header[start..start + FIELD_SIZE]
        .iter()
        .copied()
        .enumerate()
    {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a' + 10),
            b'A'..=b'F' => u32::from(byte - b'A' + 10),
            _ => {
                return Err(InitramfsError::InvalidHex {
                    offset: record_offset + start + byte_index,
                });
            }
        };
        value = (value << 4) | digit;
    }
    Ok(value)
}

fn align_four(value: usize) -> Result<usize, InitramfsError> {
    value
        .checked_add(3)
        .map(|value| value & !3)
        .ok_or(InitramfsError::OffsetOverflow)
}

fn validate_zero_padding(bytes: &[u8], start: usize, end: usize) -> Result<(), InitramfsError> {
    let padding = bytes
        .get(start..end)
        .ok_or(InitramfsError::Truncated { offset: start })?;
    if let Some(index) = padding.iter().position(|byte| *byte != 0) {
        return Err(InitramfsError::NonZeroPadding {
            offset: start + index,
        });
    }
    Ok(())
}

/// Validate one canonical path relative to the initramfs root.
pub fn validate_canonical_path(path: &str) -> Result<(), PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.starts_with('/') {
        return Err(PathError::Absolute);
    }
    if path.ends_with('/') {
        return Err(PathError::TrailingSlash);
    }
    if path.as_bytes().contains(&0) {
        return Err(PathError::InteriorNul);
    }
    for component in path.split('/') {
        match component {
            "" => return Err(PathError::EmptyComponent),
            "." => return Err(PathError::CurrentDirectory),
            ".." => return Err(PathError::ParentDirectory),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::{
        DIRECTORY, Initramfs, InitramfsEntryKind, InitramfsError, InitramfsLookupError, MAGIC,
        PathError, REGULAR_FILE,
    };
    use std::format;
    use std::vec::Vec;

    fn push_field(output: &mut Vec<u8>, value: u32) {
        output.extend_from_slice(format!("{value:08x}").as_bytes());
    }

    fn set_field(output: &mut [u8], record_offset: usize, index: usize, value: u32) {
        let encoded = format!("{value:08x}");
        let start = record_offset + 6 + index * 8;
        output[start..start + 8].copy_from_slice(encoded.as_bytes());
    }

    fn push_record(output: &mut Vec<u8>, path: &[u8], data: &[u8], mode: u32, links: u32) {
        output.extend_from_slice(MAGIC);
        for value in [
            1,
            mode,
            0,
            0,
            links,
            0,
            data.len() as u32,
            0,
            0,
            0,
            0,
            (path.len() + 1) as u32,
            0,
        ] {
            push_field(output, value);
        }
        output.extend_from_slice(path);
        output.push(0);
        while !output.len().is_multiple_of(4) {
            output.push(0);
        }
        output.extend_from_slice(data);
        while !output.len().is_multiple_of(4) {
            output.push(0);
        }
    }

    fn archive(entries: &[(&[u8], &[u8], u32, u32)]) -> Vec<u8> {
        let mut output = Vec::new();
        for (path, data, mode, links) in entries {
            push_record(&mut output, path, data, *mode, *links);
        }
        push_record(&mut output, b"TRAILER!!!", &[], 0, 1);
        output
    }

    #[test]
    fn validates_and_looks_up_regular_files_and_directories() {
        let bytes = archive(&[
            (b"bin", &[], DIRECTORY | 0o755, 2),
            (b"bin/init", b"ELF", REGULAR_FILE | 0o755, 1),
        ]);
        let parsed = Initramfs::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.len(), 2);
        assert!(!parsed.is_empty());
        let entries: Vec<_> = parsed.entries().collect();
        assert_eq!(entries[0].kind(), InitramfsEntryKind::Directory);
        assert_eq!(entries[0].permissions(), 0o755);
        assert_eq!(entries[1].path(), "bin/init");
        assert_eq!(parsed.regular_file("bin/init"), Ok(&b"ELF"[..]));
        assert_eq!(parsed.executable_file("bin/init"), Ok(&b"ELF"[..]));
        assert_eq!(
            parsed.regular_file("bin"),
            Err(InitramfsLookupError::NotARegularFile)
        );
        assert_eq!(
            parsed.regular_file("missing"),
            Err(InitramfsLookupError::NotFound)
        );

        let bytes = archive(&[(b"data", b"x", REGULAR_FILE | 0o644, 1)]);
        assert_eq!(
            Initramfs::from_bytes(&bytes)
                .unwrap()
                .executable_file("data"),
            Err(InitramfsLookupError::NotExecutable)
        );
    }

    #[test]
    fn rejects_truncation_bad_magic_hex_and_checksum() {
        assert_eq!(
            Initramfs::from_bytes(&[]),
            Err(InitramfsError::MissingTrailer)
        );

        let mut bad_magic = archive(&[]);
        bad_magic[0] = b'1';
        assert_eq!(
            Initramfs::from_bytes(&bad_magic),
            Err(InitramfsError::UnsupportedMagic { offset: 0 })
        );

        let mut bad_hex = archive(&[]);
        bad_hex[6] = b'g';
        assert_eq!(
            Initramfs::from_bytes(&bad_hex),
            Err(InitramfsError::InvalidHex { offset: 6 })
        );

        let mut checksum = archive(&[]);
        checksum[109] = b'1';
        assert_eq!(
            Initramfs::from_bytes(&checksum),
            Err(InitramfsError::NonZeroChecksum { offset: 102 })
        );
    }

    #[test]
    fn rejects_bad_names_and_unsafe_paths() {
        for (path, reason) in [
            (&b""[..], PathError::Empty),
            (&b"/init"[..], PathError::Absolute),
            (&b"bin/"[..], PathError::TrailingSlash),
            (&b"bin//init"[..], PathError::EmptyComponent),
            (&b"./init"[..], PathError::CurrentDirectory),
            (&b"../init"[..], PathError::ParentDirectory),
        ] {
            let bytes = archive(&[(path, b"x", REGULAR_FILE, 1)]);
            assert_eq!(
                Initramfs::from_bytes(&bytes),
                Err(InitramfsError::InvalidPath { offset: 0, reason })
            );
        }

        let bytes = archive(&[(b"bad\xff", b"x", REGULAR_FILE, 1)]);
        assert_eq!(
            Initramfs::from_bytes(&bytes),
            Err(InitramfsError::InvalidNameEncoding { offset: 0 })
        );

        let mut zero_name = archive(&[]);
        set_field(&mut zero_name, 0, 11, 0);
        assert_eq!(
            Initramfs::from_bytes(&zero_name),
            Err(InitramfsError::InvalidNameSize { size: 0 })
        );

        let mut unterminated = archive(&[(b"init", b"x", REGULAR_FILE, 1)]);
        unterminated[114] = b'x';
        assert_eq!(
            Initramfs::from_bytes(&unterminated),
            Err(InitramfsError::InvalidNameTermination { offset: 0 })
        );
    }

    #[test]
    fn rejects_nonzero_padding_and_invalid_payload_rules() {
        let mut name_padding = archive(&[(b"ab", b"", REGULAR_FILE, 1)]);
        name_padding[113] = 1;
        assert_eq!(
            Initramfs::from_bytes(&name_padding),
            Err(InitramfsError::NonZeroPadding { offset: 113 })
        );

        let mut data_padding = archive(&[(b"a", b"x", REGULAR_FILE, 1)]);
        data_padding[113] = 1;
        assert_eq!(
            Initramfs::from_bytes(&data_padding),
            Err(InitramfsError::NonZeroPadding { offset: 113 })
        );

        let directory_data = archive(&[(b"dir", b"x", DIRECTORY, 1)]);
        assert_eq!(
            Initramfs::from_bytes(&directory_data),
            Err(InitramfsError::DirectoryHasData { offset: 0 })
        );

        let hard_link = archive(&[(b"init", b"x", REGULAR_FILE, 2)]);
        assert_eq!(
            Initramfs::from_bytes(&hard_link),
            Err(InitramfsError::UnsupportedLinkCount {
                offset: 0,
                count: 2
            })
        );

        let unsupported = archive(&[(b"link", b"target", 0o120_777, 1)]);
        assert_eq!(
            Initramfs::from_bytes(&unsupported),
            Err(InitramfsError::UnsupportedFileType {
                offset: 0,
                mode: 0o120_777
            })
        );
    }

    #[test]
    fn rejects_duplicates_missing_trailer_and_trailer_payload() {
        let duplicate = archive(&[
            (b"init", b"one", REGULAR_FILE, 1),
            (b"init", b"two", REGULAR_FILE, 1),
        ]);
        let second_offset = {
            let first_size = 110 + 5;
            let first_data = (first_size + 3) & !3;
            (first_data + 3 + 3) & !3
        };
        assert_eq!(
            Initramfs::from_bytes(&duplicate),
            Err(InitramfsError::DuplicatePath {
                offset: second_offset
            })
        );

        let mut missing = Vec::new();
        push_record(&mut missing, b"init", b"x", REGULAR_FILE, 1);
        assert_eq!(
            Initramfs::from_bytes(&missing),
            Err(InitramfsError::MissingTrailer)
        );

        let trailer_data = archive(&[(b"TRAILER!!!", b"x", 0, 1)]);
        assert_eq!(
            Initramfs::from_bytes(&trailer_data),
            Err(InitramfsError::TrailerHasData { offset: 0 })
        );
    }

    #[test]
    fn rejects_nonzero_bytes_after_trailer() {
        let mut bytes = archive(&[]);
        bytes.extend_from_slice(&[0, 0, 1]);
        assert_eq!(
            Initramfs::from_bytes(&bytes),
            Err(InitramfsError::NonZeroPadding {
                offset: bytes.len() - 1
            })
        );
    }

    #[test]
    fn rejects_declared_payload_beyond_input_and_entry_limit() {
        let mut truncated_data = archive(&[(b"init", b"x", REGULAR_FILE, 1)]);
        set_field(&mut truncated_data, 0, 6, u32::MAX);
        assert_eq!(
            Initramfs::from_bytes(&truncated_data),
            Err(InitramfsError::Truncated { offset: 116 })
        );

        let mut too_many = Vec::new();
        for index in 0..=super::MAX_ENTRIES {
            let path = format!("file-{index}");
            push_record(&mut too_many, path.as_bytes(), &[], REGULAR_FILE, 1);
        }
        push_record(&mut too_many, b"TRAILER!!!", &[], 0, 1);
        assert_eq!(
            Initramfs::from_bytes(&too_many),
            Err(InitramfsError::EntryLimitExceeded)
        );
    }
}
