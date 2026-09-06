/* AArch64 EL1 exception vectors.
 *
 * Every architectural slot saves the ExceptionFrame layout defined by
 * exception.rs before branching to the common Rust dispatcher. Keep the
 * constants below synchronized with its compile-time offset assertions.
 */

.equ EXCEPTION_FRAME_SIZE, 288
.equ FRAME_SP_EL0,          248
.equ FRAME_ELR_EL1,         256
.equ FRAME_SPSR_EL1,        264
.equ FRAME_ESR_EL1,         272
.equ FRAME_FAR_EL1,         280
.equ VECTOR_ENTRY_SIZE,     128
.equ VECTOR_TABLE_SIZE,     2048

.section .text.exceptions, "ax"
.balign 2048

.global __exception_vectors
.type __exception_vectors, %function
__exception_vectors:

/*
 * switch_sp is required only for exceptions taken from the current EL while
 * PSTATE.SP selected SP_EL0. No general-purpose register is touched before it
 * is saved. DAIF is masked immediately to prevent an asynchronous nested
 * exception from reusing the incomplete frame.
 */
.macro exception_vector name, slot, switch_sp=0
\name:
    msr     DAIFSet, #0xf
.if \switch_sp
    msr     SPSel, #1
.endif
    sub     sp, sp, #EXCEPTION_FRAME_SIZE
    stp     x0,  x1,  [sp, #0]
    stp     x2,  x3,  [sp, #16]
    stp     x4,  x5,  [sp, #32]
    stp     x6,  x7,  [sp, #48]
    stp     x8,  x9,  [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    str     x30, [sp, #240]
    mrs     x9, SP_EL0
    str     x9, [sp, #FRAME_SP_EL0]
    mrs     x9, ELR_EL1
    str     x9, [sp, #FRAME_ELR_EL1]
    mrs     x9, SPSR_EL1
    str     x9, [sp, #FRAME_SPSR_EL1]
    mrs     x9, ESR_EL1
    str     x9, [sp, #FRAME_ESR_EL1]
    mrs     x9, FAR_EL1
    str     x9, [sp, #FRAME_FAR_EL1]
    mov     x0, sp
    mov     x1, #\slot
    b       __exception_dispatch
.if (. - \name) > VECTOR_ENTRY_SIZE
    .error "AArch64 exception vector exceeds its 128-byte slot"
.endif
    .space  VECTOR_ENTRY_SIZE - (. - \name)
.endm

exception_vector vector_current_sp0_sync,   0, 1
exception_vector vector_current_sp0_irq,    1, 1
exception_vector vector_current_sp0_fiq,    2, 1
exception_vector vector_current_sp0_serror, 3, 1
exception_vector vector_current_spx_sync,   4
exception_vector vector_current_spx_irq,    5
exception_vector vector_current_spx_fiq,    6
exception_vector vector_current_spx_serror, 7
exception_vector vector_lower_a64_sync,     8
exception_vector vector_lower_a64_irq,      9
exception_vector vector_lower_a64_fiq,     10
exception_vector vector_lower_a64_serror,  11
exception_vector vector_lower_a32_sync,    12
exception_vector vector_lower_a32_irq,     13
exception_vector vector_lower_a32_fiq,     14
exception_vector vector_lower_a32_serror,  15

.global __exception_vectors_end
__exception_vectors_end:

.type __exception_dispatch, %function
__exception_dispatch:
    bl      phoenix_exception_dispatch

    /* A returning dispatcher may update SP_EL0, ELR_EL1, or SPSR_EL1. */
    ldr     x9, [sp, #FRAME_SP_EL0]
    msr     SP_EL0, x9
    ldr     x9, [sp, #FRAME_ELR_EL1]
    msr     ELR_EL1, x9
    ldr     x9, [sp, #FRAME_SPSR_EL1]
    msr     SPSR_EL1, x9

    ldr     x30, [sp, #240]
    ldp     x28, x29, [sp, #224]
    ldp     x26, x27, [sp, #208]
    ldp     x24, x25, [sp, #192]
    ldp     x22, x23, [sp, #176]
    ldp     x20, x21, [sp, #160]
    ldp     x18, x19, [sp, #144]
    ldp     x16, x17, [sp, #128]
    ldp     x14, x15, [sp, #112]
    ldp     x12, x13, [sp, #96]
    ldp     x10, x11, [sp, #80]
    ldp     x8,  x9,  [sp, #64]
    ldp     x6,  x7,  [sp, #48]
    ldp     x4,  x5,  [sp, #32]
    ldp     x2,  x3,  [sp, #16]
    ldp     x0,  x1,  [sp, #0]
    add     sp, sp, #EXCEPTION_FRAME_SIZE
    eret

.size __exception_vectors, __exception_vectors_end - __exception_vectors
