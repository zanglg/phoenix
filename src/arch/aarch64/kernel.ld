/******************************************************************************
 *                     AArch64 Kernel Linker Script                            *
 *                    High Virtual Address Layout                            *
 ******************************************************************************/

/* Entry point of the kernel */
ENTRY(_start)

/* ------------------------------------------------------------
 * Virtual address and memory configuration
 * ------------------------------------------------------------ */

/* High virtual address for kernel space (39-bit VA space) */
KERNEL_VIRTUAL_BASE      = 0xffffff8000000000;

/* Load offset within the virtual address space
 * This represents the kernel's load offset from the base
 * Typical value for AArch64 kernels is 0x80000
 */
KERNEL_LOAD_OFFSET       = 0x80000;

/* Final virtual address where kernel sections start */
KERNEL_VIRTUAL_START     = KERNEL_VIRTUAL_BASE + KERNEL_LOAD_OFFSET;

/* Page size configuration
 * 0x1000  = 4KB  (default)
 * 0x4000  = 16KB
 * 0x10000 = 64KB
 */
PAGE_SIZE                = 0x1000;

/* Text section alignment requirement for AArch64 */
TEXT_SECTION_ALIGN       = 0x10000;

/* Stack size configuration
 * Default stack size is 64KB (0x10000)
 */
STACK_SIZE               = 0x10000;

/* ------------------------------------------------------------
 * Output sections definition
 * ------------------------------------------------------------ */
SECTIONS
{
    /* Set location counter to kernel virtual start address
     * This includes the load offset from the virtual base
     */
    . = KERNEL_VIRTUAL_START;

    /* Start of kernel image in virtual address space */
    __kernel_virtual_start = .;

    /* --------------------------------------------------------
     * Text section - executable code
     * -------------------------------------------------------- */
    .text : ALIGN(TEXT_SECTION_ALIGN)
    {
        /* Boot code must be placed first */
        *(.text.boot)

        /* All other executable code */
        *(.text .text.*)
    }

    /* --------------------------------------------------------
     * Read-only data section
     * -------------------------------------------------------- */
    .rodata : ALIGN(PAGE_SIZE)
    {
        *(.rodata .rodata.*)
    }

    /* --------------------------------------------------------
     * Data section - initialized global/static variables
     * -------------------------------------------------------- */
    .data : ALIGN(PAGE_SIZE)
    {
        *(.data .data.*)
    }

    /* --------------------------------------------------------
     * BSS section - uninitialized global/static variables
     * -------------------------------------------------------- */
    .bss : ALIGN(PAGE_SIZE)
    {
        __bss_start = .;

        *(.bss .bss.*)
        *(COMMON)                   /* Common symbols */

        . = ALIGN(PAGE_SIZE);
        __bss_end = .;
    }

    /* End of kernel image in virtual address space */
    __kernel_virtual_end = .;

    /* --------------------------------------------------------
     * Kernel stack section (not loaded, just reserved space)
     * -------------------------------------------------------- */
    .stack (NOLOAD) : ALIGN(PAGE_SIZE)
    {
        . = . + STACK_SIZE;
        __boot_stack_top = .;
    }

    /* --------------------------------------------------------
     * Discard debugging and unnecessary sections
     * -------------------------------------------------------- */
    /DISCARD/ :
    {
        *(.comment)
        *(.note.*)
        *(.debug*)
        *(.gnu*)
    }
}
