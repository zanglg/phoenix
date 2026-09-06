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
10. allocates and materializes the final permission-separated TTBR1 hierarchy from the same
    reservation-aware allocator;
11. drops the bootstrap-memory capability, then permanently stores the user owner, kernel-table
    owner, file table, and remaining allocator before either translation root is published;
12. emits `PHOENIX_INIT_KERNEL_MAP_ENTER`, publishes TTBR1, and emits
    `PHOENIX_INIT_KERNEL_MAP_OK` only after execution continues through the final map;
13. prints the planned entry, stack pointer, page count, table root, and `PHOENIX_INIT_ENTER`;
14. allocates nonzero ASID 1, publishes it with TTBR0, performs conservative all-ASID invalidation,
    loads EL0 registers, and executes `eret`;
15. validates and copies the init message from owned physical frames for bounded stdout `write`;
16. opens `etc/motd`, copies its bytes into a writable stack buffer, checks EOF, and closes it;
17. writes those copied bytes, transitions its process control to `Exited(42)`, and accepts native
    `exit(42)` as `PHOENIX_INIT_OK` only after output;
    any other terminal state emits `PHOENIX_INIT_FAIL`.

The user program independently checks its aligned stack pointer, argc, argv/envp pointers and
strings, both list terminators, page-size and entry auxiliary entries, Phoenix ABI revision, and
auxiliary terminator, all file-I/O returns, and exact EOF behavior. It emits
`Phoenix init: hello from EL0\n` followed by `Phoenix initramfs: file I/O works\n` before choosing
the success status.

## Ownership and ordering invariants

- all executable and stack frames come from the DTB-derived allocator after mandatory boot
  reservations;
- the archive is completely validated before `init` is selected, and its ELF plus initial-stack
  bytes remain borrowed until every destination page has been cleared and populated;
- no user-table root is published before all leaf bytes and all table descriptors are complete;
- a one-time static runtime retains the address-space owner, final kernel tables, file table, and
  remaining allocator state forever;
- the bootstrap-memory capability has an explicit destructor and is revoked before TTBR1 changes;
- final kernel tables cover every allocator frame, but active user-copy backends additionally
  reject any frame not owned by the retained user address space before an unsafe direct-map copy;
- the activation API requires a `'static` borrow of that complete owner;
- executable pages are user RX and never writable; stack pages are user RW and never executable;
- the guard page has no mapping;
- the final TTBR1 map also omits the linker-reserved page below the live boot-construction stack;
- table writes precede TTBR0 publication through `DSB ISHST`, followed by the documented
  invalidation and synchronization sequence;
- the current bootstrap keeps instruction and data caches disabled, so copied code does not yet
  require an active cache-maintenance sequence;
- user copy resolves virtual ranges only through retained resident ownership and the checked
  physical backend, never by dereferencing a raw EL0 pointer;
- file reads use a window/commit transaction, so a failed `copy_to_user` does not advance offset;
- ASID 1, its monotonic allocator, and the active runtime remain live forever because the terminal
  probe halts rather than returning or reclaiming ownership.
- the runtime control must reach `Ready` before final publication, `Running` before `eret`, and
  `Exited` exactly once before terminal success.

## Fixed capacities

This probe deliberately uses compile-time bounds: 16 normalized memory ranges, two logical image
mappings, five resident image pages, five user-table pages, five user leaves, eight kernel-table
pages, 1024 kernel leaves, and 512 bytes for initial stack construction. Capacity exhaustion is a
reported boot panic, never truncation. These are probe bounds, not stable process limits.

The linked boot-construction stack is separately reserved at 512 KiB. Static disassembly of the
current debug loaded-init artifact shows a `0x4cd20`-byte frame for the construction path, leaving
more than 200 KiB for callers and interrupt-free bootstrap execution. This observation is not yet
an automated upper-bound check and must be repeated after material changes to the plan types.

## Validation

Host tests cover the constituent ELF parser, process-image transaction and retry behavior, initial
stack, allocator, dynamic table topology/materialization, combined ownership, bounded write
validation, bidirectional cross-page user copying, file-table transactions, syscall decoding, and
QEMU terminal-output classification.
Target compilation validates the complete concrete composition, `include_bytes` artifact handoff,
bootstrap and ownership-checked final direct-map backends, both activation sequences, and the
exception dispatcher. Static inspection proves that the built kernel contains the exact initramfs
whose `init` entry was accepted by the loader plus every required translation progress sentinel.

Runtime validation is `RUN-INIT-001`. Success requires `cargo xtask test-init` to observe, in order,
the final-map enter marker, post-switch marker, EL0 enter marker, both exact userspace lines, and
`PHOENIX_INIT_OK`. A success marker without that exact ordered sequence is a protocol failure.
Panic, exception, `PHOENIX_INIT_FAIL`, early QEMU exit, timeout, excessive output, and stream
errors all fail.

## TODO

- run `cargo xtask test-init` on the pinned QEMU environment and retain the full evidence;
- verify TTBR0, ELR, SP, SPSR, descriptor permissions, and the guard fault under GDB;
- introduce explicit instruction-cache synchronization before enabling caches;
- runtime-validate the integrated final TTBR1 switch and its ownership-checked user copies;
- replace the single retained process control with a table owner and scheduler-context reap that
  can retire the ASID and reclaim every frame;
- accept a checked initramfs supplied independently by firmware or a bootloader instead of
  compile-time embedding;
- replace fixed probe capacities with accounted process limits where dynamic scale is needed;
- automate a static early-stack footprint bound before reducing the 512 KiB construction stack;
- replace bounded probe I/O with process-owned descriptors and general fault policy.

## Skipped work

This path is not a scheduler, process table, general `exec`, mounted initramfs filesystem, VFS,
general file-descriptor model, general console runtime, dynamic linker, TLS implementation, ASLR,
demand pager, teardown, or production init. It keeps caches, interrupts, timers, and SMP disabled.
Its purpose is to make the earliest real user-program handoff complete and reviewable without
hiding unfinished system facilities behind a successful marker.
