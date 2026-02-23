//! Serial output driver for PL011 UART.
//!
//! This module provides simple serial output functionality using the PL011 UART
//! on QEMU Virt platform.

use crate::arch::aarch64::address;

/// PL011 UART registers offsets.
mod registers {
    /// Data register (read/write).
    pub const DR: u64 = 0x00;
    /// Flag register (read-only).
    pub const FR: u64 = 0x18;
    /// Transmit FIFO full flag.
    pub const FR_TXFF: u32 = 1 << 5;
}

/// Serial output driver.
pub struct Serial {
    base: u64,
}

impl Serial {
    /// Create a new serial driver instance.
    ///
    /// # Arguments
    /// * `base` - Virtual base address of UART
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    /// Get the default serial instance for QEMU Virt platform.
    #[allow(dead_code)]
    pub fn default() -> Self {
        Self::new(address::kernel::VIRTUAL_BASE + address::virt::UART_BASE)
    }

    /// Write a single byte to serial port.
    ///
    /// # Arguments
    /// * `byte` - Byte to write
    pub fn write_byte(&self, byte: u8) {
        // Wait until transmit FIFO is not full
        while self.is_tx_full() {}

        // Write byte to data register
        unsafe {
            core::ptr::write_volatile((self.base + registers::DR) as *mut u8, byte);
        }
    }

    /// Write a string to serial port.
    ///
    /// # Arguments
    /// * `s` - String slice to write
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
    }

    /// Write a byte slice to serial port.
    ///
    /// # Arguments
    /// * `bytes` - Byte slice to write
    pub fn write_bytes(&self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_byte(byte);
        }
    }

    /// Check if transmit FIFO is full.
    fn is_tx_full(&self) -> bool {
        unsafe {
            let flags = core::ptr::read_volatile((self.base + registers::FR) as *const u32);
            (flags & registers::FR_TXFF) != 0
        }
    }
}

/// Global serial instance for kernel use.
static SERIAL: Serial = Serial::new(address::kernel::VIRTUAL_BASE + address::virt::UART_BASE);

/// Write a byte to serial port using global instance.
///
/// # Arguments
/// * `byte` - Byte to write
pub fn write_byte(byte: u8) {
    SERIAL.write_byte(byte);
}

/// Write a string to serial port using global instance.
///
/// # Arguments
/// * `s` - String slice to write
pub fn write_str(s: &str) {
    SERIAL.write_str(s);
}

/// Write a byte slice to serial port using global instance.
///
/// # Arguments
/// * `bytes` - Byte slice to write
pub fn write_bytes(bytes: &[u8]) {
    SERIAL.write_bytes(bytes);
}

/// Initialize serial output.
///
/// Currently a no-op as PL011 UART is typically pre-initialized by firmware.
pub fn init() {
    // PL011 UART is usually initialized by firmware
    // Additional initialization could be added here if needed
}
