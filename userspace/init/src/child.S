.section .text.entry, "ax"
.balign 4096
.global _start
_start:
    mov x0, #1
    adr x1, message
    mov x2, #message_end-message
    mov x8, #1
    svc #0
    cmp x0, #message_end-message
    b.ne failed
    /* A nested run is rejected without allocating or changing the caller. */
    adr x0, path
    mov x1, #5
    mov x8, #5
    svc #0
    cmn x0, #11
    b.ne failed
    mov x0, #7
    mov x8, #0
    svc #0
failed:
    mov x0, #1
    mov x8, #0
    svc #0
    b failed
.section .rodata, "a"
message: .ascii "Phoenix child: running\n"
message_end:
path: .ascii "child"
