.section .text.entry, "ax"
.balign 4096
.global _start
_start:
    /* Reused stack frames must be scrubbed before this new process sees them. */
    ldr x10, =0x7fffffb000
    ldr x11, [x10]
    cbnz x11, failed
    mov x11, #0x5a5a
    str x11, [x10]
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
    /* Exhaust only this child's table; intentionally leave every FD open. */
    mov x19, #3
opened:
    adr x0, motd
    mov x1, #8
    mov x2, #0
    mov x8, #3
    svc #0
    cmp x0, x19
    b.ne failed
    add x19, x19, #1
    cmp x19, #11
    b.ne opened
    adr x0, motd
    mov x1, #8
    mov x2, #0
    mov x8, #3
    svc #0
    cmn x0, #24
    b.ne failed
    mov x0, #7
    mov x8, #0
    svc #0
failed:
    mov x0, #1
    mov x8, #0
    svc #0
    b failed
.ltorg
.section .rodata, "a"
message: .ascii "Phoenix child: running\n"
message_end:
path: .ascii "child"
motd: .ascii "etc/motd"
