# Phoenix Development Path

Phoenix is developed as one ordered stream until the first formal release. There are no numbered
milestones and no intermediate release tags. The Cargo version remains `0.0.0` throughout this
period.

Only the first incomplete section is active. Later sections define dependency order, not parallel
work or permission to implement everything at once. The broader, unscheduled capability surface
remains in `FEATURES.md`.

## Current position

The engineering foundation is complete. The next work is **Bootstrap and early console**.

## Completed foundation

- stable Rust toolchain policy, Rust 1.95 MSRV, and AArch64 bare-metal target;
- configurable AArch64 rust-analyzer support from an x86_64 host;
- minimal `no_std` kernel crate;
- host-side xtask validation;
- version, feature, Roadmap, Changelog, and Agent policies.

## 1. Bootstrap and early console

Integrate the existing AArch64 bootstrap, define the linker layout and stack, enter Rust, print a
build identity and deterministic success sentinel through PL011, then halt safely.

Observable result: one command builds and boots Phoenix on AArch64 QEMU `virt`; an automated test
passes only after observing the success sentinel before its timeout.

Before coding, freeze the image format, load address, entry state, DTB convention, UART source,
CPU model, QEMU machine version, panic output, and test-exit protocol.

## 2. Exceptions and diagnostics

Install exception vectors, preserve a complete register context, classify faults, and produce a
useful panic report with symbols or enough addresses for offline symbolization.

Observable result: deliberate synchronous exceptions are caught and reported deterministically
instead of silently hanging QEMU.

## 3. Memory management

Discover and reserve physical memory, add a page-frame allocator, establish the kernel virtual
address space, install final page tables, and provide a guarded kernel heap and stacks.

Observable result: allocator and mapping self-tests exercise success and failure paths without
corrupting the bootstrap, DTB, image, or device mappings.

## 4. Interrupts and time

Initialize the GIC and generic timer, define interrupt-context rules, dispatch IRQs, and provide a
monotonic clock plus timer queue.

Observable result: timer interrupts advance a monotonic time source and scheduled callbacks fire
under an automated QEMU test.

## 5. Kernel execution

Introduce synchronization primitives, kernel threads, architecture context switching, idle,
scheduling, and preemption rules. Interfaces must already account for future SMP ordering even
though the first release remains single-core.

Observable result: multiple kernel threads make independently verified progress under timer-driven
scheduling.

## 6. Processes and native ABI

Add user address spaces, safe user-memory access, process and thread lifecycle, transition to
userspace, and a small documented native Phoenix syscall ABI.

Observable result: an EL0 program invokes syscalls, exits, and cannot directly access kernel
memory.

## 7. Executables and minimal system

Load ELF programs from initramfs, define the initial user stack and auxiliary data, add a minimal
VFS and file-descriptor model, and start an `init` program with console input and output.

Observable result: Phoenix boots from a clean checkout, starts `init`, runs at least one child
program, performs console and in-memory file I/O, and shuts down or reports test completion.

## 8. Release preparation

Remove undocumented failure paths, verify resource-exhaustion behavior, audit unsafe invariants,
stabilize the documented 0.1 interface boundary, and produce reproducible artifacts and an
end-to-end test report.

Only after all 0.1.0 release criteria pass:

1. change the project version from `0.0.0` to `0.1.0`;
2. move accumulated Changelog entries from `Unreleased` to `0.1.0`;
3. create the first annotated tag, `v0.1.0`.

## 0.1.0 release boundary

The first release is intentionally narrow. It supports only AArch64 QEMU `virt`, one CPU, a
native unstable userspace ABI, initramfs, console I/O, and an in-memory filesystem.

The following are not required for 0.1.0: persistent storage, networking, SMP, loadable modules,
Linux binary or syscall compatibility, POSIX completeness, real hardware, RISC-V, or x86_64.
