OUTPUT_ARCH(aarch64)
ENTRY(_start)

USER_ENTRY = 0x0000000000400000;

PHDRS
{
    text PT_LOAD FLAGS(5); /* R-X */
}

SECTIONS
{
    . = USER_ENTRY;
    __init_start = .;

    .text : ALIGN(4K)
    {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } :text

    .rodata : ALIGN(16)
    {
        *(.rodata .rodata.*)
    } :text

    __init_end = .;

    /DISCARD/ :
    {
        *(.comment)
        *(.eh_frame .eh_frame_hdr)
        *(.note .note.*)
        *(.data .data.*)
        *(.bss .bss.*)
    }
}

ASSERT(_start == USER_ENTRY, "init entry must begin at its linked user address")
ASSERT(__init_end - __init_start <= 4K, "initial init image must fit one text page")
