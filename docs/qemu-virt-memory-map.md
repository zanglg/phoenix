# QEMU Virt Platform Memory Map

This document describes the memory layout of the QEMU `virt` machine for AArch64,
which is the primary development platform for Phoenix kernel.

## Overview

The QEMU `virt` machine is a generic virtual platform that provides a standard
ARM system with common peripherals. It's widely used for kernel development
and testing.

## Physical Memory Map

| Address Range          | Size      | Description                          | Type       |
|------------------------|-----------|--------------------------------------|------------|
| `0x0000_0000` - `0x03FF_FFFF` | 64MB     | Flash memory (NOR flash)             | Flash      |
| `0x0400_0000` - `0x07FF_FFFF` | 64MB     | Reserved                             | -          |
| `0x0800_0000` - `0x08FF_FFFF` | 16MB     | GIC (Generic Interrupt Controller)   | MMIO       |
| `0x0900_0000` - `0x09FF_FFFF` | 16MB     | UART (PL011)                         | MMIO       |
| `0x0A00_0000` - `0x0BFF_FFFF` | 32MB     | RTC (PL031)                          | MMIO       |
| `0x0C00_0000` - `0x0DFF_FFFF` | 32MB     | GPIO                                 | MMIO       |
| `0x0E00_0000` - `0x0EFF_FFFF` | 16MB     | Secure UART (if present)             | MMIO       |
| `0x0F00_0000` - `0x0FFF_FFFF` | 16MB     | SMMU (System MMU)                    | MMIO       |
| `0x1000_0000` - `0x1FFF_FFFF` | 256MB    | PCI Express ECAM                     | MMIO       |
| `0x2000_0000` - `0x2FFF_FFFF` | 256MB    | PCI Express MMIO                     | MMIO       |
| `0x3000_0000` - `0x3EFF_FFFF` | 240MB    | Additional device space              | MMIO       |
| `0x3F00_0000` - `0x3FFF_FFFF` | 16MB     | PCI Express PIO                      | MMIO       |
| `0x4000_0000` - `0x7FFF_FFFF` | **1GB**  | **RAM (DRAM)**                       | RAM        |
| `0x8000_0000` - `0xFFFF_FFFF` | 2GB      | High PCI/device space                | MMIO       |

## Key Memory Regions

### RAM (DRAM)
- **Base**: `0x4000_0000` (1GB)
- **Size**: `0x4000_0000` (1GB)
- **Purpose**: Main system memory
- **Notes**: This is where the kernel and applications are loaded

### UART (PL011)
- **Base**: `0x0900_0000`
- **Size**: `0x0100_0000` (16MB)
- **Purpose**: Serial console output
- **Virtual Address**: `0xffffff8009000000` (when mapped to kernel space)

### GIC (Generic Interrupt Controller)
- **Base**: `0x0800_0000`
- **Size**: `0x0100_0000` (16MB)
- **Purpose**: Interrupt controller for ARM systems
- **Virtual Address**: `0xffffff8008000000` (when mapped to kernel space)

### Flash Memory
- **Base**: `0x0000_0000`
- **Size**: `0x0400_0000` (64MB)
- **Purpose**: Boot firmware and device tree
- **Notes**: Typically contains U-Boot or other bootloader

## Kernel Virtual Address Space

Phoenix kernel uses a high-half virtual address layout similar to Linux:

```
0xffffff8000000000 +------------------+
                   | Kernel text      |
                   | Kernel data      |
                   | Kernel BSS       |
                   | Kernel stack     |
                   |                  |
                   | Kernel mappings  |
                   |                  |
                   | VMALLOC area     |
                   |                  |
                   | User space       |
0x0000000000000000 +------------------+
```

### Key Virtual Addresses
- **Kernel Virtual Base**: `0xffffff8000000000`
- **Kernel Load Offset**: `0x80000`
- **Kernel Virtual Start**: `0xffffff8000080000`
- **UART Virtual Address**: `0xffffff8009000000`
- **GIC Virtual Address**: `0xffffff8008000000`

## Address Translation

Physical addresses are translated to kernel virtual addresses using:

```
virtual_address = physical_address + KERNEL_VIRTUAL_BASE
```

Where `KERNEL_VIRTUAL_BASE = 0xffffff8000000000`.

### Examples:
- RAM physical `0x4000_0000` → virtual `0xffffff8040000000`
- UART physical `0x0900_0000` → virtual `0xffffff8009000000`
- GIC physical `0x0800_0000` → virtual `0xffffff8008000000`

## Memory Attributes

Different memory regions have different cache attributes:

| Memory Type | MAIR Index | Attribute | Description |
|-------------|------------|-----------|-------------|
| Normal      | 0          | 0xFF      | Write-Back Cacheable |
| Device-nGnRE| 1          | 0x04      | Device, no Gathering, no Reordering, Early Write Ack |
| Device-nGnRnE| 2         | 0x00      | Device, strict ordering |
| Normal-NC   | 3          | 0x44      | Non-Cacheable |

## Boot Process Memory Usage

1. **Bootloader**: Loads kernel at `0x4008_0000` (RAM + 0x80000)
2. **Kernel Entry**: Jumps to `0xffffff8000080000` (virtual)
3. **Early MMU**: Maps RAM and devices using 1GB blocks
4. **Memblock**: Initializes with RAM region `0x4000_0000` - `0x7FFF_FFFF`
5. **Kernel Reservation**: Reserves kernel image memory

## References

1. QEMU Documentation: `docs/system/arm/virt.rst`
2. ARM Architecture Reference Manual
3. Linux ARM64 Memory Layout
4. U-Boot QEMU Virt Platform Support