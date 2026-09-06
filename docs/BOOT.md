# AArch64 Boot Contract

This document records the current, intentionally small boot contract. It describes what the
source and ELF layout require; it is not evidence that the image has run on an emulator.

## Image and entry

- Rust target: `aarch64-unknown-none-softfloat`.
- Initial platform: QEMU AArch64 `virt` using direct raw-kernel loading.
- Raw image physical load address: `0x40080000`.
- Linked virtual entry: `0xffffff8040080000` at `_start`.
- The entry CPU must already be in AArch64 state at EL1 or non-secure EL2. EL3 is unsupported.
- Asynchronous exceptions are masked because exception vectors do not exist yet.
- `x0` is preserved and passed to `kernel_main`; it is expected to contain the physical DTB
  address, but Phoenix does not parse it yet.
- Only a CPU whose `MPIDR_EL1.Aff0` is zero proceeds. Other CPUs remain in a `wfe` loop.

## Temporary execution environment

The bootstrap installs one 4 KiB L1 table in both TTBR0_EL1 and TTBR1_EL1 for a 39-bit, 4 KiB
translation regime:

- low `0..1 GiB` maps to the same physical range as Device-nGnRnE for early MMIO;
- low `1..2 GiB` maps to the same physical range as normal memory;
- `0xffffff8040000000..0xffffff807fffffff` maps physical
  `0x40000000..0x7fffffff` as normal memory.

The MMU is enabled before entering Rust. Instruction and data caches, FP/SIMD, interrupts, and
final permission-separated page tables remain disabled or deferred. This coarse mapping exists
only for early boot.

The linker reserves a zeroed BSS followed by a 64 KiB, 16-byte-aligned boot stack. Rust receives
the higher-half stack address. QEMU `virt` PL011 registers are accessed through the low device
mapping at physical/virtual address `0x09000000`.

## Observable output

On the intended platform, `kernel_main` should print the Phoenix build identity, the boot
argument, and `PHOENIX_BOOT_OK`, then flush PL011 and wait. A Rust panic should print
`PHOENIX_PANIC`, the available panic information, flush, and wait.

These strings define the future smoke-test protocol. They have not yet been observed on QEMU.

## Current validation

```bash
cargo xtask build
cargo xtask inspect
```

These commands build and statically verify the ELF machine type, entry address, first physical
and virtual load addresses, required boot symbols, page-table alignment, BSS ordering, stack
size/alignment, raw image, and linker map. `cargo xtask ci` includes both commands.

## Open runtime decisions

- exact QEMU `virt` machine version and CPU model to pin;
- the verified QEMU command line;
- timeout and process-exit mechanism for automated smoke tests;
- observed EL1 and EL2 entry behavior;
- whether direct boot supplies every assumed register and cache state.

Resolve these from real emulator evidence before adding `run`, `debug`, or a boot-test command.
