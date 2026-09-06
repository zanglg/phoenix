# Dynamically Loaded Init Probe

This document describes the opt-in `loaded-init-probe` kernel feature. It is Phoenix's first
end-to-end construction path from a separately linked ELF file to an owned AArch64 EL0 address
space. The complete path is Cross Compiled and ELF Inspected. It has not run under QEMU, so all
target behavior remains Runtime Pending.

## Current implementation

`cargo xtask test-init` performs a reproducible chain before it starts QEMU:

1. build the independent `phoenix-init` AArch64 executable;
2. remove debug sections and validate the exact resulting bytes with `ElfImage::parse`;
3. create a deterministic uncompressed `newc` archive containing those bytes as executable `init`;
4. validate the complete archive and exact entry through the production initramfs parser;
5. build the kernel with `loaded-init-probe` and pass only that checked archive to the build;
6. require the complete archive and `PHOENIX_INIT_OK` terminal marker in the kernel ELF;
7. inspect the kernel's higher-half load layout, bootstrap symbols, raw image, and linker map;
8. run the bounded QEMU harness and require the loaded-init terminal result.

The emulator-free `cargo xtask ci` performs steps 1 through 7 and never starts QEMU.

At boot, the focused kernel variant:

1. enters through the ordinary higher-half bootstrap and installs the exception vectors;
2. borrows the QEMU-provided DTB and reserves firmware, kernel-image, and DTB physical ranges;
3. validates the embedded initramfs and selects its executable regular file at canonical path
   `init`;
4. parses the selected ELF again with the production allocation-free loader;
5. creates four usable user-stack pages below `DEFAULT_USER_STACK_TOP` and one unmapped guard;
6. builds the native revision-0 stack for `argv=["/init", "phoenix"]` and
   `envp=["TERM=phoenix"]`;
7. atomically allocates and fully populates five image pages: one RX executable page and four RW
   stack pages;
8. allocates, clears, and materializes the required lower-half L1/L2/L3 translation tables;
9. combines populated leaf ownership and table ownership into one prepared address space;
10. prints the planned entry, stack pointer, page count, table root, and `PHOENIX_INIT_ENTER`;
11. publishes TTBR0, invalidates ASID-zero translations, loads EL0 registers, and executes `eret`;
12. accepts native `exit(42)` as `PHOENIX_INIT_OK`; any other exit status emits
    `PHOENIX_INIT_FAIL`.

The user program independently checks its aligned stack pointer, argc, argv/envp pointers and
strings, both list terminators, page-size and entry auxiliary entries, Phoenix ABI revision, and
auxiliary terminator before choosing the success status.

## Ownership and ordering invariants

- all executable and stack frames come from the DTB-derived allocator after mandatory boot
  reservations;
- the archive is completely validated before `init` is selected, and its ELF plus initial-stack
  bytes remain borrowed until every destination page has been cleared and populated;
- no user-table root is published before all leaf bytes and all table descriptors are complete;
- the activation API consumes the one owner containing every leaf and table frame;
- executable pages are user RX and never writable; stack pages are user RW and never executable;
- the guard page has no mapping;
- table writes precede TTBR0 publication through `DSB ISHST`, followed by the documented
  invalidation and synchronization sequence;
- the current bootstrap keeps instruction and data caches disabled, so copied code does not yet
  require an active cache-maintenance sequence;
- ASID zero and the active address-space frames remain live forever because the terminal probe
  halts rather than returning or reclaiming ownership.

## Fixed capacities

This probe deliberately uses compile-time bounds: 16 normalized memory ranges, two logical image
mappings, five resident image pages, five table pages, five leaves, and 512 bytes for initial stack
construction. Capacity exhaustion is a reported boot panic, never truncation. These are probe
bounds, not stable process limits.

## Validation

Host tests cover the constituent ELF parser, process-image transaction and retry behavior, initial
stack, allocator, dynamic table topology/materialization, combined ownership, syscall decoding,
and QEMU terminal-output classification. Target compilation validates the complete concrete
composition, `include_bytes` artifact handoff, target memory backend, activation assembly, and
exception dispatcher. Static inspection proves that the built kernel contains the exact initramfs
whose `init` entry was accepted by the loader.

Runtime validation is `RUN-INIT-001`. Success requires `cargo xtask test-init` to observe
`PHOENIX_INIT_OK`; the preceding boot and entry markers cannot satisfy it. Panic, exception,
`PHOENIX_INIT_FAIL`, early QEMU exit, timeout, excessive output, and stream errors all fail.

## TODO

- run `cargo xtask test-init` on the pinned QEMU environment and retain the full evidence;
- verify TTBR0, ELR, SP, SPSR, descriptor permissions, and the guard fault under GDB;
- introduce explicit instruction-cache synchronization before enabling caches;
- replace the temporary coarse TTBR1 RAM alias with a final permission-separated kernel map and
  documented physical direct map;
- move terminal exit into a process owner that can retire the ASID and reclaim every frame;
- accept a checked initramfs supplied independently by firmware or a bootloader instead of
  compile-time embedding;
- replace fixed probe capacities with accounted process limits where dynamic scale is needed;
- add safe user-copy support before implementing native `write`.

## Skipped work

This path is not a scheduler, process table, general `exec`, mounted initramfs filesystem, VFS,
file-descriptor model, console-using userspace runtime, dynamic linker, TLS implementation, ASLR,
demand pager, teardown, or production init. It keeps caches, interrupts, timers, and SMP disabled.
Its purpose is to make the earliest real user-program handoff complete and reviewable without
hiding unfinished system facilities behind a successful marker.
