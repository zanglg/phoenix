/* No SVC, voluntary yield, counter access or shared-memory polling in the loop.
 * The child never terminates. Only timer preemption lets init run and halt. */
.section .text, "ax"
preempt_start:
    str xzr, [sp] /* root mode */
    ldr x0, =2000000
    str x0, [sp, #8]
    mov x0, #0x1234
    str x0, [sp, #16]
    adr x0, busy_path
    mov x1, #10
    call 6
    expect 0
    b spin_initialize
busy_start:
    mov x0, #1
    str x0, [sp] /* child mode: infinite */
    mov x0, #0x5678
    str x0, [sp, #16]
spin_initialize:
    mov x0, sp
    str x0, [sp, #24]
    .irp n,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
        mov x\n, #(0x100+\n)
    .endr
    cmp xzr, xzr
spin_check:
    str x0, [sp, #32]
    mrs x0, nzcv
    lsr x0, x0, #28
    cmp x0, #6
    b.ne failed
    ldr x0, [sp, #32]
    .irp n,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30
        cmp x\n, #(0x100+\n)
        b.ne failed
    .endr
    ldr x0, [sp, #24]
    mov x1, sp
    cmp x0, x1
    b.ne failed
    ldr x0, [sp, #16]
    ldr x1, [sp]
    cbnz x1, spin_child
    mov x1, #0x1234
    cmp x0, x1
    b.ne failed
    ldr x0, [sp, #8]
    subs x0, x0, #1
    str x0, [sp, #8]
    cbz x0, preempt_done
    b spin_again
spin_child:
    mov x1, #0x5678
    cmp x0, x1
    b.ne failed
spin_again:
    mov x0, #0x100
    mov x1, #0x101
    cmp xzr, xzr
    b spin_check
preempt_done:
    mov x0, #1
    adr x1, preempt_message
    mov x2, #preempt_message_end-preempt_message
    call 1
    expect preempt_message_end-preempt_message
    mov x0, #0
    call 0 /* Init ends the system even though its child is still runnable. */
    b failed
.ltorg
.section .rodata, "a"
busy_path: .ascii "busy-child"
preempt_message: .ascii "Phoenix preemption: non-yielding init progressed registers=ok stack=ok\n"
preempt_message_end:
