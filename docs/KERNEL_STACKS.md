# Kernel Stacks

This document describes the stack protection that Phoenix currently implements and the larger
kernel-stack policy that remains to be built. The implemented part applies only after the final
AArch64 `TTBR1_EL1` hierarchy has been installed.

## Current implementation

The linker reserves this ordered physical and higher-half virtual layout inside the writable
kernel image:

```text
page-aligned BSS end
optional alignment padding
4 KiB boot-stack guard (unmapped by final TTBR1)
512 KiB boot-construction stack (RW, PXN, UXN)
optional statically linked probe storage
kernel image end
```

`__boot_stack_guard_start` and `__boot_stack_guard_end` describe exactly one page, and the linker
requires the guard end to equal `__boot_stack_bottom`. The complete image, including the guard's
physical frame, remains reserved from the physical allocator. Reserving a frame and mapping it are
separate decisions: the final direct-map planner deliberately emits no descriptor for the guard.

The bootstrap's initial coarse mappings still cover this page. The guard becomes effective only
when `kernel-map-probe` or `loaded-init-probe` publishes the final TTBR1 hierarchy. Both paths keep
executing on the higher-half boot stack across that switch. The loaded-init path subsequently
replaces TTBR0 as well, so it retains no ordinary low-address alias to the guard frame.

The stack is intentionally large because the current allocation-free debug path constructs fixed
page-table and process-image plans on the stack. Static disassembly most recently showed a
`0x4cd20`-byte loaded-init construction frame. The 512 KiB reservation leaves more than 200 KiB
for callers, but this is an observed value rather than an automated compiler-enforced bound.

## Invariants

- the guard is exactly one 4 KiB frame and immediately precedes the downward-growing stack;
- the guard frame belongs to the reserved kernel image and can never be returned by the boot
  allocator;
- the final offline translation model must resolve the guard virtual address to no mapping;
- the first and last usable stack frames remain kernel-only, writable, and non-executable;
- the stack and guard layout must fit inside the linker's first-2-MiB permission window;
- static artifact inspection rejects missing, misaligned, reordered, or wrongly sized symbols;
- no current code intentionally probes the guard until recoverable kernel-fault testing exists.

## Validation completed without QEMU

Host tests verify that direct-map overrides can leave an exact page hole without losing adjacent
permissions or large mappings. Platform tests verify that the QEMU `virt` final plan leaves the
declared guard absent while retaining text, rodata, data, and PL011 mappings. Every AArch64 kernel
variant is cross-linked and inspected for the two guard symbols, their exact size and adjacency,
the 512 KiB stack size, and the overall image bound.

This does not prove that an AArch64 translation fault occurs on stack underflow. That evidence is
tracked as `RUN-KSTACK-001` in `docs/RUNTIME_VALIDATION.md`.

## TODO

- execute a controlled guard access after final TTBR1 publication and confirm the expected EL1
  data abort without relying on a corrupted stack;
- automate a conservative early-stack footprint upper bound for supported build profiles;
- move bulky boot plans into explicitly owned scratch memory so the temporary stack can shrink;
- define a small per-thread stack size, allocation owner, lifetime, and mandatory lower guard;
- define a separate exception-stack policy before asynchronous interrupts or recoverable faults;
- switch stacks safely during thread creation, context switching, teardown, and CPU-local idle;
- decide whether an upper guard is also required and how overflow diagnostics find stack owners;
- add stack canaries or high-water accounting only after their failure and concurrency semantics
  are specified.

## Skipped work

Phoenix does not yet allocate runtime kernel stacks, switch between kernel threads, recover from
kernel stack faults, maintain per-CPU exception stacks, unwind stack traces, randomize stack
locations, or reclaim stack mappings. The one implemented guard must not be described as a
complete kernel-stack subsystem.
