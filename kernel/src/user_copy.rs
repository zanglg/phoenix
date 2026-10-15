//! Checked copying from resident user pages without dereferencing user virtual pointers.

use crate::memory::{PAGE_SIZE, PageFrame};
use crate::process_image::{PopulatedProcessImage, ResidentUserPage};
use crate::user::{UserAddr, UserAddressError, UserRange};

/// Physical-memory access required by a checked user-copy operation.
pub trait UserMemoryReader {
    /// Backend-specific read failure.
    type Error;

    /// Copy bytes from one owned physical frame into a kernel buffer.
    fn read_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), Self::Error>;
}

/// Physical-memory access required by a checked copy into user memory.
pub trait UserMemoryWriter {
    /// Backend-specific write failure.
    type Error;

    /// Copy bytes into one owned physical frame.
    fn write_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        input: &[u8],
    ) -> Result<(), Self::Error>;
}

/// Failure while copying a complete user virtual range into kernel memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserCopyError<E> {
    /// A raw ABI pointer cannot be represented by this kernel build.
    PointerTooWide {
        /// Rejected raw pointer value.
        address: u64,
    },
    /// The requested start or end lies outside the current user-address contract.
    Address(UserAddressError),
    /// No resident page owns part of the requested range.
    Unmapped {
        /// First requested byte without a resident page.
        address: usize,
    },
    /// A resident page does not permit the requested direction of access.
    PermissionDenied {
        /// First requested byte in the inaccessible page.
        address: usize,
    },
    /// The physical-memory backend rejected a prevalidated page access.
    Backend {
        /// First user virtual byte in the failed access.
        address: usize,
        /// Backend-specific error.
        error: E,
    },
}

/// Copy one complete range from an owned, populated process image.
///
/// The entire virtual range and every required readable resident page are
/// checked before the first backend read. A backend failure can leave `output`
/// partially modified; callers must not publish it unless this function
/// returns success. A zero-length request validates pointer width and the user
/// region but does not require a mapping.
pub fn copy_from_user<M: UserMemoryReader, const MAPPINGS: usize, const PAGES: usize>(
    image: &PopulatedProcessImage<MAPPINGS, PAGES>,
    memory: &mut M,
    raw_start: u64,
    output: &mut [u8],
) -> Result<(), UserCopyError<M::Error>> {
    let start_address = usize::try_from(raw_start)
        .map_err(|_| UserCopyError::PointerTooWide { address: raw_start })?;
    let start = UserAddr::new(start_address).map_err(UserCopyError::Address)?;
    if output.is_empty() {
        return Ok(());
    }
    let range = UserRange::from_start_len(start, output.len()).map_err(UserCopyError::Address)?;

    walk_range(image, range, Access::Read, |_, _, _| {
        Ok::<(), UserCopyError<M::Error>>(())
    })?;
    let mut destination_offset = 0;
    walk_range(image, range, Access::Read, |page, page_offset, length| {
        let user_address = page.virtual_start().as_usize() + page_offset;
        let destination_end = destination_offset + length;
        memory
            .read_frame(
                page.frame(),
                page_offset,
                &mut output[destination_offset..destination_end],
            )
            .map_err(|error| UserCopyError::Backend {
                address: user_address,
                error,
            })?;
        destination_offset = destination_end;
        Ok(())
    })
}

/// Copy one complete kernel byte slice into an owned, populated user range.
///
/// The complete range and every required writable resident page are checked
/// before the first backend write. A backend failure can leave user memory
/// partially modified; callers must report that failure and must not advance
/// higher-level file offsets. A zero-length request has the same pointer
/// validation semantics as [`copy_from_user`].
pub fn copy_to_user<M: UserMemoryWriter, const MAPPINGS: usize, const PAGES: usize>(
    image: &PopulatedProcessImage<MAPPINGS, PAGES>,
    memory: &mut M,
    raw_start: u64,
    input: &[u8],
) -> Result<(), UserCopyError<M::Error>> {
    let start_address = usize::try_from(raw_start)
        .map_err(|_| UserCopyError::PointerTooWide { address: raw_start })?;
    let start = UserAddr::new(start_address).map_err(UserCopyError::Address)?;
    if input.is_empty() {
        return Ok(());
    }
    let range = UserRange::from_start_len(start, input.len()).map_err(UserCopyError::Address)?;

    walk_range(image, range, Access::Write, |_, _, _| {
        Ok::<(), UserCopyError<M::Error>>(())
    })?;
    let mut source_offset = 0;
    walk_range(image, range, Access::Write, |page, page_offset, length| {
        let user_address = page.virtual_start().as_usize() + page_offset;
        let source_end = source_offset + length;
        memory
            .write_frame(page.frame(), page_offset, &input[source_offset..source_end])
            .map_err(|error| UserCopyError::Backend {
                address: user_address,
                error,
            })?;
        source_offset = source_end;
        Ok(())
    })
}

#[derive(Clone, Copy)]
enum Access {
    Read,
    Write,
}

fn walk_range<const MAPPINGS: usize, const PAGES: usize, E>(
    image: &PopulatedProcessImage<MAPPINGS, PAGES>,
    range: UserRange,
    access: Access,
    mut visit: impl FnMut(ResidentUserPage, usize, usize) -> Result<(), UserCopyError<E>>,
) -> Result<(), UserCopyError<E>> {
    let mut current = range.start().as_usize();
    while current < range.end_exclusive() {
        let page_start = current & !(PAGE_SIZE - 1);
        let page = image
            .pages()
            .find(|page| page.virtual_start().as_usize() == page_start)
            .ok_or(UserCopyError::Unmapped { address: current })?;
        let permitted = match access {
            Access::Read => page.permissions().readable(),
            Access::Write => page.permissions().writable(),
        };
        if !permitted {
            return Err(UserCopyError::PermissionDenied { address: current });
        }
        let page_offset = current - page_start;
        let length = (PAGE_SIZE - page_offset).min(range.end_exclusive() - current);
        visit(page, page_offset, length)?;
        current += length;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec;
    use std::vec::Vec;

    use super::{UserCopyError, UserMemoryReader, UserMemoryWriter, copy_from_user, copy_to_user};
    use crate::elf::ElfImage;
    use crate::memory::{AddressRange, MemoryMap, PAGE_SIZE, PageFrame, PhysAddr};
    use crate::process_image::{ProcessImageMemory, ProcessImagePlan};
    use crate::user::{USER_VIRTUAL_LIMIT, UserAddr, UserAddressError, UserStackLayout};

    #[test]
    fn copies_across_resident_pages_after_prevalidation() {
        let (image, mut memory) = populated_fixture();
        let mut output = [0_u8; 6];

        copy_from_user(
            &image,
            &mut memory,
            (0x40_0000 + PAGE_SIZE - 2) as u64,
            &mut output,
        )
        .unwrap();

        assert_eq!(output, [0xa5; 6]);
        assert_eq!(memory.reads, 2);
    }

    #[test]
    fn rejects_unmapped_and_out_of_range_requests_before_reading() {
        let (image, mut memory) = populated_fixture();
        let mut output = [0xcc_u8; 4];

        assert_eq!(
            copy_from_user(&image, &mut memory, 0x60_0000, &mut output),
            Err(UserCopyError::Unmapped { address: 0x60_0000 })
        );
        assert_eq!(memory.reads, 0);
        assert_eq!(output, [0xcc; 4]);

        assert_eq!(
            copy_from_user(
                &image,
                &mut memory,
                (USER_VIRTUAL_LIMIT - 2) as u64,
                &mut output,
            ),
            Err(UserCopyError::Address(
                UserAddressError::OutsideUserRegion {
                    address: USER_VIRTUAL_LIMIT + 2
                }
            ))
        );
        assert_eq!(memory.reads, 0);
    }

    #[test]
    fn crossing_into_a_gap_is_all_or_nothing_before_backend_reads() {
        let (image, mut memory) = populated_fixture();
        let mut output = [0xcc_u8; 4];

        assert_eq!(
            copy_from_user(
                &image,
                &mut memory,
                (0x40_0000 + 2 * PAGE_SIZE - 2) as u64,
                &mut output,
            ),
            Err(UserCopyError::Unmapped {
                address: 0x40_0000 + 2 * PAGE_SIZE
            })
        );
        assert_eq!(memory.reads, 0);
        assert_eq!(output, [0xcc; 4]);
    }

    #[test]
    fn copies_across_writable_user_pages_after_prevalidation() {
        let (image, mut memory) = populated_fixture();
        let input = [1, 2, 3, 4, 5, 6];

        copy_to_user(&image, &mut memory, 0x7f_effe, &input).unwrap();

        assert_eq!(&memory.pages[2][PAGE_SIZE - 2..], &[1, 2]);
        assert_eq!(&memory.pages[3][..4], &[3, 4, 5, 6]);
        assert_eq!(memory.writes, 2);
    }

    #[test]
    fn rejects_nonwritable_and_unmapped_destinations_before_writing() {
        let (image, mut memory) = populated_fixture();
        let input = [1, 2, 3, 4];

        assert_eq!(
            copy_to_user(&image, &mut memory, 0x40_0000, &input),
            Err(UserCopyError::PermissionDenied { address: 0x40_0000 })
        );
        assert_eq!(
            copy_to_user(&image, &mut memory, 0x60_0000, &input),
            Err(UserCopyError::Unmapped { address: 0x60_0000 })
        );
        assert_eq!(memory.writes, 0);
    }

    #[test]
    fn reports_user_write_backend_failure() {
        let (image, mut memory) = populated_fixture();
        memory.fail_write = true;
        let input = [1, 2, 3, 4];

        assert_eq!(
            copy_to_user(&image, &mut memory, 0x7f_e000, &input),
            Err(UserCopyError::Backend {
                address: 0x7f_e000,
                error: TestMemoryError::Injected
            })
        );
        assert_eq!(memory.writes, 1);
    }

    #[test]
    fn zero_length_validates_address_but_needs_no_mapping() {
        let (image, mut memory) = populated_fixture();
        assert_eq!(copy_from_user(&image, &mut memory, 0, &mut []), Ok(()));
        assert_eq!(copy_to_user(&image, &mut memory, 0, &[]), Ok(()));
        assert_eq!(
            copy_from_user(&image, &mut memory, u64::MAX, &mut []),
            Err(UserCopyError::Address(
                UserAddressError::OutsideUserRegion {
                    address: usize::MAX
                }
            ))
        );
        assert_eq!(memory.reads, 0);
        assert_eq!(memory.writes, 0);
    }

    #[test]
    fn reports_backend_failure_without_publishing_the_buffer() {
        let (image, mut memory) = populated_fixture();
        memory.fail_read = true;
        let mut output = [0xcc_u8; 4];

        assert_eq!(
            copy_from_user(&image, &mut memory, 0x40_0000, &mut output),
            Err(UserCopyError::Backend {
                address: 0x40_0000,
                error: TestMemoryError::Injected
            })
        );
        assert_eq!(memory.reads, 1);
        assert_eq!(output, [0xcc; 4]);
    }

    fn populated_fixture() -> (
        crate::process_image::PopulatedProcessImage<2, 4>,
        TestMemory,
    ) {
        let bytes = elf_fixture();
        let image = ElfImage::parse(&bytes).unwrap();
        let stack = UserStackLayout::new(UserAddr::new(0x80_0000).unwrap(), 2, 1).unwrap();
        let plan = ProcessImagePlan::<2, 4>::new(image, stack).unwrap();
        let mut map = MemoryMap::<1>::new();
        map.add_usable(
            AddressRange::new(PhysAddr::new(0x10_0000), PhysAddr::new(0x10_4000)).unwrap(),
        )
        .unwrap();
        let mut allocator = map.into_allocator();
        let allocated = plan.allocate(&mut allocator).unwrap();
        let mut memory = TestMemory::new();
        let populated = allocated.populate(&mut memory).unwrap();
        (populated, memory)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestMemoryError {
        UnknownFrame,
        InvalidRange,
        Injected,
    }

    struct TestMemory {
        pages: Vec<Vec<u8>>,
        reads: usize,
        writes: usize,
        fail_read: bool,
        fail_write: bool,
    }

    impl TestMemory {
        fn new() -> Self {
            Self {
                pages: vec![vec![0xcc; PAGE_SIZE]; 4],
                reads: 0,
                writes: 0,
                fail_read: false,
                fail_write: false,
            }
        }

        fn index(&self, frame: PageFrame) -> Result<usize, TestMemoryError> {
            let offset = frame
                .start_address()
                .as_usize()
                .checked_sub(0x10_0000)
                .ok_or(TestMemoryError::UnknownFrame)?;
            if !offset.is_multiple_of(PAGE_SIZE) || offset / PAGE_SIZE >= self.pages.len() {
                return Err(TestMemoryError::UnknownFrame);
            }
            Ok(offset / PAGE_SIZE)
        }

        fn bounds(
            &self,
            frame: PageFrame,
            offset: usize,
            length: usize,
        ) -> Result<(usize, core::ops::Range<usize>), TestMemoryError> {
            let index = self.index(frame)?;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= PAGE_SIZE)
                .ok_or(TestMemoryError::InvalidRange)?;
            Ok((index, offset..end))
        }
    }

    impl ProcessImageMemory for TestMemory {
        type Error = TestMemoryError;

        fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
            let index = self.index(frame)?;
            self.pages[index].fill(0);
            Ok(())
        }

        fn write_frame(
            &mut self,
            frame: PageFrame,
            offset: usize,
            bytes: &[u8],
        ) -> Result<(), Self::Error> {
            let (index, range) = self.bounds(frame, offset, bytes.len())?;
            self.pages[index][range].copy_from_slice(bytes);
            Ok(())
        }
    }

    impl UserMemoryReader for TestMemory {
        type Error = TestMemoryError;

        fn read_frame(
            &mut self,
            frame: PageFrame,
            offset: usize,
            output: &mut [u8],
        ) -> Result<(), Self::Error> {
            self.reads += 1;
            if self.fail_read {
                return Err(TestMemoryError::Injected);
            }
            let (index, range) = self.bounds(frame, offset, output.len())?;
            output.copy_from_slice(&self.pages[index][range]);
            Ok(())
        }
    }

    impl UserMemoryWriter for TestMemory {
        type Error = TestMemoryError;

        fn write_frame(
            &mut self,
            frame: PageFrame,
            offset: usize,
            input: &[u8],
        ) -> Result<(), Self::Error> {
            self.writes += 1;
            if self.fail_write {
                return Err(TestMemoryError::Injected);
            }
            let (index, range) = self.bounds(frame, offset, input.len())?;
            self.pages[index][range].copy_from_slice(input);
            Ok(())
        }
    }

    fn elf_fixture() -> Vec<u8> {
        let file_offset = PAGE_SIZE;
        let file_size = PAGE_SIZE + 4;
        let mut bytes = vec![0_u8; file_offset + file_size];
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
        put_u16(&mut bytes, 56, 1);
        put_u32(&mut bytes, 64, 1);
        put_u32(&mut bytes, 68, 5);
        put_u64(&mut bytes, 72, file_offset as u64);
        put_u64(&mut bytes, 80, 0x40_0000);
        put_u64(&mut bytes, 88, 0x40_0000);
        put_u64(&mut bytes, 96, file_size as u64);
        put_u64(&mut bytes, 104, file_size as u64);
        put_u64(&mut bytes, 112, PAGE_SIZE as u64);
        bytes[file_offset..].fill(0xa5);
        bytes
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
