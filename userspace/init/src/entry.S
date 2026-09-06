/* Phoenix's first separately linked user program.
 *
 * It validates the exact native revision-0 initial stack prepared by the
 * kernel, exits with status 42 on success, and exits with status 1 on any
 * mismatch. It owns no runtime, relocations, writable data, or TLS.
 */

.section .text.entry, "ax"
.balign 4096

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
