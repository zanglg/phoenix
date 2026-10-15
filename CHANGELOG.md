# Changelog

All notable changes to Phoenix are documented here. The project uses SemVer-compatible release
numbers and remains explicitly unstable before 1.0.0. During construction, changes remain under
`Unreleased` and the Cargo version remains 0.0.0. Technical completion produces a
`0.1.0-pre.N` review candidate; `0.1.0` requires explicit owner acceptance.

## [Unreleased]

## [0.1.0] - 2026-10-15

### Release

- First formal experimental AArch64 QEMU virt release, promoting the reviewed
  0.1.0-pre.2 implementation at the scope in docs/0.1-CONTRACT.md.
- Includes protected memory, exceptions and diagnostics, focused kernel worker
  preemption, independent ELF processes, native syscalls, console/archive/tmp I/O,
  fault isolation and resource recovery, with runtime CI and reproducible packaging.
- Formal-source validation and artifact provenance must be regenerated after
  promotion; prerelease artifacts are not relabeled as release artifacts.

## [0.1.0-pre.2] - 2026-10-14

### Fixed

- User-fault regression tests require each expected kind, ELF-derived fault PC,
  FAR and ESR, including read/write/execute and translation level. A QEMU negative
  control replaces all nine programs' entry instructions with illegal instructions
  and requires the host oracle to reject the result.
- Conformance init accepts CR, LF and CRLF console lines; invalid input is tested.
- Normal system termination reports init status/fault without hardcoded test
  counts or exit status 42. Conformance decisions belong to userspace and xtask.

### Added

- `test-hardening`: target rollback with 0–9 available frames, failure injection
  at all 21 image/table memory operations, and exhaustion/retirement of 254 unused
  ASIDs, followed by a successful load and 705 child lifetimes with 576 faults.
- Exact free-frame-set, ASID and process-slot recovery assertions, stale PID
  rejection, stack scrubbing, child descriptor exhaustion/reaping, retained parent
  descriptor/offset checks, and tmp-file content verification after ENOSPC.
- Recorded QEMU/Rust runtime CI covering all target suites and retaining logs/packages.
- Clean-worktree packaging which rejects a source change during artifact generation.

## [0.1.0-pre.1] - 2026-10-13

### Added

- Real EL1/EL2 state, panic, full-register, permission and guarded-memory probes with
  guest-driven exits and bounded QEMU evidence.
- Active GICv3 timer/IRQ-lock verification, round-robin kernel threads, preemption,
  sleep, idle, complete context restoration and guarded-stack reclamation.
- A four-page heap alias with two unmapped guards and transactional descriptor preflight.
- Native minimal system with independent init/child ELF programs, synchronous parent wait,
  fault containment, checked cross-page copies, frame reclamation and safe ASID retirement/reuse.
- Read-only initramfs plus bounded writable tmp files, process-owned descriptors, console
  input, atomic capacity failures and resource-exhaustion recovery tests.
- Bounded frame-pointer traces, exact-ELF offline symbolization, structured diagnostic ring,
  versioned artifact packaging, SHA-256 checksums and compiler/source provenance.
- Current 0.1 contract, unsafe invariant review, complete runtime report and MSRV checks.
- A runtime regression fix replacing large by-value final-map planning copies with in-place
  private plan updates; the 1 MiB construction stack remains unchanged.
- Separate first-2-MiB permission/guard and first-4-MiB image bounds, enforced by linker
  and artifact inspection, for the larger debug system image's RW/NX stack tail.


- Initial Rust 2024 workspace with a single Phoenix version source.
- Stable Rust toolchain policy, Rust 1.95 MSRV, and AArch64 bare-metal target.
- `no_std` AArch64 kernel binary with the initial bootstrap, higher-half linker layout, BSS
  initialization, 1 MiB boot-construction stack, PL011 early console, panic path, and safe halt loop.
- Build identity plus deterministic boot-success and panic sentinels.
- ELF, raw kernel image, and linker-map generation with static layout and symbol inspection.
- Config-file-driven rust-analyzer target selection.
- Standalone host-side xtask commands for environment checks, tests, builds, and validation.
- Explicit Host Tested, Cross Compiled, ELF Inspected, Runtime Pending, and Runtime Verified states.
- Allocation-free monotonic raw-tick values and a deterministic fixed-capacity timer queue with
  checked regression/overflow, absolute deadlines, explicit exhaustion, generation-safe
  cancellation, failure atomicity, and callback-free due-event removal.
- A checked AArch64 generic-counter frequency and nanosecond conversion model plus an exclusive
  target-only physical-timer owner that programs absolute compares while interrupt signaling
  remains masked and requires an unsafe, IRQ-ready transition to unmask.
- Strict allocation-free QEMU `virt` DTB discovery for the GICv3 distributor/redistributor,
  controller phandle, and four unique architected-timer PPIs, with malformed, disabled,
  misaligned, aliased, and wrong-parent topologies rejected before any MMIO access.
- An architecture-side GICv3 preparation type state with a unique unsafe register-backend
  contract, checked PPI/priority configuration, PIDR/SRE validation, one-way ARE preservation,
  finite RWP/wake polling, isolated field updates, closed delivery gates, and retained primary and
  best-effort recovery failures.
- Boot-CPU GICv3 redistributor selection from packed `MPIDR_EL1` affinity with PIDR/TYPER checks,
  VLPI-aware bounded scanning, exact final-TTBR1 GICD/GICR L3 device mappings, retained layout
  evidence, and a concrete bounds-checked volatile MMIO/`ICC_*_EL1` backend.
- EL2 bootstrap permission for the GIC system-register interface plus one-time post-TTBR1 GICv3
  preparation in both final-map paths, with checked IRQ masking, a 100,000-read per-wait budget,
  permanently retained closed-delivery ownership, and an ordered static/runtime marker.
- A non-transferable dispatcher-publication proof plus Host Tested GICv3 delivery type state with
  ordered and bounded gate activation/recovery/shutdown, special-INTID handling, exclusive timer
  and unexpected acknowledgement tokens, combined EOI/deactivation, and retained failure
  ownership; boot deliberately remains in the closed prepared state.
- An allocation-free IRQ-save spinlock with Acquire/Release ownership, exact nested IRQ-state
  restoration, contended try-lock rollback, non-transferable guards, host contention tests, and a
  Cross Compiled AArch64 `DAIF.I` masking backend.
- A Host Tested locked timer controller with transactional earliest-deadline rearm/disarm,
  deterministic fixed-capacity due batches returned after unlock, exact queue rollback, hardware
  recovery evidence, permanent closure after failed recovery, private programming permits, and
  pending-event rejection before explicit hardware quiescence.
- A Host Tested pinned AArch64 interrupt runtime that publishes stable vector lookup before GIC
  activation, enforces exact timer-PPI binding and runtime-before-timer lock order, reprograms the
  timer before EOI, retains abandoned acknowledgement obligations, and disarms before ordered GIC
  shutdown; the concrete non-transferable physical-timer service adapter Cross Compiles.
- Closed-gate boot publication of that runtime in both final-map paths: the physical timer is
  claimed, disabled, and observed once; pinned runtime and handle slots are Release-published; and
  every IRQ vector reaches an Acquire-checked dispatch/fatal policy without activating GIC delivery
  or unmasking `DAIF.I`, with a separate statically inspected publication progress marker.
- An IRQ-safe one-shot timer-event consumer that binds one exact queue generation and opaque event,
  rejects empty/multiple/mismatched/repeated delivery without false completion, retains the fired
  record, and is reached by the final-map IRQ wrapper only after the runtime has completed EOI.
- An active-runtime one-shot transaction that holds local IRQ masking across timer scheduling and
  exact consumer binding, cancels the newly armed timer if binding fails, and permanently closes
  the runtime when cancellation/recovery cannot restore a safely matched state.
- Checked architecture-neutral physical/virtual addresses, pages, frames, and half-open ranges.
- Strict allocation-free DTB parsing with boot CPU, bootargs, memory-region, and reservation extraction.
- Fixed-capacity physical memory normalization, conservative reservation, and first-fit frame allocation.
- Fixed-metadata fallible kernel heap core with transactionally acquired and cleared contiguous
  backing frames, exact alignment, explicit physical/byte/metadata exhaustion, implicit free-space
  coalescing, lifecycle-checked tokens, and transactional backing release.
- Fixed-capacity guarded kernel-stack slots with transactional cleared-frame ownership, lower and
  upper guard pages, generation-safe reuse, explicit failure rollback, and Host Tested AArch64
  EL1 RW/NX offline mapping.
- A provisional QEMU `virt` runtime-stack arena with 16 retained slots, eight usable pages per
  slot, two guards, full initial-leaf-absence validation, prelinked empty L2/L3 topology, and one
  pool owner in each final-TTBR1 path.
- Ownership-preserving mapped kernel-stack type state with an unsafe architecture-backend contract,
  unsafe low-level entry points, unique publication tokens, transactional map/unmap failures, and
  frame release gated on successful mapping retirement.
- A restricted AArch64 runtime-stack leaf mapper with retained-table identity, fallible preflight,
  exact descriptor/token validation, infallible batch commit, final-direct-map descriptor access,
  and per-page all-ASID TLB maintenance before mapping use or frame release.
- A bounded final-map runtime-stack conformance path that publishes one owned stack, round-trips
  bytes through its live virtual alias, retires it, verifies and scrubs physical backing, restores
  allocator/pool ownership, and gates terminal success on a dedicated marker.
- Reservation-filtered managed-frame snapshots, full-leaf final-direct-map proof, a retained-table
  RAM access capability, and loaded-init user copies no longer dependent on bootstrap raw helpers.
- Bounded final-map heap initialization after TTBR1 publication, with retained frame ownership,
  aligned release/reuse, recoverable byte and metadata exhaustion checks, and an ordered static
  and runtime-validation marker.
- Host-tested AArch64 translation indices, stage-1 descriptors, mapping attributes, and fixed-capacity offline mapping plans.
- Pinned QEMU `virt` command generation and a bounded boot integration harness with sentinel, timeout, output-limit, and log handling.
- Host-tested AArch64 vector-slot model, stable exception-frame ABI, ESR decoding, SVC immediate extraction, and data-abort classification.
- Linked 2 KiB AArch64 EL1 vector table, complete register save/restore assembly, `VBAR_EL1` installation, and fatal exception diagnostics.
- Host-tested lower-39-bit user ranges, W^X mapping plans, and guarded initial stack placement.
- Strict allocation-free ELF64/AArch64 executable validation with load ranges, zero-fill, permissions, overlap, and entry-point checks.
- Native ABI revision-0 syscall values plus AArch64 `x8`/`x0..x5` request and signed return-register adaptation.
- Opt-in, statically validated first-EL0 probe with private TTBR0 tables, RX code, guarded zeroed stack, `eret`, returning unknown syscall, and terminal `exit(42)` path.
- Separate bounded `test-el0` QEMU protocol that cannot pass on the earlier boot marker.
- Transactional process-image planning for ELF segments plus a guarded stack, with per-page clear/copy work, unique frame ownership, and all-or-nothing allocation and release.
- Retry-safe populated-image type state over an abstract private-frame backend, with full-page clearing before initialized ELF bytes are copied.
- Allocator-owned AArch64 user-table topology with intermediate-table reuse, transactional frame assignment, retry-safe descriptor materialization, and exact combined leaf/table ownership before activation.
- Checked QEMU `virt` bootstrap physical-memory backend for private image frames and translation tables under the temporary TTBR1 RAM alias.
- Transactional DTB-derived boot memory map that removes firmware reservations, the complete kernel image, and the borrowed DTB before allocator creation.
- Conservative `/reserved-memory` parsing that reserves static child ranges and rejects unsupported dynamic or translated reservation forms.
- Checked target-side DTB borrowing and linker-symbol conversion for the physical boot image
  reservation under the temporary QEMU `virt` high-memory alias.
- Opt-in boot-memory integration image and bounded `test-memory` protocol covering real DTB
  parsing, reservations, frame allocation, higher-half byte readback, scrub, and release.
- Allocation-free native initial user stack with argc, argv, envp, minimal auxiliary vector,
  16-byte alignment, checked capacity, and page-offset-aware process-image population.
- Ownership-gated AArch64 user-address-space activation that requires a permanently retained
  complete leaf/table owner before publishing TTBR0 and entering EL0t.
- Separately linked AArch64 `phoenix-init` conformance ELF plus canonical build and static
  inspection through the production loader contract.
- Opt-in loaded-init kernel variant and bounded QEMU protocol connecting the real DTB allocator,
  embedded ELF population, native stack, dynamic user tables, ownership-gated EL0 entry, and
  terminal `exit(42)`, with exact embedded-artifact inspection.
- Strict allocation-free `newc` initramfs validation, canonical lookup, deterministic host
  packaging of the first init ELF, and exact archive embedding in the loaded-init kernel.
- Fully prevalidated cross-page `copy_from_user`, retained loaded-init runtime ownership, bounded
  stdout syscall handling, and a userspace message/return-value conformance check.
- Fully prevalidated `copy_to_user`, a generation-safe fixed-capacity read-only initramfs file
  table, bounded `open`/`read`/`close` probes, and end-to-end initramfs file-I/O conformance logic.
- Allocator-owned mixed-level AArch64 kernel tables, page-granular text/rodata/data permission
  overrides, a QEMU `virt` RAM direct map and PL011 device page, one-time TTBR1 publication with
  barriers/TLB invalidation, and a bounded final-map probe now verified on QEMU 9.1.3.
- Loaded-init integration of the retained final TTBR1 owner, explicit bootstrap-memory capability
  revocation, ownership-checked post-switch user-frame copies, and ordered QEMU progress evidence.
- One linker-reserved boot-stack guard page, explicit direct-map holes, final-plan absence checks,
  and static artifact validation of guard size and placement.
- Architecture-neutral process IDs, strict lifecycle transitions, generation-safe bounded process
  ownership, reap semantics, and loaded-init ready/running/exited integration.
- Nonzero 8-bit AArch64 ASIDs, monotonic no-reuse allocation, checked TTBR0 bit composition, and
  ASID-1 activation for both the static and loaded EL0 paths.
- Architecture-neutral terminal user-fault records, AArch64 lower-EL synchronous classification,
  faulted-process lifecycle/reap semantics, and deterministic EL0/init QEMU failure protocols.
- First QEMU 9.1.3 runtime-validation pass on versioned `virt-9.1`: bounded boot, boot-memory,
  final-kernel-map, EL0, and loaded-init probes passed with retained serial logs. The first
  final-map run exposed construction-stack exhaustion; the 1 MiB stack and in-place plan building
  retain the 4 KiB page size, 512 MiB board memory, and first-2-MiB image bound.
- Project, Agent, Roadmap, and kernel capability documentation.
- Two-stage release gating: technical completion produces `0.1.0-pre.N` candidates, while final
  `0.1.0` promotion requires explicit owner review and acceptance.
