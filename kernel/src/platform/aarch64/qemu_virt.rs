//! Early devices on QEMU's AArch64 `virt` machine.

use core::ptr::{read_volatile, write_volatile};

use crate::console::ByteSink;

const PL011_BASE: usize = 0x0900_0000;
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
