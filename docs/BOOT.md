# AArch64 Boot Contract

This document records the current, intentionally small boot contract. It describes what the
source and ELF layout require; it is not evidence that the image has run on an emulator.

## Image and entry

- Rust target: `aarch64-unknown-none-softfloat`.
- Initial platform: QEMU AArch64 `virt` using direct raw-kernel loading.
- Raw image physical load address: `0x40080000`.
- Linked virtual entry: `0xffffff8040080000` at `_start`.
- The entry CPU must already be in AArch64 state at EL1 or non-secure EL2. EL3 is unsupported.
- Stage-1 translation must be disabled at entry. Instruction and data caches must be disabled, or
  firmware must have completed the cache maintenance needed to make the loaded image coherent.
- Asynchronous exceptions are masked during bootstrap; Rust installs vectors before any later
  interrupt source may be enabled.
- `x0` is preserved and passed to `kernel_main` as the physical DTB address. A target helper can
  validate and borrow that blob through the temporary high RAM alias; the default boot path does
  not consume it yet.
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
only for early boot. Rust installs the linked EL1 exception vector table before emitting its first
console line, but keeps asynchronous exceptions masked.

The linker reserves a zeroed BSS followed by a 64 KiB, 16-byte-aligned boot stack. Rust receives
the higher-half stack address. QEMU `virt` PL011 is physically at `0x09000000`, but Rust accesses
its temporary higher-half device alias at `0xffffff8009000000`; diagnostics therefore survive an
opt-in replacement of TTBR0.

## Observable output

On the intended platform, `kernel_main` should print the Phoenix build identity, the boot
argument, and `PHOENIX_BOOT_OK`, then flush PL011 and wait. A Rust panic should print
`PHOENIX_PANIC`, the available panic information, flush, and wait.

These strings define the implemented smoke-test protocol. They have not yet been observed on QEMU.

## Current validation

```bash
cargo xtask build
cargo xtask inspect
cargo xtask qemu-command
```

These commands build and statically verify the ELF machine type, entry address, first physical
and virtual load addresses, required boot symbols, page-table alignment, BSS ordering, stack
size/alignment, raw image, and linker map. `cargo xtask ci` includes both commands.

The bounded `cargo xtask test-boot` runner is documented in `docs/QEMU.md`. It is implemented and
Host Tested at the command-construction and output-classification level, but has not been executed.

## Open runtime decisions

- whether the initial `virt-9.2` and Cortex-A72 pins remain after real compatibility evidence;
- the verified, rather than merely constructed, QEMU command line;
- a guest-driven process-exit mechanism to replace host termination after the sentinel;
- observed EL1 and EL2 entry behavior;
- confirmation that direct boot supplies every assumed register and cache state.

Resolve these from real emulator evidence before adding `run`, `debug`, or a boot-test command.
