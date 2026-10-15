/* Phoenix's first separately linked user program.
 *
 * It validates the exact native revision-0 initial stack prepared by the
 * kernel, reads one initramfs file into its stack, exits with status 42 on
 * success, and exits with status 1 on any mismatch. It owns no runtime,
 * relocations, writable data, or TLS.
 */

.section .text.entry, "ax"
.balign 4096
.equ INIT_MESSAGE_LENGTH, 29
.equ INIT_PATH_LENGTH, 8
.equ INIT_FILE_LENGTH, 34
.equ INIT_READ_CAPACITY, 64

.global _start
.type _start, %function
_start:
    mov     x9, sp
    and     x9, x9, #0xf
    cbnz    x9, .Lfailed

    ldr     x9, [sp, #0]
    cmp     x9, #2
    b.ne    .Lfailed

    ldr     x10, [sp, #8]
    ldr     x11, [sp, #16]
    cbz     x10, .Lfailed
    cbz     x11, .Lfailed
    ldr     x9, [sp, #24]
    cbnz    x9, .Lfailed

    ldr     x12, [sp, #32]
    cbz     x12, .Lfailed
    ldr     x9, [sp, #40]
    cbnz    x9, .Lfailed

    ldr     x9, [sp, #48]
    cmp     x9, #6
    b.ne    .Lfailed
    ldr     x9, [sp, #56]
    cmp     x9, #1, lsl #12
    b.ne    .Lfailed

    ldr     x9, [sp, #64]
    cmp     x9, #9
    b.ne    .Lfailed
    ldr     x9, [sp, #72]
    mov     x13, #0x40
    lsl     x13, x13, #16
    cmp     x9, x13
    b.ne    .Lfailed

    ldr     x9, [sp, #80]
    mov     x13, #0x5000
    cmp     x9, x13
    b.ne    .Lfailed
    ldr     x9, [sp, #88]
    cbnz    x9, .Lfailed
    ldp     x9, x13, [sp, #96]
    orr     x9, x9, x13
    cbnz    x9, .Lfailed

    ldrb    w9, [x10, #0]
    cmp     w9, #'/'
    b.ne    .Lfailed
    ldrb    w9, [x10, #5]
    cbnz    w9, .Lfailed

    ldrb    w9, [x11, #0]
    cmp     w9, #'p'
    b.ne    .Lfailed
    ldrb    w9, [x11, #7]
    cbnz    w9, .Lfailed

    ldrb    w9, [x12, #0]
    cmp     w9, #'T'
    b.ne    .Lfailed
    ldrb    w9, [x12, #4]
    cmp     w9, #'='
    b.ne    .Lfailed
    ldrb    w9, [x12, #12]
    cbnz    w9, .Lfailed

    mov     x0, #1
    adr     x1, __init_message_start
    mov     x2, #INIT_MESSAGE_LENGTH
    mov     x8, #1
    svc     #0
    cmp     x0, #INIT_MESSAGE_LENGTH
    b.ne    .Lfailed

    adr     x0, __init_path_start
    mov     x1, #INIT_PATH_LENGTH
    mov     x2, #0
    mov     x8, #3
    svc     #0
    cmp     x0, #3
    b.ne    .Lfailed
    mov     x19, x0

    sub     sp, sp, #INIT_READ_CAPACITY
    mov     x0, x19
    mov     x1, sp
    mov     x2, #INIT_READ_CAPACITY
    mov     x8, #2
    svc     #0
    cmp     x0, #INIT_FILE_LENGTH
    b.ne    .Lfailed

    mov     x2, x0
    mov     x0, #1
    mov     x1, sp
    mov     x8, #1
    svc     #0
    cmp     x0, #INIT_FILE_LENGTH
    b.ne    .Lfailed

    mov     x0, x19
    mov     x1, sp
    mov     x2, #INIT_READ_CAPACITY
    mov     x8, #2
    svc     #0
    cbnz    x0, .Lfailed

    mov     x0, x19
    mov     x8, #4
    svc     #0
    cbnz    x0, .Lfailed
    add     sp, sp, #INIT_READ_CAPACITY

    mov     x0, #42
    mov     x8, #0
    svc     #0

.Lfailed:
    mov     x0, #1
    mov     x8, #0
    svc     #0

.Lunreachable:
    b       .Lunreachable

.size _start, . - _start

.section .rodata.message, "a"
.balign 16
.global __init_message_start
__init_message_start:
    .ascii "Phoenix init: hello from EL0\n"
.global __init_message_end
__init_message_end:

.balign 8
.global __init_path_start
__init_path_start:
    .ascii "etc/motd"
.global __init_path_end
__init_path_end:

.if (__init_message_end - __init_message_start) != INIT_MESSAGE_LENGTH
    .error "init message length constant is stale"
.endif

.if (__init_path_end - __init_path_start) != INIT_PATH_LENGTH
    .error "init path length constant is stale"
.endif
