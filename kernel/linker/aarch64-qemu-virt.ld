OUTPUT_ARCH(aarch64)
ENTRY(_start)

/*
 * QEMU's AArch64 direct-kernel path places a raw image 512 KiB above the
 * beginning of RAM. The bootstrap executes at this physical address, creates
 * a temporary 1 GiB block mapping, and then enters the linked higher half.
 */
KERNEL_PHYS_BASE = 0x0000000040080000;
KERNEL_VIRT_OFFSET = 0xffffff8000000000;
KERNEL_VIRT_BASE = KERNEL_PHYS_BASE + KERNEL_VIRT_OFFSET;
BOOT_STACK_SIZE = 64K;

PHDRS
{
    text   PT_LOAD FLAGS(5); /* R-X */
    rodata PT_LOAD FLAGS(4); /* R-- */
    data   PT_LOAD FLAGS(6); /* RW- */
}

SECTIONS
{
    . = KERNEL_VIRT_BASE;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VIRT_OFFSET)
    {
        KEEP(*(.text.boot))
        *(.text .text.*)
    } :text

    . = ALIGN(4K);
    .user_probe : AT(ADDR(.user_probe) - KERNEL_VIRT_OFFSET)
    {
        __user_probe_start = .;
        KEEP(*(.user_probe.text))
        . = ALIGN(4K);
        __user_probe_end = .;
    } :text

    . = ALIGN(4K);
    .rodata : AT(ADDR(.rodata) - KERNEL_VIRT_OFFSET)
    {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4K);
    .data : AT(ADDR(.data) - KERNEL_VIRT_OFFSET)
    {
        KEEP(*(.data.boot))
        *(.data .data.*)
    } :data

    . = ALIGN(4K);
    __bss_start = .;
    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VIRT_OFFSET)
    {
        *(.bss .bss.*)
        *(COMMON)
    } :data
    . = ALIGN(8);
    __bss_end = .;

    . = ALIGN(16);
    .boot_stack (NOLOAD) : AT(ADDR(.boot_stack) - KERNEL_VIRT_OFFSET)
    {
        __boot_stack_bottom = .;
        . += BOOT_STACK_SIZE;
        . = ALIGN(16);
        __boot_stack_top = .;
    } :data

    . = ALIGN(4K);
    .user_probe_stack (NOLOAD) : AT(ADDR(.user_probe_stack) - KERNEL_VIRT_OFFSET)
    {
        __user_probe_stack_bottom = .;
        KEEP(*(.user_probe.stack))
        __user_probe_stack_top = .;
    } :data

    . = ALIGN(4K);
    __kernel_end = .;

    /DISCARD/ :
    {
        *(.comment)
        *(.eh_frame .eh_frame_hdr)
        *(.note .note.*)
    }
}

ASSERT(_start == KERNEL_VIRT_BASE, "bootstrap must be the first linked code")
ASSERT((__boot_l1_page_table & 0xfff) == 0, "boot L1 table must be 4 KiB aligned")
ASSERT((__boot_stack_top & 0xf) == 0, "boot stack must be 16-byte aligned")
ASSERT(__kernel_end - __kernel_start < 0x3ff80000,
       "bootstrap image must fit in the temporary 1 GiB mapping")
