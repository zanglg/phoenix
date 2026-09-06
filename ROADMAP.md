# Phoenix Development Path

Phoenix is developed as one ordered stream until the first formal release. There are no numbered
milestones and no intermediate release tags. The Cargo version remains `0.0.0` throughout this
period.

Work follows the order below. A runtime check may remain pending when its environment is
unavailable, while the next explicitly host-verifiable work proceeds without assuming that check
passed. The broader, unscheduled capability surface remains in `FEATURES.md`.

## Current position

The engineering foundation and **Host-verifiable architecture foundations** are complete.
**Bootstrap and early console** remains Runtime Pending because emulator validation is outstanding.
The active no-emulator work is **Exceptions and diagnostics**.

## Completed foundation

- stable Rust toolchain policy, Rust 1.95 MSRV, and AArch64 bare-metal target;
- configurable AArch64 rust-analyzer support from an x86_64 host;
- minimal `no_std` kernel crate;
- host-side xtask validation;
- version, feature, Roadmap, Changelog, and Agent policies.

## 1. Bootstrap and early console

Integrate the existing AArch64 bootstrap, define the linker layout and stack, enter Rust, print a
build identity and deterministic success sentinel through PL011, then halt safely.

Completed without an emulator:

- bootstrap assembly is integrated through the AArch64 kernel binary;
- the higher-half linker layout, low physical load address, BSS, and 64 KiB boot stack are fixed;
- early PL011 output, panic output, build identity, and deterministic sentinels are implemented;
- the ELF, raw image, linker map, and required symbols are checked automatically.

Remaining runtime validation:

- observe the success and panic paths on a compatible QEMU environment;
- execute the implemented bounded runtime smoke test and reconcile its assumptions with evidence.

Observable result: one command builds and boots Phoenix on AArch64 QEMU `virt`; an automated test
passes only after observing the success sentinel before its timeout.

The current boot contract is in `docs/BOOT.md`. QEMU machine version and runtime test-exit
protocol remain deliberately open until emulator validation is available.

## 2. Host-verifiable architecture foundations

Build the pure and statically inspectable foundations needed by later runtime integration, in
this order:

- [x] checked physical/virtual addresses, pages, frames, and ranges;
- [x] strict read-only DTB parsing and boot-information extraction;
- [x] physical-memory region normalization, reservation, and frame allocation;
- [x] AArch64 translation indices, descriptor construction, and offline mapping plans;
- [x] exception-frame layout, vector-table layout, and syndrome decoding.

Each mechanism must be Host Tested where behavior is pure, Cross Compiled for AArch64, and kept
independent of unverified MMIO or system-register effects. This section does not install final
translation tables, write `VBAR_EL1`, enable interrupts, or claim target execution.

Observable result: malformed inputs and boundary conditions are covered by host tests, while the
AArch64 artifact retains all required static layout checks.

## 3. Exceptions and diagnostics

Install exception vectors, preserve a complete register context, classify faults, and produce a
useful panic report with symbols or enough addresses for offline symbolization.

Completed without an emulator:

- all vector slots, the exception-frame ABI, ESR and data-abort decoding are Host Tested;
- the linked vector table saves/restores complete integer state and is installed in `VBAR_EL1`;
- ELF inspection enforces the table's 2 KiB size and alignment;
- the fatal dispatcher emits a stable exception sentinel plus vector, PC, status, ESR, and FAR.

Remaining:

- execute deliberate exception probes and verify register preservation on QEMU;
- add offline symbolization and a reliable stack trace;
- define recoverable EL0 faults and syscall dispatch before returning from exceptions.

Observable result: deliberate synchronous exceptions are caught and reported deterministically
instead of silently hanging QEMU.

## 4. Memory management

Discover and reserve physical memory, add a page-frame allocator, establish the kernel virtual
address space, install final page tables, and provide a guarded kernel heap and stacks. The
descriptor and offline mapping model is Host Tested; target work still needs to allocate table
frames, materialize the permission-separated layout, install it, retire temporary aliases, and
validate the barrier and TLB-maintenance sequence.

Observable result: allocator and mapping self-tests exercise success and failure paths without
corrupting the bootstrap, DTB, image, or device mappings.

The Host Tested boot-map builder now normalizes DTB memory and removes firmware reservations, the
loaded kernel, and the borrowed DTB before freezing the allocator. A separate Cross Compiled probe
connects the real boot argument and linker symbols, allocates and round-trips one private frame,
and restores ownership; its target execution remains Runtime Pending.

## 5. Interrupts and time

Initialize the GIC and generic timer, define interrupt-context rules, dispatch IRQs, and provide a
monotonic clock plus timer queue.

Observable result: timer interrupts advance a monotonic time source and scheduled callbacks fire
under an automated QEMU test.

## 6. Kernel execution

Introduce synchronization primitives, kernel threads, architecture context switching, idle,
scheduling, and preemption rules. Interfaces must already account for future SMP ordering even
though the first release remains single-core.

Observable result: multiple kernel threads make independently verified progress under timer-driven
scheduling.

## 7. Processes and native ABI

Add user address spaces, safe user-memory access, process and thread lifecycle, transition to
userspace, and a small documented native Phoenix syscall ABI.

Host-verifiable prerequisites now include lower-39-bit user mapping and guarded-stack plans plus
the revision-0 AArch64 register and return-value convention. An opt-in statically backed probe now
links an EL0 entry, replaces TTBR0, verifies one returning unknown syscall, and terminates through
`exit(42)`. Runtime evidence, target-memory access, address-space activation, safe user copy, and
process lifecycle remain.

Allocator-owned user-table topology, retry-safe descriptor materialization, and an exact combined
owner retaining populated leaf frames plus table frames are now Host Tested. The target
bootstrap-memory backend and ownership-consuming ASID-zero activation are Cross Compiled; process
ownership, ASID allocation, retirement, safe user copy, and runtime evidence still remain.

Observable result: an EL0 program invokes syscalls, exits, and cannot directly access kernel
memory.

## 8. Executables and minimal system

Load ELF programs from initramfs, define the initial user stack and auxiliary data, add a minimal
VFS and file-descriptor model, and start an `init` program with console input and output.

The strict ELF64/AArch64 validation layer and the combined program/guarded-stack page plan are Host
Tested. Frame ownership is assigned and released transactionally, complete page zero/copy is
enforced through an explicit populated type state, and the native argc/argv/envp/minimal-auxv stack
is included with 16-byte alignment. The QEMU bootstrap backend can perform target writes while its
coarse high alias remains active. Activation, initramfs ownership, and executable entry remain.

The first standalone AArch64 `init` ELF is reproducibly linked, stripped for embedding, accepted
by the real loader, and inspected as one RX page. It validates the current startup stack and exits
through the native ABI; connecting it to the dynamic ownership path remains.

Observable result: Phoenix boots from a clean checkout, starts `init`, runs at least one child
program, performs console and in-memory file I/O, and shuts down or reports test completion.

## 9. Release preparation

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
