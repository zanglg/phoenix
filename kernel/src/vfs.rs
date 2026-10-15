//! Bounded native root: read-only initramfs plus writable `tmp/` regular files.
//! The owning kernel serializes calls. Reads commit offsets only after user copy.
use crate::abi::Errno;
use crate::initramfs::{Initramfs, InitramfsEntryKind, validate_canonical_path};

/// Maximum path bytes, excluding any terminator.
pub const PATH_LIMIT: usize = 128;
#[derive(Clone, Copy)]
struct Node<const BYTES: usize> {
    path: [u8; PATH_LIMIT],
    path_len: usize,
    bytes: [u8; BYTES],
    len: usize,
}
impl<const BYTES: usize> Node<BYTES> {
    const EMPTY: Self = Self {
        path: [0; PATH_LIMIT],
        path_len: 0,
        bytes: [0; BYTES],
        len: 0,
    };
}
/// Fixed-capacity writable file storage; no implicit eviction or partial writes.
pub struct RamFs<const FILES: usize, const BYTES: usize> {
    nodes: [Node<BYTES>; FILES],
}
impl<const FILES: usize, const BYTES: usize> Default for RamFs<FILES, BYTES> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const FILES: usize, const BYTES: usize> RamFs<FILES, BYTES> {
    /// Create an empty writable mount.
    pub const fn new() -> Self {
        Self {
            nodes: [Node::EMPTY; FILES],
        }
    }
    fn find(&self, path: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| &node.path[..node.path_len] == path.as_bytes())
    }
}
#[derive(Clone, Copy)]
enum Object<'a> {
    Archive(&'a [u8]),
    Ram(usize),
}
#[derive(Clone, Copy)]
struct Open<'a> {
    object: Object<'a>,
    offset: usize,
    writable: bool,
}
#[derive(Clone, Copy)]
struct Slot<'a> {
    generation: u64,
    open: Option<Open<'a>>,
}
/// Process-owned descriptors; 0/1/2 remain console streams in the syscall layer.
pub struct FileTable<'a, const OPEN: usize> {
    slots: [Slot<'a>; OPEN],
}
/// Read transaction identity, valid only at the exact descriptor generation/offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadCommit {
    slot: usize,
    generation: u64,
    offset: usize,
    length: usize,
}

impl ReadCommit {
    /// Number of bytes proposed by the read.
    pub const fn len(self) -> usize {
        self.length
    }
    /// Whether this read reached EOF or requested zero bytes.
    pub const fn is_empty(self) -> bool {
        self.length == 0
    }
}
impl<const OPEN: usize> Default for FileTable<'_, OPEN> {
    fn default() -> Self {
        Self::new()
    }
}
impl<'a, const OPEN: usize> FileTable<'a, OPEN> {
    /// Construct a process-local table with no open regular files.
    pub const fn new() -> Self {
        Self {
            slots: [Slot {
                generation: 0,
                open: None,
            }; OPEN],
        }
    }
    /// Open an archive or tmp file. Mode 0 is read-only; mode 1 creates/opens
    /// a writable tmp file without truncating existing contents.
    pub fn open<const FILES: usize, const BYTES: usize>(
        &mut self,
        archive: Initramfs<'a>,
        ram: &mut RamFs<FILES, BYTES>,
        path: &str,
        mode: u64,
    ) -> Result<u64, Errno> {
        validate_canonical_path(path).map_err(|_| Errno::InvalidArgument)?;
        if path.len() > PATH_LIMIT || mode > 1 {
            return Err(Errno::InvalidArgument);
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.open.is_none())
            .ok_or(Errno::TooManyOpenFiles)?;
        let generation = self.slots[index]
            .generation
            .checked_add(1)
            .ok_or(Errno::TooManyOpenFiles)?;
        let object = if path.starts_with("tmp/") {
            let node = match ram.find(path) {
                Some(node) => node,
                None if mode == 1 => {
                    let index = ram
                        .nodes
                        .iter()
                        .position(|node| node.path_len == 0)
                        .ok_or(Errno::NoSpace)?;
                    ram.nodes[index].path[..path.len()].copy_from_slice(path.as_bytes());
                    ram.nodes[index].path_len = path.len();
                    index
                }
                None => return Err(Errno::NoSuchFile),
            };
            Object::Ram(node)
        } else {
            if mode != 0 {
                return Err(Errno::PermissionDenied);
            }
            let entry = archive.find(path).ok_or(Errno::NoSuchFile)?;
            if entry.kind() == InitramfsEntryKind::Directory {
                return Err(Errno::IsDirectory);
            }
            if entry.permissions() & 0o444 == 0 {
                return Err(Errno::PermissionDenied);
            }
            Object::Archive(entry.data())
        };
        self.slots[index] = Slot {
            generation,
            open: Some(Open {
                object,
                offset: 0,
                writable: mode == 1,
            }),
        };
        Ok(index as u64 + 3)
    }
    fn index(&self, fd: u64) -> Result<usize, Errno> {
        let index = fd
            .checked_sub(3)
            .and_then(|fd| usize::try_from(fd).ok())
            .ok_or(Errno::BadFileDescriptor)?;
        if index >= OPEN || self.slots[index].open.is_none() {
            return Err(Errno::BadFileDescriptor);
        }
        Ok(index)
    }
    /// Propose data into a kernel buffer without changing the descriptor offset.
    pub fn read<const FILES: usize, const BYTES: usize>(
        &self,
        ram: &RamFs<FILES, BYTES>,
        fd: u64,
        output: &mut [u8],
    ) -> Result<ReadCommit, Errno> {
        let index = self.index(fd)?;
        let slot = self.slots[index];
        let open = slot.open.unwrap();
        let bytes = match open.object {
            Object::Archive(bytes) => bytes,
            Object::Ram(node) => &ram.nodes[node].bytes[..ram.nodes[node].len],
        };
        let count = output.len().min(bytes.len().saturating_sub(open.offset));
        output[..count].copy_from_slice(&bytes[open.offset..open.offset + count]);
        Ok(ReadCommit {
            slot: index,
            generation: slot.generation,
            offset: open.offset,
            length: count,
        })
    }
    /// Advance an offset only after the complete proposed user copy succeeded.
    pub fn commit_read(&mut self, read: ReadCommit) -> Result<(), Errno> {
        let slot = self
            .slots
            .get_mut(read.slot)
            .ok_or(Errno::BadFileDescriptor)?;
        let open = slot.open.as_mut().ok_or(Errno::BadFileDescriptor)?;
        if slot.generation != read.generation || open.offset != read.offset {
            return Err(Errno::InputOutput);
        }
        open.offset = open
            .offset
            .checked_add(read.length)
            .ok_or(Errno::InputOutput)?;
        Ok(())
    }
    /// Commit a complete writable-file write, or leave bytes and offset intact.
    pub fn write<const FILES: usize, const BYTES: usize>(
        &mut self,
        ram: &mut RamFs<FILES, BYTES>,
        fd: u64,
        input: &[u8],
    ) -> Result<usize, Errno> {
        let index = self.index(fd)?;
        let open = self.slots[index].open.as_mut().unwrap();
        if !open.writable {
            return Err(Errno::BadFileDescriptor);
        }
        let Object::Ram(node) = open.object else {
            return Err(Errno::PermissionDenied);
        };
        let end = open
            .offset
            .checked_add(input.len())
            .filter(|end| *end <= BYTES)
            .ok_or(Errno::NoSpace)?;
        ram.nodes[node].bytes[open.offset..end].copy_from_slice(input);
        ram.nodes[node].len = ram.nodes[node].len.max(end);
        open.offset = end;
        Ok(input.len())
    }
    /// Release a descriptor; later opens receive a new generation.
    pub fn close(&mut self, fd: u64) -> Result<(), Errno> {
        let index = self.index(fd)?;
        self.slots[index].open = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{format, vec::Vec};

    fn archive() -> Vec<u8> {
        let mut bytes = Vec::new();
        for (path, mode, data) in [
            ("etc", 0o040755, &b""[..]),
            ("etc/motd", 0o100444, &b"hello"[..]),
            ("TRAILER!!!", 0, &b""[..]),
        ] {
            bytes.extend_from_slice(b"070701");
            for field in [
                1,
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
                path.len() as u32 + 1,
                0,
            ] {
                bytes.extend_from_slice(format!("{field:08x}").as_bytes());
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
        bytes
    }

    #[test]
    fn read_rollback_stale_commits_and_independent_offsets() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut ram = RamFs::<1, 8>::new();
        let mut files = FileTable::<2>::new();
        let a = files.open(archive, &mut ram, "etc/motd", 0).unwrap();
        let b = files.open(archive, &mut ram, "etc/motd", 0).unwrap();
        let mut output = [0; 2];
        let proposal = files.read(&ram, a, &mut output).unwrap();
        assert_eq!(&output, b"he");
        assert_eq!(files.read(&ram, a, &mut output).unwrap(), proposal);
        files.commit_read(proposal).unwrap();
        assert_eq!(files.commit_read(proposal), Err(Errno::InputOutput));
        let stale = files.read(&ram, a, &mut output).unwrap();
        assert_eq!(&output, b"ll");
        files.read(&ram, b, &mut output).unwrap();
        assert_eq!(&output, b"he");
        files.close(a).unwrap();
        assert_eq!(files.open(archive, &mut ram, "etc/motd", 0), Ok(a));
        assert_eq!(files.commit_read(stale), Err(Errno::InputOutput));
    }

    #[test]
    fn capacity_failure_preserves_files_bytes_offsets_and_recovery() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut ram = RamFs::<1, 4>::new();
        let mut files = FileTable::<1>::new();
        let fd = files.open(archive, &mut ram, "tmp/a", 1).unwrap();
        assert_eq!(
            files.open(archive, &mut ram, "tmp/b", 1),
            Err(Errno::TooManyOpenFiles)
        );
        assert!(ram.find("tmp/b").is_none());
        files.write(&mut ram, fd, b"abc").unwrap();
        assert_eq!(files.write(&mut ram, fd, b"XX"), Err(Errno::NoSpace));
        assert_eq!(files.write(&mut ram, fd, b"d"), Ok(1));
        files.close(fd).unwrap();
        assert_eq!(
            files.open(archive, &mut ram, "tmp/b", 1),
            Err(Errno::NoSpace)
        );
        let fd = files.open(archive, &mut ram, "tmp/a", 0).unwrap();
        let mut output = [0; 8];
        let read = files.read(&ram, fd, &mut output).unwrap();
        assert_eq!(&output[..read.len()], b"abcd");
        files.commit_read(read).unwrap();
        assert!(files.read(&ram, fd, &mut output).unwrap().is_empty());
        assert_eq!(
            files.write(&mut ram, fd, b"x"),
            Err(Errno::BadFileDescriptor)
        );
        files.close(fd).unwrap();
        assert_eq!(files.close(fd), Err(Errno::BadFileDescriptor));
    }

    #[test]
    fn path_modes_permissions_and_zero_capacity_are_explicit() {
        let bytes = archive();
        let archive = Initramfs::from_bytes(&bytes).unwrap();
        let mut ram = RamFs::<0, 0>::new();
        let mut files = FileTable::<1>::new();
        for (path, mode, expected) in [
            ("tmp/../escape", 1, Errno::InvalidArgument),
            ("etc/motd", 2, Errno::InvalidArgument),
            ("etc/motd", 1, Errno::PermissionDenied),
            ("etc", 0, Errno::IsDirectory),
            ("tmp/a", 0, Errno::NoSuchFile),
            ("tmp/a", 1, Errno::NoSpace),
        ] {
            assert_eq!(files.open(archive, &mut ram, path, mode), Err(expected));
        }
        let mut zero = FileTable::<0>::new();
        assert_eq!(
            zero.open(archive, &mut ram, "etc/motd", 0),
            Err(Errno::TooManyOpenFiles)
        );
        assert_eq!(files.open(archive, &mut ram, "etc/motd", 0), Ok(3));
    }
}
