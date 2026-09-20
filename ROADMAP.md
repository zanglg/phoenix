# Phoenix Development Path

Phoenix is developed as one ordered stream until the first formal release. The Cargo version
remains `0.0.0` throughout construction. After the technical path is complete, review candidates
use SemVer prerelease versions `0.1.0-pre.N`; only an explicitly accepted candidate becomes
`0.1.0`.

Work follows the order below. A runtime check may remain pending when its environment is
unavailable, while the next explicitly host-verifiable work proceeds without assuming that check
passed. The broader, unscheduled capability surface remains in `FEATURES.md`.

## Current position

The engineering foundation and **Host-verifiable architecture foundations** are complete. A first
QEMU 9.1.3 `virt-9.1` pass verified the positive boot, boot-memory, final-kernel-map, EL0, and
loaded-init paths. Negative panic, exception, guard, permission, and recovery probes remain
Runtime Pending and must not be inferred from those positive transcripts.

## Completed foundation

- stable Rust toolchain policy, Rust 1.95 MSRV, and AArch64 bare-metal target;
- configurable AArch64 rust-analyzer support from an x86_64 host;
- minimal `no_std` kernel crate;
- host-side xtask validation;
- version, feature, Roadmap, Changelog, and Agent policies.

## 1. Bootstrap and early console

Integrate the existing AArch64 bootstrap, define the linker layout and stack, enter Rust, print a
build identity and deterministic success sentinel through PL011, then halt safely.

Implemented and statically validated:

- bootstrap assembly is integrated through the AArch64 kernel binary;
- the higher-half linker layout, low physical load address, BSS, and 1 MiB boot-construction
  stack are fixed;
- early PL011 output, panic output, build identity, and deterministic sentinels are implemented;
- the ELF, raw image, linker map, and required symbols are checked automatically.

Runtime verified:

- QEMU 9.1.3 `virt-9.1` direct boot reaches higher-half Rust, emits the build identity and boot
  argument through PL011, and satisfies the bounded `PHOENIX_BOOT_OK` smoke protocol.

Remaining runtime validation:

- execute the deliberate panic path;
- directly observe the entry exception level, BSS boundaries, and construction-stack bounds.

Observable result: one command builds and boots Phoenix on AArch64 QEMU `virt`; an automated test
passes only after observing the success sentinel before its timeout.

The current boot contract is in `docs/BOOT.md`. QEMU is pinned to `virt-9.1`; a guest-driven
test-exit protocol remains open.

## 2. Host-verifiable architecture foundations

Build the pure and statically inspectable foundations needed by later runtime integration, in
this order:

- [x] checked physical/virtual addresses, pages, frames, and ranges;
- [x] strict read-only DTB parsing and boot-information extraction;
- [x] physical-memory region normalization, reservation, and frame allocation;
- [x] AArch64 translation indices, descriptor construction, and offline mapping plans;
- [x] exception-frame layout, vector-table layout, and syndrome decoding.

Each mechanism must be Host Tested where behavior is pure, Cross Compiled for AArch64, and kept
independent of unverified MMIO or system-register effects. This section does not install final
translation tables, write `VBAR_EL1`, enable interrupts, or claim target execution.

Observable result: malformed inputs and boundary conditions are covered by host tests, while the
AArch64 artifact retains all required static layout checks.

## 3. Exceptions and diagnostics

Install exception vectors, preserve a complete register context, classify faults, and produce a
useful panic report with symbols or enough addresses for offline symbolization.

Completed without an emulator:

- all vector slots, the exception-frame ABI, ESR and data-abort decoding are Host Tested;
- the linked vector table saves/restores complete integer state and is installed in `VBAR_EL1`;
- ELF inspection enforces the table's 2 KiB size and alignment;
- the fatal dispatcher emits a stable exception sentinel plus vector, PC, status, ESR, and FAR.
- consistent lower-AArch64 synchronous failures are normalized into terminal user-fault records;
  current-EL and inconsistent exceptions retain the kernel fatal path.

Remaining:

- execute deliberate exception probes and verify register preservation on QEMU;
- add offline symbolization and a reliable stack trace;
- define recoverable EL0 page faults before returning from exceptions.

Observable result: deliberate synchronous exceptions are caught and reported deterministically
instead of silently hanging QEMU.

## 4. Memory management

Discover and reserve physical memory, add a page-frame allocator, establish the kernel virtual
address space, install final page tables, and provide a guarded kernel heap and stacks. The
descriptor and mixed-level kernel-table model is Host Tested. A focused Cross Compiled image now
uses the real DTB allocator to create a direct map, separates text/rodata/data permissions, maps
PL011 at page granularity, omits a linker-reserved guard below the boot-construction stack,
permanently retains the tables, and publishes TTBR1 with barriers and TLB invalidation. The focused
QEMU probe has passed. A fixed-metadata fallible heap core now transactionally owns, clears, and
returns contiguous physical backing; alignment, overflow, byte and metadata exhaustion, release,
coalescing, stale tokens, and injected backing failures are Host Tested. Final-direct-map boot
integration now retains a Host Tested proof that every reservation-filtered allocator frame has
the exact EL1 RW/NX mapping and routes loaded-init user copies through a table-owner-bound
capability. The focused final-map image now Cross Compiles a four-frame/four-record post-TTBR1
heap probe with aligned allocation, recoverable byte/metadata exhaustion, release/reuse, and an
ordered success marker; target rerun and production consumer sizing remain. Guarded runtime-stack
slots, transactional frame ownership, clearing, failure rollback, reuse, and offline AArch64 RW/NX
mapping are Host Tested. Final-map builders now prove a retained provisional 16-slot/eight-page
QEMU `virt` arena has no leaf translations and prelink its empty L2/L3 topology in the retained
final hierarchy. An ownership-preserving mapped-stack type state now makes publication and
retirement failures transactional and prevents mapped frames from reaching the unpublished
release path. A restricted AArch64 backend now preflights exact active L3 descriptors, performs
infallible batch writes, and completes per-page all-ASID TLB maintenance before publication or
release; host failure injection covers partial-update prevention. The focused image Cross Compiles
one bounded post-TTBR1 stack allocation, virtual-alias round trip, retirement, scrub, and release
with an ordered success marker. Exact GICD and affinity-selected GICR device pages now bring the
current image to eight tables; both final-map paths now retain their unique pinned GIC/timer owner
and IRQ lookup after TTBR1 publication with delivery closed. That image and mapper still need target execution,
and stack entry, measured sizing, thread switching, and exception-stack activation remain;
the loaded-init path retires temporary TTBR0 aliases when it installs its user hierarchy.

Observable result: allocator and mapping self-tests exercise success and failure paths without
corrupting the bootstrap, DTB, image, or device mappings.

The Host Tested boot-map builder now normalizes DTB memory and removes firmware reservations, the
loaded kernel, and the borrowed DTB before freezing the allocator. A separate Cross Compiled probe
connects the real boot argument and linker symbols, allocates and round-trips one private frame,
and restores ownership; the focused QEMU probe has passed. The final layout and its
explicit deferrals are documented in `docs/FINAL_KERNEL_ADDRESS_SPACE.md`.

## 5. Interrupts and time

Initialize the GIC and generic timer, define interrupt-context rules, dispatch IRQs, and provide a
monotonic clock plus timer queue.

Host-verifiable foundation now available:

- exact raw tick instants and durations use a checked, non-wrapping `u64` timeline;
- lower counter observations and duration overflow are explicit failures;
- a fixed-capacity, allocation-free absolute-deadline queue has deterministic ordering,
  generation-checked cancellation, explicit exhaustion, and no in-lock callbacks;
- an exclusive AArch64 physical-timer register owner, checked `CNTFRQ_EL0` model, and
  nanosecond-to-tick round-up are Host Tested/Cross Compiled but not connected to boot;
- strict platform-layer DTB discovery validates the GICv3 distributor/redistributor ranges,
  controller phandle, and all four architected-timer PPIs without mapping or touching MMIO;
- a failure-bounded GICv3 preparation type state validates PIDR/SRE, preserves one-way affinity
  routing, wakes GICR, configures the timer PPI, and retains explicit recovery evidence while both
  Group-1 delivery gates stay closed;
- boot integration now packs current `MPIDR_EL1` affinity, scans PIDR2/TYPER with VLPI-aware
  strides and a mandatory `Last`, retains the unique matching 128 KiB RD/SGI pair, and maps only it
  plus the complete GICD range as exact final-TTBR1 device pages;
- a non-copying concrete AArch64 backend bounds and performs volatile MMIO plus named `ICC_*_EL1`
  accesses against the retained table/layout contract;
- the EL2 bootstrap permits the GIC system-register interface, and both final-map paths check
  `DAIF.I`, construct the backend only after TTBR1 publication, prepare the DTB-selected timer PPI
  with a fixed 100,000-read per-wait budget, then claim/disable the physical timer and publish the
  permanently retained closed-gate runtime;
- a non-`Send`, non-`Sync` dispatcher-publication proof gates a Host Tested transition that opens
  only affinity-routed Non-secure Group-1 distribution before the current CPU interface, retains
  bounded activation/recovery/shutdown evidence, and never changes `DAIF.I`;
- one exclusive acknowledgement token separates special INTIDs 1020–1023 from real timer or
  unexpected INTIDs, requires combined `ICC_EOIR1_EL1` completion for every real result, and
  retains the outstanding token on failure; the active owner also remembers an abandoned
  obligation and blocks re-acknowledgement or shutdown;
- an allocation-free IRQ-save spinlock supplies Host Tested Acquire/Release exclusion and exact
  local-mask restoration, with a Cross Compiled AArch64 `DAIF.I` backend now used for closed-gate
  runtime publication and lookup in both final-map boot paths;
- an architecture-neutral locked timer controller transactionally couples queue mutations to
  earliest-deadline rearm/disarm, returns due batches only after unlock, restores exact queue state
  on programming failure, closes permanently if hardware recovery also fails, and requires a
  private active-runtime permit for every hardware command;
- a pinned single-CPU interrupt runtime publishes stable dispatcher lookup before activation,
  checks the exact timer PPI, orders outer runtime/GIC locking before the timer lock, reprograms
  before EOI, rejects shutdown with pending events, and disarms before closing GIC delivery;
- the non-transferable AArch64 physical timer implements the locked service backend, but only the
  active runtime can create its post-activation programming permit;
- both final-map paths now claim and disable the physical timer, capture the initial counter, pin
  the one-event runtime in static storage, Release-publish its handle, and route IRQ vector slots
  through an Acquire-checked lookup while delivery remains closed;
- host tests cover boundaries, selection failures, exact page mappings, register update isolation,
  finite polling, injected failure, gate ordering, special/unexpected acknowledgements, abandoned
  completion ownership, pinned publication, timer-before-EOI dispatch, shutdown ordering, and
  ordered output protocol; boot still retains prepared state, so a bounded event consumer,
  activation, `DAIF.I` unmask/removal, sleep integration, and all current target execution remain
  pending.

Observable result: timer interrupts advance a monotonic time source and scheduled callbacks fire
under an automated QEMU test.

## 6. Kernel execution

Introduce synchronization primitives, kernel threads, architecture context switching, idle,
scheduling, and preemption rules. Interfaces must already account for future SMP ordering even
though the first release remains single-core.

Host-verifiable foundation now available:

- an architecture-neutral `IrqSpinLock<T>` masks local IRQ delivery before acquisition, uses
  Acquire/Release ownership, restores the exact prior IRQ state, and keeps its guard on the
  acquiring CPU;
- contended `try_lock` restores IRQ state without changing ownership, while nested different-lock
  guards preserve an outer mask in LIFO order;
- the AArch64 backend changes only `DAIF.I` with compiler barriers and `ISB`, and is Cross Compiled
  in the final-map publication and IRQ lookup paths, but has not handled a target IRQ;
- an architecture-neutral timer controller and pinned AArch64 interrupt runtime prove
  transactional queue/programming, runtime-before-timer lock order, and callbacks-after-unlock in
  host tests; and
- the primitive has no allocation, poisoning, fairness, or scheduler/preemption integration; the
  runtime-before-timer order and closed-gate boot lookup are published, while active delivery,
  scheduler adoption, and all target evidence remain pending.

Observable result: multiple kernel threads make independently verified progress under timer-driven
scheduling.

## 7. Processes and native ABI

Add user address spaces, safe user-memory access, process and thread lifecycle, transition to
userspace, and a small documented native Phoenix syscall ABI.

Host-verifiable prerequisites now include lower-39-bit user mapping and guarded-stack plans plus
the revision-0 AArch64 register and return-value convention. An opt-in statically backed probe now
links an EL0 entry, replaces TTBR0, verifies one returning unknown syscall, and terminates through
`exit(42)`. That positive path is Runtime Verified; negative faults and the table-owned/thread
portions of process lifecycle remain.

Allocator-owned user-table topology, retry-safe descriptor materialization, and an exact combined
owner retaining populated leaf frames plus table frames are now Host Tested. The target
bootstrap-memory backend, nonzero 8-bit ASID model, checked TTBR0 composition, and
ownership-retaining ASID-1 activation are Cross Compiled. The ASID allocator deliberately never
reuses an ID until retirement and invalidation exist. A
Host Tested full-range `copy_from_user` uses resident physical ownership instead of dereferencing
raw pointers. `copy_to_user` applies the same complete residency and permission checks. Process
identity and the strict created/ready/running/exited-or-faulted state machine are now Host Tested,
including a generation-checked fixed-capacity ownership table. Loaded init uses that state machine
through normal exit and classified terminal lower-EL faults. Table-owned general runtime
publication, threads, scheduler-owned fault teardown, ASID reuse/retirement, recoverable page
faults, and negative fault evidence still remain.

Observable result: an EL0 program invokes syscalls, exits, and cannot directly access kernel
memory.

## 8. Executables and minimal system

Load ELF programs from initramfs, define the initial user stack and auxiliary data, add a minimal
VFS and file-descriptor model, and start an `init` program with console input and output.

The strict ELF64/AArch64 validation layer and the combined program/guarded-stack page plan are Host
Tested. Frame ownership is assigned and released transactionally, complete page zero/copy is
enforced through an explicit populated type state, and the native argc/argv/envp/minimal-auxv stack
is included with 16-byte alignment. The QEMU bootstrap backend can perform target writes while its
coarse high alias remains active. A strict initramfs lookup, fixed-capacity read-only file table,
transactional file offsets, and the complete dynamic executable entry path are Cross Compiled;
their positive loaded-init composition is Runtime Verified. General VFS ownership, table-owned
process resources, negative paths, and multi-process behavior remain.

The first standalone AArch64 `init` ELF is reproducibly linked, stripped for embedding, accepted
by the real loader, and inspected as one RX page. A deterministic, strictly validated `newc`
initramfs carries the exact executable under canonical path `init`. A focused kernel variant finds
it there and connects the real boot DTB and allocator through ELF/stack population, dynamically
allocated user tables, a retained final TTBR1 hierarchy, the combined ownership gate, `eret`,
stack self-validation, and bounded `write`, `open`, `read`, `close`, and `exit` calls. Init reads an
exact file from its initramfs into writable user memory and verifies EOF before successful exit.
The harness requires ordered proof that the final kernel map was entered before EL0. The positive
path is Cross Compiled, ELF Inspected, and Runtime Verified on QEMU 9.1.3 `virt-9.1`; deliberate
user-fault and resource-exhaustion paths remain Pending.

Observable result: Phoenix boots from a clean checkout, starts `init`, runs at least one child
program, performs console and in-memory file I/O, and shuts down or reports test completion.

## 9. Release preparation

Remove undocumented failure paths, verify resource-exhaustion behavior, audit unsafe invariants,
stabilize the documented 0.1 interface boundary, and produce reproducible artifacts and an
end-to-end test report.

After all technical 0.1.0 criteria pass:

1. change the project version from `0.0.0` to `0.1.0-pre.1`;
2. move the candidate's accumulated Changelog entries from `Unreleased` to `0.1.0-pre.1`;
3. create the annotated candidate tag `v0.1.0-pre.1`;
4. submit the candidate for owner inspection and implement every requested correction;
5. issue further candidates as `0.1.0-pre.2`, `0.1.0-pre.3`, and so on when another complete
   review cycle is needed.

Only after the owner explicitly accepts a reviewed candidate:

1. change the project version to `0.1.0`;
2. finalize the `0.1.0` Changelog entry;
3. create the annotated `v0.1.0` tag.

Passing automated technical gates authorizes a prerelease candidate, not the final release.

## 0.1.0 release boundary

The first release is intentionally narrow. It supports only AArch64 QEMU `virt`, one CPU, a
native unstable userspace ABI, initramfs, console I/O, and an in-memory filesystem.

The following are not required for 0.1.0: persistent storage, networking, SMP, loadable modules,
Linux binary or syscall compatibility, POSIX completeness, real hardware, RISC-V, or x86_64.
