.section .text.entry, "ax"
.balign 4096
.global _start
_start:
    /* argv[0] selects one of the archive's fault-* entries. */
    ldr x0, [sp, #8]
    ldrb w0, [x0, #6]
    cmp w0, #'t'
    b.eq .Ltranslation
    cmp w0, #'n'
    b.eq .Lnx
    cmp w0, #'i'
    b.eq .Lillegal
    cmp w0, #'s'
    b.eq .Lsvc
    cmp w0, #'w'
    b.eq .Lwrite
    cmp w0, #'l'
    b.eq .Llowmmio
    cmp w0, #'r'
    b.eq .Llowram
    cmp w0, #'g'
    b.eq .Lguard
    /* Deliberately try to read the kernel's higher-half text from EL0. */
    ldr x0, =0xffffff8040080000
.global fault_kernel_read
fault_kernel_read:
    ldr x1, [x0]
    b .Lunexpected
.Ltranslation:
    mov x0, #0
.global fault_translation
fault_translation:
    ldr x1, [x0]
    b .Lunexpected
.Lnx:
    /* A fixed mapped stack address gives the harness an independent NX oracle. */
    ldr x0, =0x7fffffe000
    br x0
.Lillegal:
.global fault_illegal
fault_illegal:
    .inst 0x00000000
    b .Lunexpected
.Lsvc:
.global fault_svc
fault_svc:
    svc #1
    b .Lunexpected
.Lwrite:
    adr x0, _start
.global fault_code_write
fault_code_write:
    str xzr, [x0]
    b .Lunexpected
.Llowmmio:
    mov x0, #0x09000000
.global fault_low_mmio
fault_low_mmio:
    ldr x1, [x0]
    b .Lunexpected
.Llowram:
    mov x0, #0x40000000
.global fault_low_ram
fault_low_ram:
    ldr x1, [x0]
    b .Lunexpected
.Lguard:
    ldr x0, =0x7fffffa000
.global fault_stack_guard
fault_stack_guard:
    ldr x1, [x0]
.Lunexpected:
    /* Unexpected access must fail the parent test. */
    mov x0, #1
    mov x8, #0
    svc #0
    b _start
.ltorg
