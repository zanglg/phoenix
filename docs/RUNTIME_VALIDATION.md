# Runtime Validation Ledger

Phoenix is currently developed without QEMU. This ledger prevents compiled or statically
inspected code from being mistaken for code that has executed on the target platform.

## States

- **Runtime Pending**: implementation exists, but no target execution evidence exists.
- **Runtime Verified**: the documented command, environment, expected output, and failure path
  were all exercised successfully.

An entry can become Runtime Verified only by recording the environment, exact command, relevant
output, and source commit. Host tests, cross-compilation, disassembly, and ELF inspection are
valuable evidence but cannot close an entry in this ledger.

## Pending checks

| ID | Capability | State | Evidence required |
| --- | --- | --- | --- |
| RUN-BOOT-001 | Enter `_start` through QEMU direct kernel boot | Runtime Pending | Exact QEMU machine/CPU command reaches early output |
| RUN-BOOT-002 | Normalize EL2 or EL1 state and enter Rust at EL1 | Runtime Pending | Observed EL path and `kernel_main` entry |
| RUN-BOOT-003 | Temporary identity and higher-half mappings | Runtime Pending | Execution crosses MMU enable and continues at higher-half Rust code |
| RUN-BOOT-004 | BSS initialization and 64 KiB boot stack | Runtime Pending | Runtime probes confirm zeroed BSS and stack bounds/alignment |
| RUN-CONSOLE-001 | QEMU `virt` PL011 output and flush | Runtime Pending | Complete build identity and success sentinel are observed |
| RUN-PANIC-001 | Early panic reporting | Runtime Pending | Deliberate panic emits `PHOENIX_PANIC` before timeout |
| RUN-SMOKE-001 | Bounded automated boot smoke test | Runtime Pending | `cargo xtask test-boot` observes the sentinel and terminates QEMU predictably on the pinned board |
| RUN-DTB-001 | Parse the QEMU-provided DTB in early boot | Runtime Pending | Real blob validates and yields expected CPU, memory, chosen, and reservation data |
| RUN-MM-001 | Initialize the frame allocator from discovered memory | Runtime Pending | Allocations avoid the image, DTB, boot tables, stack, and firmware reservations |
| RUN-MMU-001 | Install final permission-separated kernel tables | Runtime Pending | Text, rodata, data, stack, DTB, and MMIO mappings behave with documented permissions |
| RUN-MMU-002 | Retire temporary aliases and maintain the TLB | Runtime Pending | Execution survives table switch, barriers, invalidation, and low-RAM alias removal |
| RUN-EXC-001 | Install and enter the EL1 exception vector table | Runtime Pending | Installed `VBAR_EL1` points at the linked 2 KiB table and a deliberate fault emits `PHOENIX_EXCEPTION` from the correct slot |
| RUN-EXC-002 | Preserve and restore the exception frame | Runtime Pending | All `x0..x30`, SP, PC, status, syndrome, and fault-address values match controlled probes |
| RUN-ELF-001 | Populate and map a validated AArch64 ELF image | Runtime Pending | File bytes, zero-fill, page permissions, entry address, and rollback behavior match the image contract |
| RUN-EL0-001 | Enter and return from the first EL0 program | Runtime Pending | EL0 uses the guarded stack, cannot access kernel mappings, and returns only through a handled exception |
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
