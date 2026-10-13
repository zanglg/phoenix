.section .text.diagnostics, "ax"
.balign 4
.global phoenix_register_probe
.type phoenix_register_probe, %function
phoenix_register_probe:
    sub sp, sp, #112
    stp x19, x20, [sp, #0]
    stp x21, x22, [sp, #16]
    stp x23, x24, [sp, #32]
    stp x25, x26, [sp, #48]
    stp x27, x28, [sp, #64]
    stp x29, x30, [sp, #80]
    mrs x9, SP_EL0
    str x9, [sp, #96]
    mov x9, #0x1230
    msr SP_EL0, x9
    mov x9, #0xa0000000
    msr NZCV, x9
    mov x0, #256
    mov x1, #257
    mov x2, #258
    mov x3, #259
    mov x4, #260
    mov x5, #261
    mov x6, #262
    mov x7, #263
    mov x8, #264
    mov x9, #265
    mov x10, #266
    mov x11, #267
    mov x12, #268
    mov x13, #269
    mov x14, #270
    mov x15, #271
    mov x16, #272
    mov x17, #273
    mov x18, #274
    mov x19, #275
    mov x20, #276
    mov x21, #277
    mov x22, #278
    mov x23, #279
    mov x24, #280
    mov x25, #281
    mov x26, #282
    mov x27, #283
    mov x28, #284
    mov x29, #285
    mov x30, #286
.global phoenix_register_probe_trap
phoenix_register_probe_trap:
    brk #0x123
    str x16, [sp, #104]
    mrs x16, NZCV
    lsr x16, x16, #28
    cmp x16, #0xa
    b.ne .Lregister_failure
    mrs x16, SP_EL0
    sub x16, x16, #0x1000
    cmp x16, #0x230
    b.ne .Lregister_failure
    ldr x16, [sp, #104]
    cmp x0, #256
    b.ne .Lregister_failure
    cmp x1, #257
    b.ne .Lregister_failure
    cmp x2, #258
    b.ne .Lregister_failure
    cmp x3, #259
    b.ne .Lregister_failure
    cmp x4, #260
    b.ne .Lregister_failure
    cmp x5, #261
    b.ne .Lregister_failure
    cmp x6, #262
    b.ne .Lregister_failure
    cmp x7, #263
    b.ne .Lregister_failure
    cmp x8, #264
    b.ne .Lregister_failure
    cmp x9, #265
    b.ne .Lregister_failure
    cmp x10, #266
    b.ne .Lregister_failure
    cmp x11, #267
    b.ne .Lregister_failure
    cmp x12, #268
    b.ne .Lregister_failure
    cmp x13, #269
    b.ne .Lregister_failure
    cmp x14, #270
    b.ne .Lregister_failure
    cmp x15, #271
    b.ne .Lregister_failure
    cmp x16, #272
    b.ne .Lregister_failure
    cmp x17, #273
    b.ne .Lregister_failure
    cmp x18, #274
    b.ne .Lregister_failure
    cmp x19, #275
    b.ne .Lregister_failure
    cmp x20, #276
    b.ne .Lregister_failure
    cmp x21, #277
    b.ne .Lregister_failure
    cmp x22, #278
    b.ne .Lregister_failure
    cmp x23, #279
    b.ne .Lregister_failure
    cmp x24, #280
    b.ne .Lregister_failure
    cmp x25, #281
    b.ne .Lregister_failure
    cmp x26, #282
    b.ne .Lregister_failure
    cmp x27, #283
    b.ne .Lregister_failure
    cmp x28, #284
    b.ne .Lregister_failure
    cmp x29, #285
    b.ne .Lregister_failure
    cmp x30, #286
    b.ne .Lregister_failure
    mov x0, #0
    b .Lregister_restore
.Lregister_failure:
    mov x0, #1
.Lregister_restore:
    ldr x9, [sp, #96]
    msr SP_EL0, x9
    ldp x19, x20, [sp, #0]
    ldp x21, x22, [sp, #16]
    ldp x23, x24, [sp, #32]
    ldp x25, x26, [sp, #48]
    ldp x27, x28, [sp, #64]
    ldp x29, x30, [sp, #80]
    add sp, sp, #112
    ret
.size phoenix_register_probe, . - phoenix_register_probe

.global phoenix_fault_probe
.type phoenix_fault_probe, %function
phoenix_fault_probe:
    cmp x1, #1
    b.eq .Lwrite
    cmp x1, #2
    b.eq .Lexecute
    ldr x1, [x0]
    ret
.Lwrite:
    str xzr, [x0]
    ret
.Lexecute:
    br x0
.global phoenix_fault_probe_resume
phoenix_fault_probe_resume:
    ret
.size phoenix_fault_probe, . - phoenix_fault_probe

.global phoenix_fatal_probe
.global phoenix_fatal_probe_trap
.type phoenix_fatal_probe, %function
phoenix_fatal_probe:
phoenix_fatal_probe_trap:
    brk #0x456
    ret
.size phoenix_fatal_probe, . - phoenix_fatal_probe

.global phoenix_stack_probe
.type phoenix_stack_probe, %function
phoenix_stack_probe:
    mov x9, sp
    mov sp, x0
    stp x9, x30, [sp, #-16]!
    mov x0, sp
    ldp x9, x30, [sp], #16
    mov sp, x9
    ret
.size phoenix_stack_probe, . - phoenix_stack_probe
