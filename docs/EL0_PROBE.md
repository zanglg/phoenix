# First EL0 Conformance Probe

This document describes the opt-in `el0-probe` kernel feature. It provides the shortest statically
verifiable path through user translation, `eret`, `SVC`, exception return, and process exit. The
probe has not executed on QEMU and is therefore Runtime Pending.

## Purpose and activation

The default kernel still prints `PHOENIX_BOOT_OK` and halts. `cargo xtask test-el0` builds the same
kernel with `--features el0-probe`, statically inspects the extra image layout, and then expects
`PHOENIX_EL0_OK` from QEMU. It is separate from `test-boot`, so seeing the earlier boot marker
cannot accidentally pass the EL0 test.

Emulator-free `cargo xtask ci` also links and inspects the feature, then rebuilds and inspects the
default image. It never executes the probe.

## Static user image

The probe is a one-page AArch64 assembly program linked into the kernel image. It is not presented
as an ELF loader result. Its fixed virtual layout is:

| User virtual range | Mapping |
| --- | --- |
| `0x00400000..0x00401000` | Probe code, EL0 read/execute, EL1 read-only, non-global |
| `0x00800000..0x00801000` | Unmapped guard page |
| `0x00801000..0x00802000` | Zeroed stack, EL0/EL1 read/write, never execute, non-global |

Four aligned static table pages form TTBR0 L1, shared L2, code L3, and stack L3 tables. The setup
uses the checked descriptor constructors and converts linked higher-half resources to physical
addresses using the documented kernel offset. The tables and stack are cleared under exclusive
single-CPU ownership before publication.

Table writes are followed by the same crate-private activation primitive used by allocator-backed
address spaces: `DSB ISHST`, replacement of `TTBR0_EL1` with fixed nonzero ASID 1, `ISB`,
`TLBI VMALLE1`, `DSB ISH`, and `ISB`. The transition sets `SP_EL0`, `ELR_EL1`, and `SPSR_EL1` for
EL0t with asynchronous exceptions masked, then executes `eret`. The installed TTBR1 higher-half
mapping remains the kernel address space. The early console now uses its higher-half PL011 alias,
so removing the bootstrap TTBR0 identity/device table does not strand diagnostics.

## Probe protocol

The user program performs:

1. system call 99, which is unassigned;
2. verify that the exception dispatcher returns `-NotImplemented` in `x0` through `eret`;
3. call `exit(42)`;
4. branch to a failure `exit(1)` if the return value was wrong.

Only lower-AArch64 synchronous exceptions with class `SupervisorCallAArch64` and immediate zero
enter this probe dispatcher. Unknown and not-yet-implemented `write` calls return
`-NotImplemented`. `exit(42)` prints `PHOENIX_EL0_OK` and halts; another status prints
`PHOENIX_EL0_FAIL`. Other exceptions retain the fatal diagnostic path.

This deliberately validates a returning syscall before the terminal exit call. It does not claim
process teardown, scheduling, ELF loading, or general syscall support.

## Invariants

- the feature is opt-in and cannot change default boot behavior;
- user code and stack occupy exactly one page each and are ELF Inspected for alignment and size;
- the guard page has no descriptor;
- EL0 cannot access any TTBR1 kernel or device mapping;
- user code is never writable through its EL0 mapping and user stack is never executable;
- the user stack is cleared before EL0 access;
- page tables are private until all descriptors are complete;
- exception vectors and the EL1 boot stack exist before `eret`;
- the EL0 test ignores `PHOENIX_BOOT_OK` and requires its own final marker.

## Current validation

- Host Tested: descriptors, virtual indices, syscall number/arguments/return values, exception-frame
  adaptation, QEMU command construction, and EL0 marker classification;
- Cross Compiled: all feature-gated Rust and assembly;
- ELF Inspected: user entry equals the start of one aligned code page and the stack is one aligned
  page;
- disassembly inspected during development: the probe contains the expected two `SVC #0` paths;
- Runtime Pending: every CPU, translation, exception, and marker behavior above.

## TODO

- execute `cargo xtask test-el0` and record the complete transcript;
- verify the EL0 exception frame, SPSR mode, PC, TTBR0 root, and user permissions under GDB;
- replace `VMALLE1` with an exact ASID-aware invalidation policy before identifiers are reused;
- install a permission-separated final TTBR1 so the kernel alias is no longer a coarse RWX block;
- enable instruction/data caches only after the required coherency sequence is defined;
- replace the linked assembly page with an ELF image populated through allocator-owned frames;
- use the general guarded-stack planner rather than this fixed one-page layout;
- turn successful `exit` into process teardown and a scheduler handoff;
- add fault probes for kernel access, guard-page access, execution from stack, and bad SVC immediate.

## Skipped work

The probe intentionally skips DTB-to-allocator integration, dynamic page-table allocation, ELF
population, argv/envp/auxv, user-copy helpers, `write`, VFS, process objects, scheduling, timers,
signals, SMP, caches, and guest-driven QEMU exit. It is a conformance bridge, not the production
userspace implementation.
