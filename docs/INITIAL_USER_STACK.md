# Initial Native User Stack

This document defines Phoenix native ABI revision 0 process-entry stack construction. The
allocation-free builder and its process-image integration are Host Tested and Cross Compiled. No
dynamically loaded user program has yet consumed this layout on a target, so behavior is Runtime
Pending. The loaded-init kernel variant now constructs this exact layout before EL0 entry.

## Current implementation

`InitialStackImage<CAPACITY>` builds an owned byte image immediately below the top of an existing
`UserStackLayout`. It accepts a validated user entry address, `argv`, and `envp`; requires at least
`argv[0]`; rejects interior zero bytes; checks every size/address operation; and fails explicitly if
either its fixed byte capacity or the mapped stack is too small.

At the 16-byte-aligned initial `SP_EL0`, consecutive little-endian 64-bit words contain:

```text
argc
argv[0] ... argv[argc - 1]
0
envp[0] ... envp[envc - 1]
0
aux key, aux value ...
AT_NULL, 0
optional zero alignment padding
NUL-terminated argument and environment strings
```

The initial auxiliary vector contains:

| Key | Value |
| --- | --- |
| `6` (`AUX_PAGE_SIZE`) | `4096` |
| `9` (`AUX_ENTRY`) | validated ELF entry address |
| `0x5000` (`AUX_PHOENIX_ABI_REVISION`) | `0` |
| `0` (`AUX_NULL`) | `0` terminator |

The numeric page-size and entry keys follow the common ELF auxiliary-vector convention. The
Phoenix ABI key is private to the unstable native ABI and is not a Linux compatibility promise.
The builder owns its output, so argument and environment source strings may be released after
construction.

`ProcessImagePlan::with_initial_stack` splits the borrowed byte image at page boundaries. Each stack
page records both a destination offset and its exact source slice. The ordinary population
transaction clears every complete stack frame before writing these slices, so the stack has the
same retry and unpublished-ownership guarantees as ELF segment data. Successful population drops
all source borrows and retains the adjusted initial stack pointer for later EL0 entry.

## Invariants

- `SP_EL0` is 16-byte aligned and lies inside the mapped usable stack;
- the initialized range is exactly `[SP_EL0, stack_top)`;
- every `argv` and `envp` pointer targets a NUL-terminated string in that range;
- both pointer lists have an explicit null terminator;
- the auxiliary vector has an explicit `(AUX_NULL, 0)` terminator;
- all multi-byte values use the initial AArch64 little-endian ABI;
- no initialized bytes enter the guard page or exceed a destination frame;
- each destination stack frame is fully cleared before any stack bytes are copied;
- no page-table root may be published until complete process-image population succeeds.

## Validation

Host tests decode the complete word layout and verify `argc`, pointer targets, strings, auxiliary
entries, alignment, missing `argv[0]`, interior zeros, fixed-capacity exhaustion, mapped-stack
exhaustion, and a stack image spanning two physical pages at a nonzero first-page offset. The
process-image test confirms cleared prefix bytes, exact reconstructed content, retained stack
pointer, and frame release. AArch64 checks compile the same representation and population path.

## TODO

- execute `cargo xtask test-init` so the separately linked init validates this stack on target;
- add executable program-header information when the ELF subset supports `AT_PHDR`, `AT_PHENT`,
  and `AT_PHNUM`;
- add random bytes and `AT_RANDOM` only after a kernel entropy policy exists;
- decide the representation and limits for credentials, platform strings, and executable path;
- define whether an empty environment is the production default for the first `init`;
- add process-wide accounting before allowing substantially larger argument blocks;
- document how future architectures select word size and byte order without changing native ABI
  revision 0 silently.

## Skipped work

This layout does not provide Linux ABI compatibility, TLS, dynamic-loader handoff, vDSO data,
credentials, random canaries, ASLR, stack growth, signals, copied file-descriptor state, or a
general `exec` implementation. It prepares immutable initial bytes only; safe user-memory access
after publication remains a separate mechanism.
