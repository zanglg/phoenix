.section .text, "ax"
sleep_start:
    /* Rejected duration must not change the current context or allocate. */
    ldr x0, =1000000001
    call 9
    expect -22
    mov x0, #0
    call 9
    expect 0
    mov x19, #16
sleep_round:
    adr x0, nap_path
    mov x1, #9
    call 6
    expect 0
    mov x20, #0x1234
    str x20, [sp, #128]
    ldr x0, =20000000
    tbnz x19, #0, sleep_duration_ready
    ldr x0, =40000000 /* Child exits while its parent is still sleeping. */
sleep_duration_ready:
    call 9
    expect 0
    ldr x9, [sp, #128]
    cmp x9, x20
    b.ne failed
    call 7
    expect 53
    subs x19, x19, #1
    b.ne sleep_round
    mov x0, #1
    adr x1, sleep_message
    mov x2, #sleep_message_end-sleep_message
    call 1
    expect sleep_message_end-sleep_message
    mov x0, #0
    call 0
sleep_child:
    mov x20, #0x5678
    str x20, [sp, #128]
    ldr x0, =30000000
    call 9
    expect 0
    ldr x9, [sp, #128]
    cmp x9, x20
    b.ne failed
    mov x0, #53
    call 0
    b failed
.ltorg
.section .rodata, "a"
nap_path: .ascii "nap-child"
sleep_message: .ascii "Phoenix sleep: rounds=16 deadlines=ok stack=ok wait=ok\n"
sleep_message_end:
