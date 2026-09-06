# First Native User Program

This document describes Phoenix's first separately linked userspace executable,
`userspace/init`. It is built, stripped for embedding, accepted by the real Phoenix ELF parser,
stored byte-for-byte as `init` in a deterministic initramfs, and statically inspected. The focused
kernel embeds that exact archive. The program has not executed at EL0 and is therefore Runtime
Pending.

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
current auxiliary key/value, and stack alignment. It then invokes native `write(1, message, 29)`,
requires an exact 29-byte return, and calls `exit(42)`. A mismatch invokes `exit(1)`. All terminal
paths enter an infinite loop if a broken kernel returns from `exit`.

`cargo xtask build-init` produces the development ELF, an embeddable ELF with debug sections
removed, and a linker map. `cargo xtask inspect-init` feeds the exact embeddable artifact into
`ElfImage::parse`, requires one page-bounded RX segment, checks the fixed entry and linker symbols,
and applies a 128 KiB defensive artifact-size ceiling. The same command constructs and validates a
strict `newc` archive containing the exact bytes. Loaded-init kernel inspection requires that
complete inspected archive to be present in the kernel ELF. Emulator-free `cargo xtask ci` runs
all of these checks.

## Invariants

- userspace is an independent Cargo package but inherits the single Phoenix version and toolchain;
- only the AArch64 bare-metal target is linked;
- `_start` and `__init_start` equal `0x00400000`;
- linked executable bytes occupy no more than one 4 KiB virtual page;
- the Phoenix loader accepts the exact stripped artifact intended for embedding;
- the initramfs entry contains that exact artifact and the focused kernel contains the exact
  archive, not merely files with matching names;
- the only load segment is user-readable, user-executable, and never writable;
- the linked message has a statically checked 29-byte extent within that RX segment;
- successful exit is reachable only after `write` returned the complete message length;
- program success and stack-validation failure have distinct exit statuses;
- building or inspecting the program cannot imply EL0 execution.

## Validation

The AArch64 assembler and linker validate every instruction and the linker assertions. Static
inspection validates loader acceptance, machine/type/header fields through the parser, entry,
load count, final permissions, rounded virtual extent, in-memory size, message bytes and symbols,
file-size bound, and nonempty linker map. The source was also assembled during implementation to
verify the `write` and `exit` SVC paths and expected stack offsets. The loaded-init kernel path that consumes this
artifact is documented in `docs/LOADED_INIT_PROBE.md`. Target execution is tracked by
`RUN-INIT-001`.

## TODO

- execute `cargo xtask test-init` and record the complete target evidence;
- implement production process exit and teardown instead of halting in the probe dispatcher;
- replace the probe-only stdout write with a real descriptor-backed syscall;
- move from a conformance-only init to a small Rust runtime once its startup ABI is stable enough;
- supply the checked initramfs independently from the kernel build and govern it with a manifest.

## Skipped work

This program is not a shell, service manager, libc, POSIX environment, Linux binary, dynamically
linked executable, or production `init`. Although packaged as the first initramfs member, it
performs no file lookup and knows only the probe's fixed stdout descriptor. Its single purpose is
to verify the first real loader-to-EL0 and user-copy handoff with the smallest auditable separately
linked artifact.
