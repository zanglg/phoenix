# Runtime Validation Ledger

This ledger prevents compiled or statically inspected code from being mistaken for code that has
executed on the target platform.

## States

- **Runtime Pending**: implementation exists, but no target execution evidence exists.
- **Runtime Verified**: the documented command, environment, expected output, and failure path
  were all exercised successfully.

An entry can become Runtime Verified only by recording the environment, exact command, relevant
output, and source commit. Host tests, cross-compilation, disassembly, and ELF inspection are
valuable evidence but cannot close an entry in this ledger.

## Recorded environment

The first target-validation run used QEMU emulator version 9.1.3 (`v9.1.3`) from
`/Volumes/workspace/qemu/monterey-aarch64/build/qemu-system-aarch64`, on an Apple Silicon host.
The exact board was `virt-9.1,virtualization=on,gic-version=3,highmem=off`, CPU `cortex-a72`,
TCG, 512 MiB RAM, and one CPU. Source identified itself as commit `8792647e370e`, dirty, debug,
with the validation fixes uncommitted. Each bounded command used that directory at the front of
`PATH`, completed inside the documented ten-second timeout, and wrote its transcript to the
generated `qemu-*.log` path documented in `docs/QEMU.md`.

The initial `test-kernel-map` attempt failed before its enter marker. QEMU exception tracing and
disassembly identified a construction-stack overflow: an approximately 305 KiB outer frame plus
an approximately 530 KiB nested planner frame exceeded the former 512 KiB reservation. The
correcting source uses a 1 MiB construction stack and in-place transactional plan updates. The
repeated probe and every other positive probe below passed. This changes neither the 4 KiB page
size nor the 512 MiB QEMU memory model.

## Validation ledger

| ID | Capability | State | Evidence required |
| --- | --- | --- | --- |
| RUN-BOOT-001 | Enter `_start` through QEMU direct kernel boot | Runtime Verified | `cargo xtask test-boot` on the recorded board reached early output and `PHOENIX_BOOT_OK` |
| RUN-BOOT-002 | Normalize EL2 or EL1 state and enter Rust at EL1 | Runtime Pending | Observed EL path and `kernel_main` entry |
| RUN-BOOT-003 | Temporary identity and higher-half mappings | Runtime Verified | The QEMU boot probe continued in higher-half Rust code through `PHOENIX_BOOT_OK` |
| RUN-BOOT-004 | BSS initialization and 1 MiB boot-construction stack | Runtime Pending | Runtime probes confirm zeroed BSS and stack bounds/alignment without crossing the linker-owned range |
| RUN-CONSOLE-001 | QEMU `virt` PL011 output and flush | Runtime Verified | Complete build identity and `PHOENIX_BOOT_OK` were observed |
| RUN-PANIC-001 | Early panic reporting | Runtime Pending | Deliberate panic emits `PHOENIX_PANIC` before timeout |
| RUN-SMOKE-001 | Bounded automated boot smoke test | Runtime Verified | `cargo xtask test-boot` observed `PHOENIX_BOOT_OK` and terminated predictably |
| RUN-DTB-001 | Parse the QEMU-provided DTB in early boot | Runtime Verified | `test-memory` accepted the real DTB and reported size 1,048,576 and boot CPU 0 |
| RUN-MM-001 | Initialize the frame allocator from discovered memory | Runtime Verified | `test-memory` reported 130,530 free frames and allocated, scrubbed, and restored one private frame |
| RUN-MM-002 | Mutate private frames through the bootstrap high RAM alias | Runtime Pending | First/last valid pages clear and copy correctly; out-of-window frames are rejected without a target fault |
| RUN-MM-003 | End-to-end boot memory integration probe | Runtime Verified | `cargo xtask test-memory` reached `PHOENIX_MEMORY_OK` after the documented DTB, reservation, round-trip, and ownership-restoration path |
| RUN-KHEAP-001 | Initialize and exercise the fallible heap through the final direct map | Runtime Pending | The Cross Compiled/ELF Inspected final-map path requires ordered `PHOENIX_KERNEL_MAP_ENTER`, post-TTBR1 four-frame heap initialization, aligned allocation, recoverable byte/metadata exhaustion, release/reuse, `PHOENIX_KERNEL_HEAP_OK`, subsequent `PHOENIX_KERNEL_STACK_OK`, and continued progress to `PHOENIX_KERNEL_MAP_OK`; the earlier final-map run predates this protocol |
| RUN-MMU-001 | Install final permission-separated kernel tables | Runtime Verified | `cargo xtask test-kernel-map` reached ordered `PHOENIX_KERNEL_MAP_ENTER`/`PHOENIX_KERNEL_MAP_OK` through the final text/rodata/data/stack/PL011 mappings; negative permission probes remain separate Pending work |
| RUN-MMU-002 | Retire temporary TTBR0 aliases | Runtime Pending | No bootstrap handle or low pointer remains, TTBR0 is replaced, low RAM/MMIO addresses fault, and higher-half execution survives the maintenance sequence |
| RUN-MMU-003 | Access allocator-managed RAM through the retained final-map capability | Runtime Pending | After final TTBR1 publication, the exact table owner and offline proof permit valid owned-frame read/write/clear operations and reject unmanaged or cross-page requests without a target fault |
| RUN-ASID-001 | Publish a nonzero process ASID | Runtime Pending | GDB observes ASID 1 in TTBR0 for both EL0 variants, address translation survives the switch, and ASID zero is not used for process mappings |
| RUN-KSTACK-001 | Final boot-stack guard translation fault | Runtime Pending | After final TTBR1 publication, GDB confirms the guard has no valid descriptor and a controlled access reports the expected EL1 data abort without corrupting the live stack |
| RUN-KSTACK-002 | Publish, enter, and retire an owned guarded runtime stack | Runtime Pending | Final-map paths Cross Compile a retained arena owner plus prelinked empty L2/L3 topology. The restricted AArch64 mapper is Host Tested with read/mismatch injection, exact RW/NX leaves, preserved guards, ownership tokens, and ordered maintenance before release. The focused image Cross Compiles/ELF Inspects an ordered `PHOENIX_KERNEL_STACK_OK` path that allocates, publishes, round-trips via the virtual alias, retires, verifies/scrubs physical backing, and restores ownership without switching SP. The current seven-table image has not run on target. Closure requires observing that path, aligned stack entry, and target proof that guard leaves stay invalid and TLB invalidation completes before frame return |
| RUN-EXC-001 | Install and enter the EL1 exception vector table | Runtime Pending | Installed `VBAR_EL1` points at the linked 2 KiB table and a deliberate fault emits `PHOENIX_EXCEPTION` from the correct slot |
| RUN-EXC-002 | Preserve and restore the exception frame | Runtime Pending | All `x0..x30`, SP, PC, status, syndrome, and fault-address values match controlled probes |
| RUN-IRQ-001 | Discover and initialize the pinned GICv3 interrupt path | Runtime Pending | The real DTB yields the expected non-overlapping GICD/GICR ranges and timer parent/PPI, the boot CPU redistributor and `ICC_*_EL1` interface initialize while IRQs remain masked, and a controlled PPI is acknowledged, dispatched, EOI'd/deactivated, and returned from without an unexpected interrupt or lock-order violation |
| RUN-TIME-001 | Advance monotonic time and deliver a scheduled timer event | Runtime Pending | On the pinned board, repeated generic-counter reads never regress, checked frequency conversion is correct, the programmed timer PPI is acknowledged through the GIC, due events fire in documented order outside queue protection, and execution continues after interrupt return |
| RUN-UFAULT-001 | Classify and terminate a faulting EL0 process | Runtime Pending | Deliberate translation, permission, execute-never, illegal-instruction, and bad-SVC cases retain exact PC/address/syndrome data, move loaded init from Running to Faulted exactly once, emit `PHOENIX_USER_FAULT` before the matching probe failure sentinel, and never masquerade as `PHOENIX_EXCEPTION` or success |
| RUN-ELF-001 | Populate and map a validated AArch64 ELF image | Runtime Verified | `test-init` selected the inspected ELF and reached the loader's terminal success path |
| RUN-STACK-001 | Consume the native initial user stack | Runtime Verified | `test-init` reached success only after init's aligned-SP, argc/argv/envp, auxv, and terminator self-validation |
| RUN-UCOPY-001 | Copy an active user range through retained frame ownership | Runtime Pending | After final TTBR1 publication, valid and invalid same-page/cross-page reads and writes use only owned direct-mapped frames and return exact bytes or `BadAddress` without a kernel fault |
| RUN-FS-001 | Read a process-local initramfs descriptor into user memory | Runtime Verified | `test-init` emitted exact `etc/motd` bytes after bounded `open`, `read`, EOF, and `close` |
| RUN-INIT-001 | Execute the separately linked native init ELF | Runtime Verified | `cargo xtask test-init` observed all ordered map/EL0 markers, exact output, and `PHOENIX_INIT_OK`; deliberate fault behavior remains `RUN-UFAULT-001` Pending |
| RUN-EL0-001 | Enter and return from the first EL0 program | Runtime Verified | `cargo xtask test-el0` observed `PHOENIX_EL0_ENTER`/`PHOENIX_EL0_OK` after the ASID-1 TTBR0 path, `eret`, unknown syscall, and `exit(42)` |
| RUN-ABI-001 | Dispatch native `SVC #0` calls | Runtime Pending | Register arguments, return values, unknown calls, `write`, and `exit` match ABI revision 0 |

## Validation procedure template

When an emulator becomes available, update each entry with:

1. QEMU version, host architecture, machine version, CPU model, and memory size;
2. exact Phoenix commit and build profile;
3. exact build and launch commands;
4. complete serial output relevant to the check;
5. timeout and process-exit result;
6. pass/fail result and any discovered contract correction.

Do not delete failed evidence. Fix the implementation or contract, retain a concise failure note,
and repeat the validation on the correcting commit.
