//! Kernel boot and initialization module.
//!
//! This module handles kernel boot process, memory initialization, and
//! early system setup.

use crate::arch::aarch64::address;
use crate::mm::memblock;

/// Kernel boot information.
pub struct BootInfo {
    /// Physical address of kernel image start.
    pub kernel_phys_start: u64,
    /// Physical address of kernel image end.
    #[allow(dead_code)]
    pub kernel_phys_end: u64,
    /// Size of kernel image in bytes.
    pub kernel_size: u64,
}

impl BootInfo {
    /// Create boot info from kernel virtual addresses.
    ///
    /// # Arguments
    /// * `kernel_virt_start` - Virtual start address of kernel
    /// * `kernel_virt_end` - Virtual end address of kernel
    pub fn from_virtual(kernel_virt_start: u64, kernel_virt_end: u64) -> Self {
        let kernel_phys_start = address::translation::virt_to_phys(kernel_virt_start);
        let kernel_phys_end = address::translation::virt_to_phys(kernel_virt_end);
        let kernel_size = kernel_phys_end - kernel_phys_start;

        Self {
            kernel_phys_start,
            kernel_phys_end,
            kernel_size,
        }
    }
}

/// Initialize memory management subsystem.
///
/// # Arguments
/// * `boot_info` - Kernel boot information
///
/// # Returns
/// Result indicating success or error
pub fn init_memory(boot_info: &BootInfo) -> Result<(), &'static str> {
    // Get RAM region for QEMU Virt platform
    let (ram_base, ram_size) = address::regions::ram();

    // Initialize memblock with available RAM
    memblock::init(ram_base, ram_size)?;

    // Reserve kernel image memory
    memblock::reserve(boot_info.kernel_phys_start, boot_info.kernel_size)?;

    Ok(())
}

/// Test memory allocation functionality.
///
/// # Returns
/// Result with allocated address or error
pub fn test_memory_allocation() -> Result<u64, &'static str> {
    // Test allocation of a 4KB page with 4KB alignment
    memblock::alloc(address::kernel::PAGE_SIZE, address::kernel::PAGE_SIZE)
}

/// Print kernel memory information.
///
/// # Arguments
/// * `boot_info` - Kernel boot information
pub fn print_memory_info(_boot_info: &BootInfo) {
    use crate::arch::aarch64::serial;

    serial::write_str("Kernel physical memory: [");
    // TODO: Implement proper hex formatting
    serial::write_str("]\n");
}

/// Early kernel initialization.
///
/// This function performs essential initialization steps that must happen
/// before any other kernel functionality.
pub fn early_init() {
    use crate::arch::aarch64::serial;

    // Initialize serial output
    serial::init();
    serial::write_str("Phoenix kernel booting...\n");
}

/// Main kernel initialization.
///
/// This function performs all kernel initialization after early setup.
///
/// # Arguments
/// * `kernel_virt_start` - Virtual start address of kernel
/// * `kernel_virt_end` - Virtual end address of kernel
pub fn kernel_init(kernel_virt_start: u64, kernel_virt_end: u64) {
    use crate::arch::aarch64::serial;

    let boot_info = BootInfo::from_virtual(kernel_virt_start, kernel_virt_end);

    // Initialize memory management
    serial::write_str("Initializing memory management...\n");
    if let Err(e) = init_memory(&boot_info) {
        serial::write_str("Failed to initialize memory: ");
        serial::write_bytes(e.as_bytes());
        serial::write_str("\n");
        loop {}
    }

    // Test memory allocation
    serial::write_str("Testing memory allocation...\n");
    match test_memory_allocation() {
        Ok(addr) => {
            serial::write_str("Allocated page at ");
            // Simple hex output
            let hex_digits = b"0123456789ABCDEF";
            for shift in (0..16).rev() {
                let nibble = (addr >> (shift * 4)) & 0xF;
                serial::write_byte(hex_digits[nibble as usize]);
            }
            serial::write_str("\n");
        }
        Err(e) => {
            serial::write_str("Allocation failed: ");
            serial::write_bytes(e.as_bytes());
            serial::write_str("\n");
        }
    }

    // Print memory information
    print_memory_info(&boot_info);

    serial::write_str("Kernel initialization complete!\n");
    serial::write_str("Hello, world!\n");
}
