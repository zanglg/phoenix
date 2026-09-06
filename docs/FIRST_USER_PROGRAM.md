# First Native User Program

This document describes Phoenix's first separately linked userspace executable,
`userspace/init`. It is built, stripped for embedding, accepted by the real Phoenix ELF parser, and
statically inspected. It has not executed at EL0 and is therefore Runtime Pending.

## Current implementation

`phoenix-init` is an independent `no_std`, `no_main` AArch64 ELF64 executable linked at
`0x00400000`. It has one bounded, read/execute `PT_LOAD` segment and no writable data, heap,
runtime, TLS, relocation, dynamic interpreter, or library dependency. The source entry is a small
AArch64 assembly routine; Rust supplies only the freestanding crate and unreachable panic policy.

The initial program expects this exact native stack input:

- `argv = ["/init", "phoenix"]`;
- `envp = ["TERM=phoenix"]`;
- page-size, entry-address, and Phoenix ABI revision-0 auxiliary entries;
- null terminators and a 16-byte-aligned stack pointer.

It checks the argument count, non-null pointers, selected string bytes and terminators, every
current auxiliary key/value, and stack alignment. Success invokes native `exit(42)` through
`SVC #0`; any mismatch invokes `exit(1)`. Both paths enter an infinite loop if a broken kernel
returns from the terminal syscall.

`cargo xtask build-init` produces the development ELF, an embeddable ELF with debug sections
removed, and a linker map. `cargo xtask inspect-init` feeds the exact embeddable artifact into
`ElfImage::parse`, requires one page-bounded RX segment, checks the fixed entry and linker symbols,
and applies a 128 KiB defensive artifact-size ceiling. Emulator-free `cargo xtask ci` runs both.

## Invariants

- userspace is an independent Cargo package but inherits the single Phoenix version and toolchain;
- only the AArch64 bare-metal target is linked;
- `_start` and `__init_start` equal `0x00400000`;
- linked executable bytes occupy no more than one 4 KiB virtual page;
- the Phoenix loader accepts the exact stripped artifact intended for embedding;
- the only load segment is user-readable, user-executable, and never writable;
- program success and stack-validation failure have distinct exit statuses;
- building or inspecting the program cannot imply EL0 execution.

## Validation

The AArch64 assembler and linker validate every instruction and the linker assertions. Static
inspection validates loader acceptance, machine/type/header fields through the parser, entry,
load count, final permissions, rounded virtual extent, in-memory size, named symbols, file-size
bound, and nonempty linker map. The source was also disassembled during implementation to verify
both `SVC #0` paths and expected stack offsets. Target execution is tracked by `RUN-INIT-001`.

## TODO

- embed this exact inspected artifact in a focused kernel variant;
- feed it through DTB allocation, process-image population, dynamic user tables, and owned
  activation;
- add a QEMU harness requiring a dedicated loaded-init success sentinel;
- implement production process exit and teardown instead of halting in the probe dispatcher;
- implement native `write` after fault-safe user copying exists;
- move from a conformance-only init to a small Rust runtime once its startup ABI is stable enough;
- source the first production image from a checked initramfs rather than the kernel build tree.

## Skipped work

This program is not a shell, service manager, libc, POSIX environment, Linux binary, initramfs
member, dynamically linked executable, or production `init`. It performs no I/O and owns no file
descriptors. Its single purpose is to verify the first real loader-to-EL0 handoff with the smallest
auditable separately linked artifact.
