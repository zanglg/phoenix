# Process Lifecycle

This document describes Phoenix's Host Tested process identity and lifecycle model and its first
Cross Compiled use by loaded init. It does not claim that a scheduler or complete teardown path
exists.

## Current implementation

`kernel/src/process.rs` defines an architecture-neutral process control model:

```text
Created -> Ready -> Running -> Exited(status) -> reap
```

Every transition is explicit and rejects the wrong source phase without modifying state. A native
exit status retains the complete 64-bit ABI value; Phoenix does not currently apply Unix's
eight-bit truncation rule.

A `ProcessId` is the pair `(slot, nonzero generation)`. `ProcessTable<T, N>` owns up to `N`
`Process<T>` values and therefore owns each process's resource aggregate `T`. New processes occupy
the lowest reusable slot. Reaping is allowed only after exit, returns the resource aggregate to the
caller, and increments the slot generation. An old identifier then reports `StaleIdentifier`
instead of naming the replacement process. A generation that reaches `u64::MAX` retires its slot
after reap rather than wrapping to zero or reusing an earlier identity.

Lookup distinguishes an out-of-range slot, a current vacant generation, and a stale generation.
Insertion failure returns the unconsumed resource aggregate. Failed lookup, transition, or reap
does not remove resources or partially change lifecycle state.

## Loaded-init integration

The one-process loaded-init runtime now retains a `ProcessControl` beside its address-space owner,
file table, final kernel tables, and remaining physical allocator. Its fixed bootstrap identity is
slot zero, generation one.

The control remains `Created` while ELF pages and both translation hierarchies are constructed. It
becomes `Ready` only after all resources are complete and immediately before the runtime aggregate
is installed. After final TTBR1 publication, a narrow non-escaping mutable operation transitions it
to `Running`; only then does Phoenix emit `PHOENIX_INIT_ENTER` and publish TTBR0 through `eret`.

The native `exit` handler must successfully transition the running control to `Exited(status)` in
addition to checking status 42 and prior output before it emits `PHOENIX_INIT_OK`. The terminal
probe then halts. It deliberately retains all process resources because no scheduler context or
safe TTBR0 retirement-and-reap handoff exists yet.

The generic process table is not yet used by this one-process static runtime. The integration
validates the lifecycle boundary without pretending that table publication, locking, or teardown
is already safe on target hardware.

## Invariants

- process identity includes a nonzero generation and is not merely a reusable integer slot;
- no process can enter `Ready` twice, run before ready, exit before running, or run after exit;
- only an exact current identifier can inspect or mutate table-owned resources;
- reaping is the sole operation that separates resources from an exited table entry;
- slot reuse always advances the generation; exhaustion retires the slot permanently;
- process-table capacity exhaustion returns ownership to the caller;
- lifecycle state is policy metadata and never substitutes for address-space or frame ownership;
- loaded init retains its resources after exit until a future scheduler-owned teardown context can
  deactivate TTBR0 and reclaim them safely.

## Validation completed without QEMU

Host tests cover valid and invalid transitions, generation-zero rejection, deterministic slot
selection, capacity failure with resource recovery, mutable resource access through an exact ID,
out-of-range/vacant/stale lookup, reap-before-exit rejection, generation increment, old-ID
invalidation, and permanent retirement at generation exhaustion.

The loaded-init variant is Cross Compiled and ELF Inspected with the process model linked into its
ready/start/exit path. Runtime execution remains part of `RUN-INIT-001`; static inclusion is not
evidence that EL0 reached the transitions.

## TODO

- make a process table, rather than the static probe aggregate, the authoritative runtime owner;
- assign ASIDs with generations and retire TTBR0 before reclaiming page-table or user frames;
- separate processes from threads and define which state is process-wide versus schedulable;
- add a scheduler-owned transition from running to ready or blocked without weakening the current
  terminal-state rules;
- define parent/child ownership, zombie retention, wait semantics, orphan adoption, and init exit;
- move file descriptors, credentials, limits, and VM accounting into the process resource owner;
- route recoverable EL0 faults through a terminal process outcome rather than a kernel-wide halt;
- add synchronization and memory-ordering rules before more than one CPU or interrupt context can
  access the table;
- define identifier exposure and exhaustion behavior in the native ABI.

## Skipped work

Phoenix does not yet provide process creation, fork, exec replacement, threads, scheduling,
blocking, wakeup, signals, wait, parent relationships, namespaces, credentials, per-process limits,
ASID allocation, address-space teardown, or runtime resource reclamation. `ProcessTable` is a
bounded ownership mechanism, not a scheduler or a claim of POSIX process semantics.
