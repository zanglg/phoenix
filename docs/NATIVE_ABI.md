# Native Phoenix ABI revision 0

The released 0.1 syscall table, limits and error behavior are in
[0.1-CONTRACT.md](0.1-CONTRACT.md#processes-and-native-abi). Development additions
and scheduling invariants are in [0.2-SCHEDULING.md](0.2-SCHEDULING.md). Revision 0 is native
and unstable; it implies no Linux or POSIX compatibility.

AArch64 uses `SVC #0`, number in x8, six arguments in x0–x5, return in x0.
Returning calls preserve other integer registers and the architectural next PC.
Numbers 0–5 are exit, write, read, open, close and synchronous child run. Unknown
numbers return ENOSYS. Development calls 6–8 add single-child spawn, wait and
cooperative yield with the bounded semantics defined in the 0.2 design.
Development call 9 adds sleep for 0–1,000,000,000 nanoseconds: zero yields, positive
durations return 0 no earlier than their deadline, and larger values return EINVAL.
A nonzero SVC immediate is a terminal BadSystemCall fault,
which kills the child and returns EFAULT to its waiting parent.

The system dispatcher supports console descriptors 0/1/2, eight regular descriptors,
256-byte transfers and 128-byte canonical relative paths. Read/write tmp mode is 1;
read-only archive/tmp mode is 0. Stdin returns EAGAIN when empty. File reads commit
offsets only after complete validated user copying; writes have no partial-capacity
success. Parent and child register/address-space ownership is described in the
current contract and [UNSAFE_REVIEW.md](UNSAFE_REVIEW.md).

The focused `test-init` fixture deliberately retains its earlier read-only bridge
and does not implement `run`; `test-system` exercises the complete revision-0 system
boundary. Both are explicitly separate image variants.

Native initial stack words, alignment, argc/argv/envp and auxiliary-vector layout
remain defined in [INITIAL_USER_STACK.md](INITIAL_USER_STACK.md). ELF images are
static AArch64 executables with no dynamic linker or TLS contract. The runtime
ledger records actual positive, negative and resource-exhaustion execution evidence.
