//! Early devices on QEMU's AArch64 `virt` machine.

use core::ptr::{self, read_volatile, write_volatile};

use crate::arch::aarch64::paging::Descriptor;
use crate::arch::aarch64::user_page_table::TranslationTableMemory;
use crate::console::ByteSink;
use crate::memory::{PAGE_SIZE, PageFrame};
use crate::process_image::ProcessImageMemory;

const PL011_BASE: usize = 0xffff_ff80_0900_0000;
const BOOTSTRAP_RAM_PHYSICAL_START: usize = 0x4000_0000;
const BOOTSTRAP_RAM_PHYSICAL_END: usize = 0x8000_0000;
const KERNEL_VIRTUAL_OFFSET: usize = 0xffff_ff80_0000_0000;
const UART_DR: usize = 0x00;
const UART_FR: usize = 0x18;
const UART_FR_BUSY: u32 = 1 << 3;
const UART_FR_TXFF: u32 = 1 << 5;

/// Polled PL011 transmitter used before the driver subsystem exists.
pub struct EarlyPl011 {
    base: usize,
}

impl EarlyPl011 {
    /// Construct the early console for QEMU `virt`.
    pub const fn new() -> Self {
        Self { base: PL011_BASE }
    }

    /// Wait until every submitted byte has left the transmitter.
    pub fn flush(&mut self) {
        while self.flags() & UART_FR_BUSY != 0 {
            core::hint::spin_loop();
        }
    }

    fn flags(&self) -> u32 {
        let register = (self.base + UART_FR) as *const u32;
        // SAFETY: the platform contract maps the QEMU `virt` PL011 register
        // page at `PL011_BASE` as device memory before this method is called.
        unsafe { read_volatile(register) }
    }
}

impl Default for EarlyPl011 {
    fn default() -> Self {
        Self::new()
    }
}

impl ByteSink for EarlyPl011 {
    fn write_byte(&mut self, byte: u8) {
        while self.flags() & UART_FR_TXFF != 0 {
            core::hint::spin_loop();
        }

        let register = (self.base + UART_DR) as *mut u32;
        // SAFETY: the platform contract maps the QEMU `virt` PL011 register
        // page at `PL011_BASE` as device memory before this method is called.
        unsafe {
            write_volatile(register, u32::from(byte));
        }
    }
}

/// Failure while accessing a private RAM frame through the temporary TTBR1 alias.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapMemoryError {
    /// The frame is not completely covered by the temporary 1 GiB RAM block.
    OutsideTemporaryRam {
        /// Rejected physical frame start.
        address: usize,
    },
    /// A requested byte range extends beyond one 4 KiB frame.
    AccessOutOfBounds {
        /// Byte offset within the frame.
        offset: usize,
        /// Requested byte count.
        length: usize,
    },
    /// Physical-to-virtual address conversion overflowed.
    AddressOverflow,
}

/// Checked access to private RAM through the bootstrap higher-half block mapping.
///
/// This handle is deliberately platform- and bootstrap-specific. It must be
/// replaced by the final physical-memory mapping before the coarse TTBR1 block
/// is retired.
pub struct BootstrapPhysicalMemory {
    _private: (),
}

impl BootstrapPhysicalMemory {
    /// Assert that the QEMU `virt` bootstrap higher-half RAM alias is active.
    ///
    /// # Safety
    ///
    /// The caller must ensure the current EL1 translation regime maps physical
    /// `0x40000000..0x80000000` at `KERNEL_VIRTUAL_OFFSET` as writable normal
    /// memory for the entire lifetime of this handle. Every supplied frame must
    /// be privately owned, and no concurrent CPU may access it while a trait
    /// method mutates it.
    pub const unsafe fn assume_bootstrap_mapping() -> Self {
        Self { _private: () }
    }

    fn translated_address(
        frame: PageFrame,
        offset: usize,
        length: usize,
    ) -> Result<usize, BootstrapMemoryError> {
        let frame_start = frame.start_address().as_usize();
        if !(BOOTSTRAP_RAM_PHYSICAL_START..=BOOTSTRAP_RAM_PHYSICAL_END - PAGE_SIZE)
            .contains(&frame_start)
        {
            return Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: frame_start,
            });
        }
        let end = offset
            .checked_add(length)
            .ok_or(BootstrapMemoryError::AddressOverflow)?;
        if offset > PAGE_SIZE || end > PAGE_SIZE {
            return Err(BootstrapMemoryError::AccessOutOfBounds { offset, length });
        }
        frame_start
            .checked_add(offset)
            .and_then(|physical| physical.checked_add(KERNEL_VIRTUAL_OFFSET))
            .ok_or(BootstrapMemoryError::AddressOverflow)
    }
}

impl ProcessImageMemory for BootstrapPhysicalMemory {
    type Error = BootstrapMemoryError;

    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        let destination = Self::translated_address(frame, 0, PAGE_SIZE)? as *mut u8;
        // SAFETY: construction guarantees the complete higher-half frame is
        // writable and privately owned. The checked range is exactly one page.
        unsafe {
            ptr::write_bytes(destination, 0, PAGE_SIZE);
        }
        Ok(())
    }

    fn write_frame(
        &mut self,
        frame: PageFrame,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), Self::Error> {
        let destination = Self::translated_address(frame, offset, bytes.len())? as *mut u8;
        if bytes.is_empty() {
            return Ok(());
        }
        // SAFETY: construction guarantees unique writable destination memory;
        // the checked range lies within one frame. `ptr::copy` also permits a
        // source slice that aliases the destination during bootstrap loading.
        unsafe {
            ptr::copy(bytes.as_ptr(), destination, bytes.len());
        }
        Ok(())
    }
}

impl TranslationTableMemory for BootstrapPhysicalMemory {
    type Error = BootstrapMemoryError;

    fn clear_table(&mut self, frame: PageFrame) -> Result<(), Self::Error> {
        ProcessImageMemory::clear_frame(self, frame)
    }

    fn write_descriptor(
        &mut self,
        frame: PageFrame,
        index: usize,
        descriptor: Descriptor,
    ) -> Result<(), Self::Error> {
        let offset = index
            .checked_mul(core::mem::size_of::<u64>())
            .ok_or(BootstrapMemoryError::AddressOverflow)?;
        let destination = Self::translated_address(frame, offset, size_of::<u64>())? as *mut u64;
        // SAFETY: the frame is private writable normal memory, its base is page
        // aligned, and the checked descriptor offset preserves u64 alignment.
        unsafe {
            ptr::write(destination, descriptor.raw());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BOOTSTRAP_RAM_PHYSICAL_END, BOOTSTRAP_RAM_PHYSICAL_START, BootstrapMemoryError,
        BootstrapPhysicalMemory, KERNEL_VIRTUAL_OFFSET,
    };
    use crate::memory::{PAGE_SIZE, PageFrame, PhysAddr};

    #[test]
    fn translates_only_complete_frames_in_the_bootstrap_ram_window() {
        let first = frame(BOOTSTRAP_RAM_PHYSICAL_START);
        let last = frame(BOOTSTRAP_RAM_PHYSICAL_END - PAGE_SIZE);
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(first, 0, PAGE_SIZE),
            Ok(KERNEL_VIRTUAL_OFFSET + BOOTSTRAP_RAM_PHYSICAL_START)
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(last, PAGE_SIZE - 1, 1),
            Ok(KERNEL_VIRTUAL_OFFSET + BOOTSTRAP_RAM_PHYSICAL_END - 1)
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame(0x3fff_f000), 0, 1),
            Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: 0x3fff_f000
            })
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame(BOOTSTRAP_RAM_PHYSICAL_END), 0, 1),
            Err(BootstrapMemoryError::OutsideTemporaryRam {
                address: BOOTSTRAP_RAM_PHYSICAL_END
            })
        );
    }

    #[test]
    fn rejects_cross_page_and_overflowing_accesses() {
        let frame = frame(BOOTSTRAP_RAM_PHYSICAL_START);
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame, PAGE_SIZE - 1, 2),
            Err(BootstrapMemoryError::AccessOutOfBounds {
                offset: PAGE_SIZE - 1,
                length: 2
            })
        );
        assert_eq!(
            BootstrapPhysicalMemory::translated_address(frame, usize::MAX, 2),
            Err(BootstrapMemoryError::AddressOverflow)
        );
    }

    fn frame(address: usize) -> PageFrame {
        PageFrame::from_start(PhysAddr::new(address)).unwrap()
    }
}
