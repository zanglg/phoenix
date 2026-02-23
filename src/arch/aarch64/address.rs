//! Memory address definitions for AArch64 QEMU Virt platform.
//!
//! This module provides centralized memory address definitions for the QEMU Virt
//! platform, following the standard memory layout used by QEMU's `virt` machine.

/// QEMU Virt platform memory layout constants.
pub mod virt {
    /// Base physical address of RAM (DRAM) in QEMU Virt platform.
    ///
    /// According to QEMU documentation, the `virt` machine places RAM at
    /// 0x4000_0000 (1GB) by default for AArch64.
    pub const RAM_BASE: u64 = 0x4000_0000;

    /// Default RAM size for QEMU Virt platform (1GB).
    pub const RAM_SIZE: u64 = 0x4000_0000;

    /// End address of RAM (exclusive).
    #[allow(dead_code)]
    pub const RAM_END: u64 = RAM_BASE + RAM_SIZE;

    /// PL011 UART (serial) base address.
    ///
    /// The PL011 UART is located at 0x0900_0000 in the memory map.
    /// When mapped to kernel virtual space, this becomes 0xffffff8009000000.
    pub const UART_BASE: u64 = 0x0900_0000;

    /// GIC (Generic Interrupt Controller) base address.
    #[allow(dead_code)]
    pub const GIC_BASE: u64 = 0x0800_0000;

    /// PCI Express ECAM (Enhanced Configuration Access Mechanism) base.
    #[allow(dead_code)]
    pub const PCIE_ECAM_BASE: u64 = 0x4010_0000;

    /// PCI Express MMIO base.
    #[allow(dead_code)]
    pub const PCIE_MMIO_BASE: u64 = 0x4020_0000;

    /// PCI Express PIO (Programmed I/O) base.
    #[allow(dead_code)]
    pub const PCIE_PIO_BASE: u64 = 0x3eff_0000;

    /// Flash memory base address.
    #[allow(dead_code)]
    pub const FLASH_BASE: u64 = 0x0000_0000;

    /// Flash memory size.
    #[allow(dead_code)]
    pub const FLASH_SIZE: u64 = 0x0400_0000;

    /// Device memory region base.
    #[allow(dead_code)]
    pub const DEVICE_BASE: u64 = 0x0800_0000;

    /// Device memory region size.
    #[allow(dead_code)]
    pub const DEVICE_SIZE: u64 = 0x7800_0000;
}

/// Kernel virtual address space layout.
pub mod kernel {
    /// Kernel virtual base address (high-half).
    ///
    /// This is the base of the kernel's virtual address space, using the
    /// standard Linux high-half layout for 39-bit VA space.
    pub const VIRTUAL_BASE: u64 = 0xffffff8000000000;

    /// Kernel load offset within virtual address space.
    ///
    /// Typical value for AArch64 kernels is 0x80000.
    #[allow(dead_code)]
    pub const LOAD_OFFSET: u64 = 0x80000;

    /// Kernel virtual start address (VIRTUAL_BASE + LOAD_OFFSET).
    #[allow(dead_code)]
    pub const VIRTUAL_START: u64 = VIRTUAL_BASE + LOAD_OFFSET;

    /// Page size (4KB).
    pub const PAGE_SIZE: u64 = 0x1000;

    /// Text section alignment requirement for AArch64.
    #[allow(dead_code)]
    pub const TEXT_SECTION_ALIGN: u64 = 0x10000;

    /// Default kernel stack size (64KB).
    #[allow(dead_code)]
    pub const STACK_SIZE: u64 = 0x10000;
}

/// Memory type attributes for MAIR_EL1.
pub mod mair {
    /// Normal memory, Inner Write-Back Cacheable, Outer Write-Back Cacheable.
    #[allow(dead_code)]
    pub const MT_NORMAL: u64 = 0xFF;

    /// Normal memory, Non-Cacheable.
    #[allow(dead_code)]
    pub const MT_NORMAL_NC: u64 = 0x44;

    /// Device memory, nGnRnE (no Gathering, no Reordering, no Early Write Ack).
    #[allow(dead_code)]
    pub const MT_DEVICE_NGNRNE: u64 = 0x00;

    /// Device memory, nGnRE (no Gathering, no Reordering, Early Write Ack).
    #[allow(dead_code)]
    pub const MT_DEVICE_NGNRE: u64 = 0x04;
}

/// Helper functions for address translation.
pub mod translation {
    use super::{kernel, virt};

    /// Convert physical address to kernel virtual address.
    ///
    /// # Arguments
    /// * `phys` - Physical address to convert
    ///
    /// # Returns
    /// Kernel virtual address
    #[allow(dead_code)]
    pub fn phys_to_virt(phys: u64) -> u64 {
        phys + kernel::VIRTUAL_BASE
    }

    /// Convert kernel virtual address to physical address.
    ///
    /// # Arguments
    /// * `virt` - Kernel virtual address to convert
    ///
    /// # Returns
    /// Physical address
    pub fn virt_to_phys(virt: u64) -> u64 {
        virt - kernel::VIRTUAL_BASE
    }

    /// Get UART virtual address for kernel use.
    ///
    /// # Returns
    /// Virtual address of UART for MMIO access
    #[allow(dead_code)]
    pub fn uart_virt() -> u64 {
        phys_to_virt(virt::UART_BASE)
    }

    /// Get GIC virtual address for kernel use.
    ///
    /// # Returns
    /// Virtual address of GIC for MMIO access
    #[allow(dead_code)]
    pub fn gic_virt() -> u64 {
        phys_to_virt(virt::GIC_BASE)
    }
}

/// Memory region definitions for memblock initialization.
pub mod regions {
    use super::virt;

    /// Get the default RAM region for QEMU Virt platform.
    ///
    /// # Returns
    /// Tuple of (base, size) for RAM
    pub fn ram() -> (u64, u64) {
        (virt::RAM_BASE, virt::RAM_SIZE)
    }

    /// Get the device memory region for QEMU Virt platform.
    ///
    /// # Returns
    /// Tuple of (base, size) for device memory
    #[allow(dead_code)]
    pub fn device() -> (u64, u64) {
        (virt::DEVICE_BASE, virt::DEVICE_SIZE)
    }

    /// Get the flash memory region for QEMU Virt platform.
    ///
    /// # Returns
    /// Tuple of (base, size) for flash memory
    #[allow(dead_code)]
    pub fn flash() -> (u64, u64) {
        (virt::FLASH_BASE, virt::FLASH_SIZE)
    }
}
