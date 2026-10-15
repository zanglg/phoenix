# AArch64 ASID allocation and retirement

The bootstrap selects an 8-bit ASID field. ASID zero is reserved for temporary
bootstrap translations; process IDs use 1–255, encoded in TTBR0 bits 55:48. The
checked TTBR composer keeps root BADDR and ASID fields disjoint.

The allocator chooses the lowest unowned ID and returns an explicit Exhausted
error without mutation when none remains. Without retirement it preserves the
original monotonic allocation behavior. `release_retired` is unsafe: the caller
must uniquely own the allocation, prevent old-root activation on every CPU, and
complete invalidation of all old translations, or prove the root was never installed.
Double retirement is rejected; copied numeric IDs are not ownership capabilities.

The one-CPU system reserves ASID 1 for init. Children use ASID 2 repeatedly. On
child exit/fault, it first installs the parent's root, executes DSB/ISB and global
TLBI with completion, then consumes child frame ownership and retires the ASID.
Every switch reads back TTBR0 and checks the exact root/ASID value. Different
programs at the same user virtual addresses exercise this sequence in QEMU.

The focused EL0 and loaded-init fixtures continue to retain ASID 1 forever.
Host tests cover all 255 values, exhaustion, more than 255 reuse cycles, invalid
IDs, exact encoding and double retirement. Target results are in
[RUNTIME_VALIDATION.md](RUNTIME_VALIDATION.md). Per-ASID optimized invalidation,
epoch allocation and SMP shootdown are deferred.
