/* boot.S
 *
 * Early AArch64 bootstrap for the kernel.
 *
 * This file contains the minimum architecture-specific code required to
 * transfer control from the firmware-provided CPU state into the Rust kernel
 * entry point.
 *
 * Design model
 * ------------
 * The kernel is linked in the higher-half virtual address space, but is loaded
 * and first executed through a low physical address alias. During early boot,
 * MMU translation is disabled, so all symbol references must remain valid while
 * the CPU is still executing from the physical load address.
 *
 * The bootstrap code installs a temporary EL1 stage-1 translation table that
 * provides:
 *
 *   - a low device mapping for 0..1GiB physical addresses, so early kernel
 *     code can access low MMIO before final device mappings exist;
 *   - a low identity mapping, so instruction fetch can continue immediately
 *     after enabling the MMU; and
 *   - a higher-half mapping, so the kernel can switch to its linked virtual
 *     address space before entering the kernel entry point.
 *
 * This temporary mapping is intentionally coarse. It exists only to reach the
 * kernel entry point. The kernel memory subsystem is expected to build and
 * install the final kernel page tables with proper permissions for text,
 * rodata, data, stack, heap and device memory.
 *
 * Entry assumptions
 * -----------------
 * The CPU is expected to enter this code in AArch64 state at either EL2 or EL1.
 * EL3 setup is deliberately outside the scope of this file and should be
 * handled by firmware such as TF-A, U-Boot, EDK2 or platform-specific monitor
 * code.
 *
 * If entered at EL2, this code performs the minimal EL2 configuration required
 * to run the kernel at EL1. The transition to EL1 uses an exception return with
 * an explicit SPSR_EL2 value so that the resulting EL1 PSTATE is deterministic.
 *
 * If entered at EL1, this code still normalizes PSTATE through an EL1 exception
 * return. This mirrors the style used by Linux arm64 early boot code and avoids
 * depending on firmware-provided interrupt mask or stack-selection state.
 *
 * Deliberately deferred work
 * -------------------------
 * This file does not enable data cache, instruction cache, FP/SIMD, debug
 * facilities, SMP bring-up or precise MMIO mappings. Those require additional
 * policy and runtime support, such as cache maintenance, exception handling,
 * context switching and a final page-table layout.
 *
 * Invariants
 * ----------
 * - Only the primary core continues past the early parking code.
 * - kernel_main() is entered at EL1.
 * - kernel_main() is entered with the MMU enabled.
 * - kernel_main() is entered using the higher-half boot stack.
 * - x0 is preserved as the boot argument, commonly the physical address of the
 *   device tree blob.
 */

.section .text.boot, "ax"
.balign 4

.global _start
.type _start, %function

/* -------------------------------------------------------------------------- */
/* Generic bit constants                                                       */
/* -------------------------------------------------------------------------- */

.equiv BIT_0,                  (1 << 0)
.equiv BIT_1,                  (1 << 1)
.equiv BIT_2,                  (1 << 2)
.equiv BIT_3,                  (1 << 3)
.equiv BIT_4,                  (1 << 4)
.equiv BIT_5,                  (1 << 5)
.equiv BIT_6,                  (1 << 6)
.equiv BIT_7,                  (1 << 7)
.equiv BIT_8,                  (1 << 8)
.equiv BIT_9,                  (1 << 9)
.equiv BIT_10,                 (1 << 10)
.equiv BIT_11,                 (1 << 11)
.equiv BIT_12,                 (1 << 12)
.equiv BIT_13,                 (1 << 13)
.equiv BIT_14,                 (1 << 14)
.equiv BIT_15,                 (1 << 15)
.equiv BIT_16,                 (1 << 16)
.equiv BIT_20,                 (1 << 20)
.equiv BIT_22,                 (1 << 22)
.equiv BIT_23,                 (1 << 23)
.equiv BIT_28,                 (1 << 28)
.equiv BIT_29,                 (1 << 29)
.equiv BIT_31,                 (1 << 31)
.equiv BIT_53,                 (1 << 53)
.equiv BIT_54,                 (1 << 54)

/* -------------------------------------------------------------------------- */
/* Exception level and PSTATE constants                                        */
/* -------------------------------------------------------------------------- */

.equiv CURRENT_EL1,            (0b01 << 2)
.equiv CURRENT_EL2,            (0b10 << 2)

.equiv PSR_MODE_EL1H,          0b0101
.equiv PSR_D_BIT,              BIT_9
.equiv PSR_A_BIT,              BIT_8
.equiv PSR_I_BIT,              BIT_7
.equiv PSR_F_BIT,              BIT_6

/*
 * Initial PSTATE used for synthetic exception return into EL1.
 *
 * Result:
 *   - target mode: EL1h
 *   - Debug exception masked
 *   - SError masked
 *   - IRQ masked
 *   - FIQ masked
 */
.equiv INIT_PSTATE_EL1,        (PSR_D_BIT | PSR_A_BIT | PSR_I_BIT | PSR_F_BIT | PSR_MODE_EL1H)

/* -------------------------------------------------------------------------- */
/* MPIDR                                                                       */
/* -------------------------------------------------------------------------- */

/*
 * Simple primary-core selection.
 *
 * This accepts only Aff0 == 0. It is sufficient for early single-core bring-up.
 * A production SMP path should use the platform topology instead of assuming
 * Aff0 uniquely identifies the boot CPU.
 */
.equiv MPIDR_AFF0_MASK,        0xff

/* -------------------------------------------------------------------------- */
/* EL2 control                                                                 */
/* -------------------------------------------------------------------------- */

/*
 * HCR_EL2.RW:
 *   1 = EL1 is AArch64.
 */
.equiv HCR_EL2_RW,             BIT_31

/*
 * CNTHCTL_EL2:
 *
 * EL1PCTEN:
 *   allow EL1 access to physical counter.
 *
 * EL1PCEN:
 *   allow EL1 access to physical timer.
 */
.equiv CNTHCTL_EL2_EL1PCTEN,   BIT_0
.equiv CNTHCTL_EL2_EL1PCEN,    BIT_1

.equiv CNTHCTL_EL2_EL1_TIMER_ACCESS, (CNTHCTL_EL2_EL1PCTEN | CNTHCTL_EL2_EL1PCEN)

/* -------------------------------------------------------------------------- */
/* SCTLR_EL1                                                                   */
/* -------------------------------------------------------------------------- */

.equiv SCTLR_EL1_M,            BIT_0
.equiv SCTLR_EL1_C,            BIT_2
.equiv SCTLR_EL1_I,            BIT_12

/*
 * Cortex-A72-style reset value used as the EL1 bootstrap baseline:
 *
 *   - little endian
 *   - MMU disabled
 *   - data cache disabled
 *   - instruction cache disabled
 *   - architecturally required/reserved bits preserved for this target class
 *
 * Keep this named rather than scattering the raw value across the bootstrap
 * path. If the target CPU family changes, audit this value against the target
 * architecture profile and TRM.
 */
.equiv SCTLR_EL1_A72_MMU_OFF,  0x00c50838
.equiv SCTLR_EL1_BOOT_INIT,    SCTLR_EL1_A72_MMU_OFF

/* -------------------------------------------------------------------------- */
/* MAIR_EL1                                                                    */
/* -------------------------------------------------------------------------- */

/*
 * Bootstrap memory attributes.
 *
 * AttrIdx[0]:
 *   Normal memory, Inner/Outer Write-Back, Read/Write Allocate.
 *
 * AttrIdx[1]:
 *   Device-nGnRnE. Reserved for later precise MMIO mappings.
 */
.equiv MAIR_ATTR_NORMAL_WBWA,      0xff
.equiv MAIR_ATTR_DEVICE_nGnRnE,    0x00

.equiv MAIR_IDX_NORMAL,            0
.equiv MAIR_IDX_DEVICE,            1

.equiv MAIR_EL1_BOOT_VALUE,        ((MAIR_ATTR_NORMAL_WBWA << (8 * MAIR_IDX_NORMAL)) | (MAIR_ATTR_DEVICE_nGnRnE << (8 * MAIR_IDX_DEVICE)))

/* -------------------------------------------------------------------------- */
/* TCR_EL1                                                                     */
/* -------------------------------------------------------------------------- */

/*
 * Temporary translation regime:
 *
 *   - TTBR0 VA size: 39-bit
 *   - TTBR1 VA size: 39-bit
 *   - translation granule: 4KiB
 *   - page-table walk cacheability: Inner/Outer WBWA
 *   - page-table walk shareability: Inner Shareable
 *   - intermediate physical address size: 4GiB
 *
 * The 4GiB IPS value is intentionally conservative for bootstrap. If the
 * kernel needs to access physical memory above 4GiB during early boot, derive
 * IPS from ID_AA64MMFR0_EL1.PARange instead of using this fixed value.
 */
.equiv TCR_T0SZ_39BIT,         25
.equiv TCR_IRGN0_WBWA,         (0b01 << 8)
.equiv TCR_ORGN0_WBWA,         (0b01 << 10)
.equiv TCR_SH0_INNER,          (0b11 << 12)
.equiv TCR_TG0_4K,             (0b00 << 14)

.equiv TCR_T1SZ_39BIT,         (25 << 16)
.equiv TCR_IRGN1_WBWA,         (0b01 << 24)
.equiv TCR_ORGN1_WBWA,         (0b01 << 26)
.equiv TCR_SH1_INNER,          (0b11 << 28)
.equiv TCR_TG1_4K,             (0b10 << 30)

.equiv TCR_IPS_4GB,            (0b000 << 32)

.equiv TCR_EL1_BOOT_VALUE,     (TCR_T0SZ_39BIT | TCR_IRGN0_WBWA | TCR_ORGN0_WBWA | TCR_SH0_INNER | TCR_TG0_4K | TCR_T1SZ_39BIT | TCR_IRGN1_WBWA | TCR_ORGN1_WBWA | TCR_SH1_INNER | TCR_TG1_4K | TCR_IPS_4GB)

/* -------------------------------------------------------------------------- */
/* Temporary L1 block descriptor                                               */
/* -------------------------------------------------------------------------- */

/*
 * With 4KiB granule and 39-bit VA, one level-1 block maps 1GiB.
 *
 * L1 index 0 is used for:
 *
 *   TTBR0 low device window:
 *     VA 0x0000000000000000 -> PA 0x0000000000000000
 *
 *   This gives the early kernel a coarse 0..1GiB virtual window for low
 *   physical MMIO before the final device mappings exist. It is mapped as
 *   Device-nGnRnE and privileged-never-execute.
 *
 * L1 index 1 is used for:
 *
 *   TTBR0 low identity alias:
 *     VA 0x0000000040000000 -> PA 0x0000000040000000
 *
 *   TTBR1 higher-half alias:
 *     VA 0xffffff8040000000 -> PA 0x0000000040000000
 *
 * This dual use works because both virtual addresses resolve to L1 index 1
 * within their respective translation tables.
 */
.equiv LOW_DEVICE_BLOCK_BASE,   0x00000000
.equiv KERNEL_PHYS_BLOCK_BASE,  0x40000000

.equiv DESC_TYPE_BLOCK,        0b01
.equiv DESC_ATTRIDX_NORMAL,    (MAIR_IDX_NORMAL << 2)
.equiv DESC_ATTRIDX_DEVICE,    (MAIR_IDX_DEVICE << 2)
.equiv DESC_AP_EL1_RW,         (0b00 << 6)
.equiv DESC_SH_INNER,          (0b11 << 8)
.equiv DESC_AF,                BIT_10
.equiv DESC_PXN,               BIT_53
.equiv DESC_UXN,               BIT_54

.equiv BOOT_L1_DEVICE_BLOCK,   (LOW_DEVICE_BLOCK_BASE | DESC_PXN | DESC_UXN | DESC_AF | DESC_SH_INNER | DESC_AP_EL1_RW | DESC_ATTRIDX_DEVICE | DESC_TYPE_BLOCK)
.equiv BOOT_L1_NORMAL_BLOCK,   (KERNEL_PHYS_BLOCK_BASE | DESC_UXN | DESC_AF | DESC_SH_INNER | DESC_AP_EL1_RW | DESC_ATTRIDX_NORMAL | DESC_TYPE_BLOCK)

/* -------------------------------------------------------------------------- */
/* Address helper                                                              */
/* -------------------------------------------------------------------------- */

/*
 * Resolve a linked symbol to its current runtime address.
 *
 * Before MMU enable, the image executes from its physical load address while
 * symbols are linked at higher-half virtual addresses. PC-relative addressing
 * still works because both the current PC and the referenced symbol have the
 * same virtual-to-physical offset.
 */
.macro adr_runtime reg, sym
    adrp    \reg, \sym
    add     \reg, \reg, :lo12:\sym
.endm

/* -------------------------------------------------------------------------- */
/* Entry                                                                       */
/* -------------------------------------------------------------------------- */

_start:
    /*
     * Preserve the firmware boot argument.
     *
     * Commonly this is the physical address of the FDT/DTB. The value is passed
     * unchanged to kernel_main().
     */
    mov     x20, x0

    /*
     * Do not allow asynchronous exceptions during early state normalization.
     * Exception vectors are not installed yet.
     */
    msr     DAIFSet, #0xf

    /*
     * This bootstrap only starts the primary CPU. Secondary CPU bring-up is a
     * later SMP responsibility.
     */
    mrs     x0, mpidr_el1
    and     x0, x0, #MPIDR_AFF0_MASK
    cbnz    x0, boot_park_core

    bl      boot_enter_el1
    bl      boot_install_boot_page_table
    bl      boot_clear_bss
    b       boot_enable_mmu_and_enter_rust

.size _start, . - _start

/* -------------------------------------------------------------------------- */
/* EL normalization                                                            */
/* -------------------------------------------------------------------------- */

.type boot_enter_el1, %function
boot_enter_el1:
    mrs     x0, CurrentEL

    cmp     x0, #CURRENT_EL2
    b.eq    boot_drop_el2_to_el1

    cmp     x0, #CURRENT_EL1
    b.eq    boot_reenter_el1_with_known_pstate

    /*
     * EL3 is intentionally unsupported here.
     * Use firmware to enter Non-secure EL2 or EL1.
     */
    b       boot_park_core

.size boot_enter_el1, . - boot_enter_el1

.type boot_reenter_el1_with_known_pstate, %function
boot_reenter_el1_with_known_pstate:
    /*
     * Use SP_EL1 while running at EL1.
     */
    msr     SPSel, #1

    /*
     * Normalize EL1 control state. MMU and caches remain disabled.
     */
    ldr     x0, =SCTLR_EL1_BOOT_INIT
    msr     sctlr_el1, x0
    isb

    /*
     * Synthetic exception return to obtain a known EL1 PSTATE.
     */
    msr     elr_el1, lr
    mov     x0, #INIT_PSTATE_EL1
    msr     spsr_el1, x0
    eret

.size boot_reenter_el1_with_known_pstate, . - boot_reenter_el1_with_known_pstate

.type boot_drop_el2_to_el1, %function
boot_drop_el2_to_el1:
    /*
     * Use SP_EL2 while still executing at EL2.
     */
    msr     SPSel, #1

    /*
     * The kernel runs at EL1 in AArch64 state.
     */
    mov     x0, #HCR_EL2_RW
    msr     hcr_el2, x0
    isb

    /*
     * Allow EL1 to access the physical timer and counter.
     */
    mov     x0, #CNTHCTL_EL2_EL1_TIMER_ACCESS
    msr     cnthctl_el2, x0
    msr     cntvoff_el2, xzr

    /*
     * Program EL1 SCTLR before entering EL1.
     */
    ldr     x0, =SCTLR_EL1_BOOT_INIT
    msr     sctlr_el1, x0
    isb

    /*
     * Return to the caller at EL1h with D/A/I/F masked.
     */
    msr     elr_el2, lr
    mov     x0, #INIT_PSTATE_EL1
    msr     spsr_el2, x0
    eret

.size boot_drop_el2_to_el1, . - boot_drop_el2_to_el1

/* -------------------------------------------------------------------------- */
/* Temporary MMU setup                                                         */
/* -------------------------------------------------------------------------- */

.type boot_install_boot_page_table, %function
boot_install_boot_page_table:
    /*
     * Install the same temporary L1 table into TTBR0 and TTBR1.
     *
     * TTBR0 provides the low identity alias.
     * TTBR1 provides the higher-half alias.
     */
    adr_runtime x0, __boot_l1_page_table
    msr     ttbr0_el1, x0
    msr     ttbr1_el1, x0

    ldr     x0, =MAIR_EL1_BOOT_VALUE
    msr     mair_el1, x0

    ldr     x0, =TCR_EL1_BOOT_VALUE
    msr     tcr_el1, x0

    isb

    /*
     * Remove stale EL1 stage-1 translations before enabling the MMU.
     */
    tlbi    vmalle1
    dsb     sy
    isb

    ret

.size boot_install_boot_page_table, . - boot_install_boot_page_table

/* -------------------------------------------------------------------------- */
/* BSS clearing                                                                */
/* -------------------------------------------------------------------------- */

.type boot_clear_bss, %function
boot_clear_bss:
    adr_runtime x0, __bss_start
    adr_runtime x1, __bss_end

.Lclear_bss_loop:
    cmp     x0, x1
    b.hs    .Lclear_bss_done

    str     xzr, [x0], #8
    b       .Lclear_bss_loop

.Lclear_bss_done:
    ret

.size boot_clear_bss, . - boot_clear_bss

/* -------------------------------------------------------------------------- */
/* Enable MMU and enter Rust                                                   */
/* -------------------------------------------------------------------------- */

.type boot_enable_mmu_and_enter_rust, %function
boot_enable_mmu_and_enter_rust:
    /*
     * Enable only the MMU.
     *
     * Data and instruction caches remain disabled here. They should be enabled
     * later after final page tables and cache maintenance are implemented.
     */
    mrs     x0, sctlr_el1
    orr     x0, x0, #SCTLR_EL1_M
    msr     sctlr_el1, x0
    isb

    /*
     * Low identity mapping still covers the currently executing code.
     * Higher-half mapping is also available now.
     *
     * Switch to the higher-half boot stack before entering the kernel.
     */
    ldr     x0, =__boot_stack_top
    mov     sp, x0

    /*
     * kernel_main(boot_arg: usize) -> !
     */
    mov     x0, x20
    ldr     x8, =kernel_main
    br      x8

.size boot_enable_mmu_and_enter_rust, . - boot_enable_mmu_and_enter_rust

/* -------------------------------------------------------------------------- */
/* Core parking                                                                */
/* -------------------------------------------------------------------------- */

.type boot_park_core, %function
boot_park_core:
.Lpark_core_loop:
    wfe
    b       .Lpark_core_loop

.size boot_park_core, . - boot_park_core

/*
 * Keep literal pools generated by `ldr xN, =...` close to the bootstrap code.
 */
.ltorg

/* -------------------------------------------------------------------------- */
/* Temporary boot page table                                                   */
/* -------------------------------------------------------------------------- */

.section .data.boot, "aw"
.balign 4096

.global __boot_l1_page_table
.type __boot_l1_page_table, %object

__boot_l1_page_table:
    /*
     * L1 index 0:
     *   0x00000000..0x3fffffff -> 0x00000000..0x3fffffff
     *
     * This is a temporary low device window for early boot. It allows the
     * kernel to access low physical MMIO before the kernel installs final page
     * tables with precise per-device attributes and permissions.
     *
     * Attribute index:
     *   Device-nGnRnE
     *
     * Execution:
     *   PXN and UXN are set. This range is data/device access only.
     */
    .quad   BOOT_L1_DEVICE_BLOCK

    /*
     * L1 index 1:
     *   0x40000000..0x7fffffff -> 0x40000000..0x7fffffff
     *
     * This normal-memory block is used twice because the same L1 page table is
     * installed into TTBR0_EL1 and TTBR1_EL1:
     *
     *   - TTBR0 uses it as a low identity mapping while control flow crosses
     *     the MMU enable boundary.
     *   - TTBR1 uses it as the higher-half kernel mapping whose virtual base
     *     is defined in linker.ld.
     */
    .quad   BOOT_L1_NORMAL_BLOCK

    /*
     * Remaining 510 entries, indices 2..511:
     *   invalid.
     *
     * A 4KiB L1 table contains 512 entries. The two entries above consume
     * indices 0 and 1. `.rept 510` emits the remaining 510 eight-byte invalid
     * descriptors so the table is exactly one page.
     *
     * The `.size` directive below does not allocate storage and does not
     * conflict with `.rept`. It records the byte size of the symbol for object
     * metadata/debugging.
     */
    .rept   510
        .quad 0
    .endr

.size __boot_l1_page_table, . - __boot_l1_page_table
