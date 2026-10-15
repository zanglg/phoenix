/* Two independent instances share code addresses, never stacks or descriptors. */
.section .text.entry, "ax"
.balign 4096
.global _start
.type _start, %function
.macro call number
    mov x8, #\number
    svc #0
.endm
.macro expect value
    mov x9, #\value
    cmp x0, x9
    b.ne failed
.endm
_start:
    ldr x9, [sp, #8]
    ldrb w9, [x9]
    sub sp, sp, #256
    cmp w9, #'y'
    b.eq child_start
    /* Empty wait, a lone yield, and failed spawn must preserve the caller. */
    call 7
    expect -10
    bl checked_yield
    mov x0, #0
    mov x1, #5
    call 6
    expect -14
    adr x0, missing
    mov x1, #7
    call 6
    expect -2
    adr x0, badelf
    mov x1, #6
    call 6
    expect -22
    /* fd 3 is a per-round cursor; fd 4 survives all children at offset one. */
    adr x0, shared
    mov x1, #8
    mov x2, #1
    call 3
    expect 3
    call 4
    expect 0
    adr x0, motd
    mov x1, #8
    mov x2, #0
    call 3
    expect 3
    /* Temporarily occupy fd 3, so the retained motd lands at fd 4. */
    adr x0, motd
    mov x1, #8
    mov x2, #0
    call 3
    expect 4
    mov x0, #3
    call 4
    expect 0
    mov x0, #4
    mov x1, sp
    mov x2, #1
    call 2
    expect 1
    ldrb w9, [sp]
    cmp w9, #'P'
    b.ne failed
    mov x19, #64
round:
    /* Reset shared bytes to AB, then give the parent a fresh read cursor. */
    adr x0, shared
    mov x1, #8
    mov x2, #1
    call 3
    expect 3
    mov x0, #3
    adr x1, initial
    mov x2, #2
    call 1
    expect 2
    mov x0, #3
    call 4
    expect 0
    adr x0, shared
    mov x1, #8
    mov x2, #0
    call 3
    expect 3
    mov x20, #0x1234
    str x20, [sp, #128]
    adr x0, child
    mov x1, #11
    call 6
    expect 0
    /* A ready child occupies the only slot; synchronous run is rejected too. */
    adr x0, child
    mov x1, #11
    call 6
    expect -11
    call 5
    expect -11
    bl checked_yield
    /* C proves the child really ran; fd 3 must still be at the parent's zero. */
    mov x0, #3
    mov x1, sp
    mov x2, #1
    call 2
    expect 1
    ldrb w9, [sp]
    cmp w9, #'C'
    b.ne failed
    bl checked_yield
    mov x0, #3
    mov x1, sp
    mov x2, #2
    call 2
    expect 2
    ldrh w9, [sp]
    mov x10, #0x4442 /* BD */
    cmp x9, x10
    b.ne failed
    bl checked_yield /* Child exits before parent waits. */
    adr x0, child
    mov x1, #11
    call 6
    expect -11 /* A zombie also holds the slot. */
    call 7
    expect 43
    ldr x9, [sp, #128]
    cmp x9, x20
    b.ne failed
    call 7
    expect -10
    mov x0, #3
    call 4
    expect 0
    /* Wait first: both child yields must select the child again, never init. */
    adr x0, child
    mov x1, #11
    call 6
    expect 0
    call 7
    expect 43
    /* Terminal faults before and after the wait must have the same outcome. */
    adr x0, illegal
    mov x1, #7
    call 6
    expect 0
    call 7
    expect -14
    adr x0, illegal
    mov x1, #7
    call 6
    expect 0
    bl checked_yield
    call 7
    expect -14
    subs x19, x19, #1
    b.ne round
    /* The retained parent descriptor remains positioned after P. */
    mov x0, #4
    mov x1, sp
    mov x2, #64
    call 2
    expect 33
    ldrb w9, [sp]
    cmp w9, #'h'
    b.ne failed
    mov x0, #4
    call 4
    expect 0
    /* Preserve the legacy synchronous operation after cooperative reuse. */
    adr x0, normal
    mov x1, #5
    call 5
    expect 7
    mov x0, #1
    adr x1, done
    mov x2, #done_end-done
    call 1
    expect done_end-done
    mov x0, #0
    call 0

child_start:
    /* A new address space must expose zero, not a previous child's poison. */
    ldr x10, =0x7fffffb000
    ldr x11, [x10]
    cbnz x11, failed
    mov x11, #0x5a5a
    str x11, [x10]
    mov x20, #0x5678
    str x20, [sp, #128]
    call 7
    expect -10
    adr x0, child
    mov x1, #11
    call 6
    expect -11
    adr x0, shared
    mov x1, #8
    mov x2, #1
    call 3
    expect 3
    adr x0, motd
    mov x1, #8
    mov x2, #0
    call 3
    expect 4
    mov x0, #4
    mov x1, sp
    mov x2, #1
    call 2
    expect 1
    ldrb w9, [sp]
    cmp w9, #'P'
    b.ne failed
    mov x0, #3
    adr x1, changed
    mov x2, #1
    call 1
    expect 1
    bl checked_yield
    mov x0, #3
    mov x1, sp
    mov x2, #1
    call 2
    expect 1
    ldrb w9, [sp]
    cmp w9, #'B'
    b.ne failed
    mov x0, #3
    adr x1, changed_end
    mov x2, #1
    call 1
    expect 1
    bl checked_yield
    ldr x9, [sp, #128]
    cmp x9, x20
    b.ne failed
    mov x0, #43
    call 0 /* Descriptors intentionally remain open until reap. */

/* Check all preserved GPRs and NZCV on every cooperative handoff. */
checked_yield:
    sub sp, sp, #112
    stp x19, x20, [sp]
    stp x21, x22, [sp, #16]
    stp x23, x24, [sp, #32]
    stp x25, x26, [sp, #48]
    stp x27, x28, [sp, #64]
    stp x29, x30, [sp, #80]
    mov x0, sp
    str x0, [sp, #96]
    .irp n,1,2,3,4,5,6,7,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
        mov x\n, #(0x100+\n)
    .endr
    cmp xzr, xzr
    call 8
    cbnz x0, failed
    mrs x0, nzcv
    cmp x8, #8
    b.ne failed
    mov x8, #0x60000000
    cmp x0, x8
    b.ne failed
    .irp n,1,2,3,4,5,6,7,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
        mov x0, #(0x100+\n)
        cmp x\n, x0
        b.ne failed
    .endr
    ldr x0, [sp, #96]
    mov x8, sp
    cmp x0, x8
    b.ne failed
    ldp x19, x20, [sp]
    ldp x21, x22, [sp, #16]
    ldp x23, x24, [sp, #32]
    ldp x25, x26, [sp, #48]
    ldp x27, x28, [sp, #64]
    ldp x29, x30, [sp, #80]
    add sp, sp, #112
    ret
failed:
    mov x0, #1
    call 0
    b failed
.size _start, .-_start
.ltorg
.section .rodata, "a"
shared: .ascii "tmp/coop"
motd: .ascii "etc/motd"
initial: .ascii "AB"
changed: .ascii "C"
changed_end: .ascii "D"
child: .ascii "yield-child"
normal: .ascii "child"
illegal: .ascii "fault-i"
missing: .ascii "missing"
badelf: .ascii "badelf"
done: .ascii "Phoenix cooperative: rounds=64 registers=ok stack=ok files=ok waits=ok\n"
done_end:
