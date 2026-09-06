# QEMU Virt Bootstrap Physical Memory

This document describes the target-side private-frame access backend available while the temporary
QEMU `virt` TTBR1 mapping remains active. The implementation is Cross Compiled and Host Tested for
address validation, but every memory access is Runtime Pending.

## Current implementation

The AArch64 bootstrap maps QEMU RAM physical `0x40000000..0x80000000` to the higher-half virtual
range beginning at `0xffffff8040000000`. `BootstrapPhysicalMemory` converts a checked physical
range inside that window to its high alias and implements both loader backends:

- `ProcessImageMemory` clears a private frame and copies a checked byte range into it;
- `TranslationTableMemory` clears all table entries and writes one aligned 64-bit descriptor;
- a checked read method copies bytes from a private frame for boot-probe verification.

It can also borrow the firmware DTB from the physical address in `x0`. The implementation first
checks and reads only the 40-byte header, then validates the declared total size before creating
the complete slice. The returned `DeviceTree` lifetime is tied to the bootstrap-memory handle, so
safe code cannot retain it after the mapping assertion is released. No DTB page may be allocated
while this borrow exists.

`kernel_physical_range` converts the linked higher-half `__kernel_start..__kernel_end` symbols into
one physical range and rejects an empty, reversed, untranslated, or out-of-window image. This is
the single target-side source of the kernel reservation supplied to boot memory-map construction.

Every operation validates that the complete physical frame is inside the temporary RAM block and
that offset plus length stays within one page. Address arithmetic is checked. Empty writes are
accepted after range validation. Source/destination overlap is supported for defensive bootstrap
loading, although normal ELF input and newly allocated destination frames are disjoint.

The handle can be constructed only through an unsafe assertion that the documented TTBR1 mapping
is active and remains active, the frames are privately owned, and no other CPU accesses them.
Methods are safe after that boundary because all per-operation address and length inputs are
checked.

## Invariants

- only checked ranges within physical `0x40000000..0x80000000` are accessible;
- no operation crosses a 4 KiB frame boundary;
- frame clearing covers exactly one full page;
- descriptor stores are naturally aligned and bounded to one of 512 entries;
- all destination frames remain private and unpublished while being mutated;
- the temporary high RAM alias remains valid for the handle's entire use;
- a borrowed DTB and all its derived values remain inside that alias and retain the handle's
  lifetime;
- the linker-derived physical kernel image is non-empty and fully inside bootstrap RAM;
- this backend is discarded before the temporary coarse TTBR1 mapping is retired.

## Validation

Host tests cover the first and last valid frames, physical byte-range boundaries, the last valid
byte, a cross-page request, arithmetic overflow, and high-link-to-physical kernel conversion.
Bare-metal AArch64 checks compile DTB borrowing, linker symbol access, the actual pointer operations,
and their safety comments. QEMU must still prove that the high alias is readable and writable
normal memory, that the generated DTB can be parsed through it, and that populated instructions
and descriptors are observed by the CPU.

## TODO

- construct the backend only after a runtime assertion of the expected bootstrap translation
  regime;
- feed loader writes exclusively with frames owned through the DTB-derived allocator;
- add explicit cache maintenance before enabling caches or executing copied instructions;
- replace it with a final permission-separated physical direct map or bounded temporary mapper;
- prevent the final TTBR1 switch while any bootstrap-memory handle can still exist;
- record target faults and address translations in the runtime validation ledger.

## Skipped work

This backend does not discover RAM, allocate frames, map memory above the first 1 GiB QEMU RAM
window, provide DMA coherency, synchronize CPUs, modify live page tables, or define the permanent
kernel direct map. It is a narrow bridge from the existing bootstrap to the production ownership
model, not a general physical-memory API.
