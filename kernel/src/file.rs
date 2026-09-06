//! Fixed-capacity read-only file descriptions backed by a validated initramfs.

use crate::initramfs::{Initramfs, InitramfsEntryKind, PathError, validate_canonical_path};

/// First descriptor assigned to an opened file.
pub const FIRST_OPEN_FILE_DESCRIPTOR: u64 = 3;

/// Failure while opening or operating on a read-only initramfs file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileError {
    /// The requested path is not canonical relative to the root.
    InvalidPath(PathError),
    /// No archive entry has the requested path.
    NotFound,
    /// The requested path names a directory.
    IsDirectory,
    /// The archive mode does not grant any read bit.
    PermissionDenied,
    /// Every fixed-capacity open-file slot is occupied.
    TooManyOpenFiles,
    /// The raw descriptor does not identify an open regular file.
    BadDescriptor,
    /// A slot can no longer issue a fresh generation identifier.
    GenerationExhausted,
    /// The file changed or advanced after a read window was obtained.
    StaleRead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OpenFile<'a> {
    data: &'a [u8],
    offset: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileSlot<'a> {
    generation: u64,
    file: Option<OpenFile<'a>>,
}

impl FileSlot<'_> {
    const EMPTY: Self = Self {
        generation: 0,
        file: None,
    };
}

/// Immutable bytes proposed by a read without advancing its file offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadWindow<'a> {
    descriptor: u64,
    generation: u64,
    offset: usize,
    bytes: &'a [u8],
}

impl<'a> ReadWindow<'a> {
    /// Return bytes available for the current bounded read.
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Return the descriptor whose offset may be committed.
    pub const fn descriptor(self) -> u64 {
        self.descriptor
    }

    /// Return the offset at which this read began.
    pub const fn offset(self) -> usize {
        self.offset
    }
}

/// One process-local table of read-only files borrowed from an initramfs.
///
/// Descriptors zero through two are reserved for standard streams. This table
/// allocates the lowest available descriptor starting at three. Reads use a
/// window/commit protocol so a failed user-memory copy cannot silently advance
/// the open-file offset.
#[derive(Debug, Eq, PartialEq)]
pub struct ReadOnlyFileTable<'a, const OPEN_FILES: usize> {
    archive: Initramfs<'a>,
    slots: [FileSlot<'a>; OPEN_FILES],
}

impl<'a, const OPEN_FILES: usize> ReadOnlyFileTable<'a, OPEN_FILES> {
    /// Create an empty descriptor table over a fully validated archive.
    pub const fn new(archive: Initramfs<'a>) -> Self {
        Self {
            archive,
            slots: [FileSlot::EMPTY; OPEN_FILES],
        }
    }

    /// Open one readable regular file and return the lowest available descriptor.
    pub fn open(&mut self, path: &str) -> Result<u64, FileError> {
        validate_canonical_path(path).map_err(FileError::InvalidPath)?;
        let entry = self.archive.find(path).ok_or(FileError::NotFound)?;
        if entry.kind() == InitramfsEntryKind::Directory {
            return Err(FileError::IsDirectory);
        }
        if entry.permissions() & 0o444 == 0 {
            return Err(FileError::PermissionDenied);
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.file.is_none())
            .ok_or(FileError::TooManyOpenFiles)?;
        let generation = self.slots[index]
            .generation
            .checked_add(1)
            .ok_or(FileError::GenerationExhausted)?;
        self.slots[index] = FileSlot {
            generation,
            file: Some(OpenFile {
                data: entry.data(),
                offset: 0,
            }),
        };
        Ok(FIRST_OPEN_FILE_DESCRIPTOR + index as u64)
    }

    /// Propose at most `maximum` bytes without advancing the file offset.
    pub fn read_window(
        &self,
        descriptor: u64,
        maximum: usize,
    ) -> Result<ReadWindow<'a>, FileError> {
        let slot = self.slot(descriptor)?;
        let file = slot.file.ok_or(FileError::BadDescriptor)?;
        let end = file.offset.saturating_add(maximum).min(file.data.len());
        Ok(ReadWindow {
            descriptor,
            generation: slot.generation,
            offset: file.offset,
            bytes: &file.data[file.offset..end],
        })
    }

    /// Commit the complete proposed read after its destination copy succeeds.
    pub fn commit_read(&mut self, window: ReadWindow<'a>) -> Result<(), FileError> {
        let slot = self.slot_mut(window.descriptor)?;
        let file = slot.file.as_mut().ok_or(FileError::BadDescriptor)?;
        if slot.generation != window.generation || file.offset != window.offset {
            return Err(FileError::StaleRead);
        }
        file.offset = file
            .offset
            .checked_add(window.bytes.len())
            .filter(|offset| *offset <= file.data.len())
            .ok_or(FileError::StaleRead)?;
        Ok(())
    }

    /// Close one opened file. Standard-stream descriptors are not owned here.
    pub fn close(&mut self, descriptor: u64) -> Result<(), FileError> {
        let slot = self.slot_mut(descriptor)?;
        if slot.file.take().is_none() {
            return Err(FileError::BadDescriptor);
        }
        Ok(())
    }

    fn descriptor_index(descriptor: u64) -> Result<usize, FileError> {
        let relative = descriptor
            .checked_sub(FIRST_OPEN_FILE_DESCRIPTOR)
            .ok_or(FileError::BadDescriptor)?;
        usize::try_from(relative).map_err(|_| FileError::BadDescriptor)
    }

    fn slot(&self, descriptor: u64) -> Result<&FileSlot<'a>, FileError> {
        let index = Self::descriptor_index(descriptor)?;
        self.slots.get(index).ok_or(FileError::BadDescriptor)
    }

    fn slot_mut(&mut self, descriptor: u64) -> Result<&mut FileSlot<'a>, FileError> {
        let index = Self::descriptor_index(descriptor)?;
        self.slots.get_mut(index).ok_or(FileError::BadDescriptor)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::format;
    use std::vec::Vec;

    use super::{FIRST_OPEN_FILE_DESCRIPTOR, FileError, ReadOnlyFileTable};
    use crate::initramfs::{Initramfs, PathError};

    #[test]
    fn opens_lowest_slot_and_tracks_independent_offsets() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut files = ReadOnlyFileTable::<2>::new(archive);
        let first = files.open("etc/motd").unwrap();
        let second = files.open("etc/motd").unwrap();
        assert_eq!(first, FIRST_OPEN_FILE_DESCRIPTOR);
        assert_eq!(second, FIRST_OPEN_FILE_DESCRIPTOR + 1);

        let window = files.read_window(first, 3).unwrap();
        assert_eq!(window.bytes(), b"pho");
        assert_eq!(files.read_window(first, 3).unwrap().bytes(), b"pho");
        files.commit_read(window).unwrap();
        assert_eq!(files.read_window(first, 8).unwrap().bytes(), b"enix");
        assert_eq!(files.read_window(second, 8).unwrap().bytes(), b"phoenix");
    }

    #[test]
    fn reports_eof_and_rejects_stale_read_windows() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut files = ReadOnlyFileTable::<1>::new(archive);
        let descriptor = files.open("etc/motd").unwrap();
        let stale = files.read_window(descriptor, 2).unwrap();
        let complete = files.read_window(descriptor, usize::MAX).unwrap();
        files.commit_read(complete).unwrap();
        assert!(files.read_window(descriptor, 1).unwrap().bytes().is_empty());
        assert_eq!(files.commit_read(stale), Err(FileError::StaleRead));
    }

    #[test]
    fn close_invalidates_windows_and_recycles_descriptors_safely() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut files = ReadOnlyFileTable::<1>::new(archive);
        let descriptor = files.open("etc/motd").unwrap();
        let old = files.read_window(descriptor, 2).unwrap();
        files.close(descriptor).unwrap();
        assert_eq!(files.close(descriptor), Err(FileError::BadDescriptor));
        assert_eq!(files.open("etc/motd"), Ok(descriptor));
        assert_eq!(files.commit_read(old), Err(FileError::StaleRead));
    }

    #[test]
    fn rejects_invalid_objects_permissions_capacity_and_descriptors() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut files = ReadOnlyFileTable::<1>::new(archive);

        assert_eq!(
            files.open("../etc/motd"),
            Err(FileError::InvalidPath(PathError::ParentDirectory))
        );
        assert_eq!(
            files.open("etc/\0motd"),
            Err(FileError::InvalidPath(PathError::InteriorNul))
        );
        assert_eq!(files.open("missing"), Err(FileError::NotFound));
        assert_eq!(files.open("etc"), Err(FileError::IsDirectory));
        assert_eq!(files.open("secret"), Err(FileError::PermissionDenied));
        assert_eq!(files.read_window(1, 1), Err(FileError::BadDescriptor));
        assert_eq!(files.close(u64::MAX), Err(FileError::BadDescriptor));
        files.open("etc/motd").unwrap();
        assert_eq!(files.open("etc/motd"), Err(FileError::TooManyOpenFiles));
    }

    #[test]
    fn refuses_to_wrap_a_slot_generation() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut files = ReadOnlyFileTable::<1>::new(archive);
        files.slots[0].generation = u64::MAX;

        assert_eq!(files.open("etc/motd"), Err(FileError::GenerationExhausted));
        assert!(files.slots[0].file.is_none());
    }

    fn archive() -> Vec<u8> {
        let mut bytes = Vec::new();
        append_record(&mut bytes, 1, "etc", 0o040_755, &[]);
        append_record(&mut bytes, 2, "etc/motd", 0o100_444, b"phoenix");
        append_record(&mut bytes, 3, "secret", 0o100_000, b"hidden");
        append_record(&mut bytes, 0, "TRAILER!!!", 0, &[]);
        bytes
    }

    fn append_record(bytes: &mut Vec<u8>, inode: u32, path: &str, mode: u32, data: &[u8]) {
        bytes.extend_from_slice(b"070701");
        for value in [
            inode,
            mode,
            0,
            0,
            1,
            0,
            data.len() as u32,
            0,
            0,
            0,
            0,
            (path.len() + 1) as u32,
            0,
        ] {
            bytes.extend_from_slice(format!("{value:08x}").as_bytes());
        }
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        bytes.extend_from_slice(data);
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
    }
}
