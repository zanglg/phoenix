# QEMU Virt Bootstrap Physical Memory

This document describes the target-side private-frame access backend available while the temporary
QEMU `virt` TTBR1 mapping remains active. The implementation is Cross Compiled and Host Tested for
address validation, but every memory access is Runtime Pending.

## Current implementation

The AArch64 bootstrap maps QEMU RAM physical `0x40000000..0x80000000` to the higher-half virtual
range beginning at `0xffffff8040000000`. `BootstrapPhysicalMemory` converts a complete 4 KiB frame
inside that physical window to its high alias and implements both loader backends:

- `ProcessImageMemory` clears a private frame and copies a checked byte range into it;
- `TranslationTableMemory` clears all table entries and writes one aligned 64-bit descriptor.

Every operation validates that the complete physical frame is inside the temporary RAM block and
that offset plus length stays within one page. Address arithmetic is checked. Empty writes are
accepted after range validation. Source/destination overlap is supported for defensive bootstrap
loading, although normal ELF input and newly allocated destination frames are disjoint.

The handle can be constructed only through an unsafe assertion that the documented TTBR1 mapping
is active and remains active, the frames are privately owned, and no other CPU accesses them.
Methods are safe after that boundary because all per-operation address and length inputs are
checked.

## Invariants

- only complete frames within physical `0x40000000..0x80000000` are accessible;
- no operation crosses a 4 KiB frame boundary;
- frame clearing covers exactly one full page;
- descriptor stores are naturally aligned and bounded to one of 512 entries;
- all destination frames remain private and unpublished while being mutated;
- the temporary high RAM alias remains valid for the handle's entire use;
- this backend is discarded before the temporary coarse TTBR1 mapping is retired.

## Validation

Host tests cover the first and last valid frames, both physical window boundaries, the last valid
byte, a cross-page request, and arithmetic overflow. Bare-metal AArch64 checks compile the actual
pointer operations and their safety comments. QEMU must still prove that the high alias is writable
normal memory and that populated instructions and descriptors are observed by the CPU.

## TODO

- construct the backend only after a runtime assertion of the expected bootstrap translation
  regime;
- feed it exclusively with frames reserved through the DTB-derived allocator;
- add explicit cache maintenance before enabling caches or executing copied instructions;
- replace it with a final permission-separated physical direct map or bounded temporary mapper;
- prevent the final TTBR1 switch while any bootstrap-memory handle can still exist;
- record target faults and address translations in the runtime validation ledger.

## Skipped work

This backend does not discover RAM, allocate frames, map memory above the first 1 GiB QEMU RAM
window, provide DMA coherency, synchronize CPUs, modify live page tables, or define the permanent
kernel direct map. It is a narrow bridge from the existing bootstrap to the production ownership
model, not a general physical-memory API.
