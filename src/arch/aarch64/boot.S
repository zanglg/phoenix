/******************************************************************************
 *                                                                            *
 * AArch64 Kernel Bootstrap Code                                              *
 *                                                                            *
 ******************************************************************************/

.section .text.boot
.globl _start

_start:
    /* ------------------------------------------------------------
     * Multi-core Filter
     * ------------------------------------------------------------ */
    mrs  x19, mpidr_el1
    and  x19, x19, #0xff        /* Check Affinity 0 (CPU ID) */
    cbnz x19, .L_halt           /* Only Primary Core (ID 0) proceeds */

    bl   .L_init_kernel_el      /* Initialize Exception Level */
    bl   .L_setup_cpu           /* Configure System Control Registers */
    bl   .L_create_pagetable    /* Setup Initial Translation Tables */

/* ------------------------------------------------------------
 * MMU Enablement and Context Switch
 * ------------------------------------------------------------ */
.L_enable_mmu:
    /* Enable MMU and Caches
     * M - BIT[0]  - 0x1 - Enable MMU
     * C - BIT[2]  - 0x1 - Enable Data and Unified Caches
     * I - BIT[12] - 0x1 - Enable Instruction Cache
     */
    mrs  x0, sctlr_el1
    ldr  x1, =0x1005
    orr  x0, x0, x1             /* M=1, C=1, I=1 */
    msr  sctlr_el1, x0
    isb

    /* Absolute Jump to Virtual Space */
    ldr  x8, =.L_virtual_jump
    br   x8

.L_virtual_jump:
    /* ------------------------------------------------------------
     * Establish Virtual Address Stack
     * ------------------------------------------------------------
     * Now running in High Virtual Address Space (TTBR1)
     */
    ldr  x0, =__boot_stack_top
    mov  sp, x0                 /* Switch to true Virtual Stack */

    /* Efficiently clear BSS using STP (Store Pair) */
    ldr  x0, =__bss_start
    ldr  x1, =__bss_end
.L_bss_loop:
    cmp  x0, x1
    b.ge .L_bss_done
    stp  xzr, xzr, [x0], #16    /* Clear 16 bytes per iteration */
    b    .L_bss_loop

.L_bss_done:
    bl   kernel_main              /* Enter Kernel */

.L_halt:
    wfe
    b    .L_halt

/* ------------------------------------------------------------
 * Exception Level Initialization
 * ------------------------------------------------------------ */
.L_init_kernel_el:
    mrs  x0, CurrentEL
    and  x0, x0, #0b1100        /* BIT[3:2] contains EL */
    cmp  x0, #0b1000            /* 0b1000 is EL2 */
    b.eq .L_init_el2

.L_init_el1:
    /* Low-level EL1 Reset: Little Endian, MMU/Cache Disabled */
    ldr  x0, =0x30C50830
    msr  sctlr_el1, x0          /* Ref: ARM DDI 0487 - SCTLR_EL1 Reset Value */
    isb
    ret

.L_init_el2:
    msr  SPsel, #1              /* Use SP_ELx for Exception level ELx */

    /* HCR_EL2: Execution State Control
     * HCR_RW - BIT[31] - 0x1 - EL1 execution state is AArch64
     */
    ldr  x0, =0x80000000
    msr  hcr_el2, x0
    isb

    /* SPSR_EL2: Saved Program Status Register
     * M[3:0] - BIT[3:0] - 0x5   - Return to EL1h (using SP_EL1)
     * DAIF   - BIT[9:6] - 0b1111 - Mask all interrupts
     */
    mov  x0, #0x3c5
    msr  spsr_el2, x0

    /* Switch to EL1 via Exception Return */
    adr  x0, .L_init_el1
    msr  elr_el2, x0
    eret

/* ------------------------------------------------------------
 * System Control Register Configuration
 * ------------------------------------------------------------ */
.L_setup_cpu:
    tlbi vmalle1                /* Invalidate local TLB */
    dsb  nsh

    /* MAIR_EL1: Memory Attribute Indirection Register
     * Index 0: MT_NORMAL         - 0xFF
     * Index 1: MT_NORMAL_TAGGED  - 0xF0
     * Index 2: MT_NORMAL_NC      - 0x44
     * Index 3: MT_DEVICE_nGnRnE  - 0x00
     * Index 4: MT_DEVICE_nGnRE   - 0x04
     */
    ldr  x0, =0x040044f0ff
    msr  mair_el1, x0

    /* TCR_EL1: Translation Control Register
     * TCR_T0SZ   - BIT[5:0]   - 25   - 2^(64-25) = 39-bit VA for TTBR0
     * TCR_IRGN0  - BIT[9:8]   - 0x1  - Normal, Inner Write-Back Cacheable
     * TCR_ORGN0  - BIT[11:10] - 0x1  - Normal, Outer Write-Back Cacheable
     * TCR_SH0    - BIT[13:12] - 0x3  - Inner Shareable for TTBR0
     * TCR_TG0    - BIT[14]    - 0x0  - 4KB granule for TTBR0
     * TCR_T1SZ   - BIT[21:16] - 25   - 2^(64-25) = 39-bit VA for TTBR1
     * TCR_IRGN1  - BIT[25:24] - 0x1  - Normal, Inner Write-Back Cacheable
     * TCR_ORGN1  - BIT[27:26] - 0x1  - Normal, Outer Write-Back Cacheable
     * TCR_SH1    - BIT[29:28] - 0x3  - Inner Shareable for TTBR1
     * TCR_TG1    - BIT[30]    - 0x2  - 4KB granule for TTBR1 (Ref: BIT[31:30]=0b10)
     * TCR_IPS    - BIT[34:32] - 0x5  - 48-bit Intermediate Physical Address
     */
    ldr  x0, =0x5b5193519
    msr  tcr_el1, x0
    isb
    ret

/* ------------------------------------------------------------
 * Page Table Pointer Setup
 * ------------------------------------------------------------ */
.L_create_pagetable:
    adrp x0, __kernel_pagetable_l1
    add  x0, x0, :lo12:__kernel_pagetable_l1
    msr  ttbr0_el1, x0          /* For Identity Mapping */
    msr  ttbr1_el1, x0          /* For High-half Kernel Mapping (Entry 0 Mirror) */
    isb
    ret

/* ------------------------------------------------------------
 * Static Page Tables (1GB Block Mapping)
 * ------------------------------------------------------------ */
.section .data
.balign 4096
__kernel_pagetable_l1:
    /* [0]: Mapping to PA 0x00000000 (Device)
     * MT_DEVICE_nGnRnE (Index 3), NS, AF, UXN/PXN
     */
    .quad 0x006000000000040D

    /* [1]: Mapping to PA 0x40000000 (RAM)
     * MT_NORMAL (Index 0), Inner Shareable, AF, UXN
     */
    .quad 0x0040000040000721

    /* ------------------------------------------------------------
     * Pad the rest of the 4KB page (512 total entries * 8 bytes).
     * This prevents the linker from placing other data structures
     * inside the translation table page, which could lead to
     * cache-line conflicts or security leaks.
     * ------------------------------------------------------------ */
    .fill 510, 8, 0

.balign 4096 /* Confirm final page boundary alignment */
