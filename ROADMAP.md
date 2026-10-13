# Phoenix Development Path

Phoenix follows one ordered technical path to AArch64 QEMU `virt` on one CPU.
Construction uses `0.0.0`; completed technical candidates use `0.1.0-pre.N`.
Only explicit owner review and acceptance promotes a candidate to `0.1.0`.

## Current position

The technical path is implemented and validated at the bounded scope in
[docs/0.1-CONTRACT.md](docs/0.1-CONTRACT.md). The next gate is candidate review,
corrections if requested, and explicit owner acceptance. Command results and
source identity are recorded in [docs/RUNTIME_VALIDATION.md](docs/RUNTIME_VALIDATION.md).
No support claim is inferred from compilation alone.

## 1. Bootstrap and early console

Complete: AArch64 EL2/EL1 direct boot, higher-half Rust entry, full BSS checks,
1 MiB construction stack, polled PL011, deterministic sentinels and intentional
panic reporting. Both entry levels and the guest-driven test exit run on QEMU.

## 2. Host-verifiable architecture foundations

Complete: checked addresses/ranges, DTB parsing, normalized reservations, physical
allocation, descriptor/table planning, exception-frame layout and syndrome models.
Boundary failures, transaction rollback and malformed inputs are host tested.

## 3. Exceptions and diagnostics

Complete: installed vectors, all 31 GPRs and control-state round trip, classified
kernel/user faults, bounded validated frame traces, exact-ELF offline symbol lookup,
and a structured diagnostic ring with loss accounting. Terminal user faults return
to a surviving parent; demand paging and resumption of the faulting instruction
are outside the first-release boundary.

## 4. Memory management

Complete: DTB/image reservations, fallible physical allocation, final permission
separation, exact device maps, retained direct-map ownership, guarded fallible heap,
and owned guarded stacks. QEMU tests exercise byte/metadata exhaustion, allocator
exhaustion/recovery, range endpoints, heap/stack guards, RO/NX faults, mapping
publication/retirement, real stack entry and frame reclamation. Host failure
injection verifies preflight-before-commit behavior.

The planner now mutates its private plan in place: an observed debug-stack overflow
was corrected without growing the 1 MiB construction stack. The separate image
reservation and permission windows are recorded in [docs/0.1-DECISIONS.md](docs/0.1-DECISIONS.md).

## 5. Interrupts and time

Complete: DTB-selected GICv3 and physical timer, pinned publication, ordered
activation, monotonic counter observations, transactional timer queue, exact
post-EOI one-shot consumption, and bounded shutdown. QEMU proves IRQ deferral while
an IRQ-save lock is held and delivery after its exact mask restoration.

## 6. Kernel execution

Complete for the focused integration: three-slot round-robin scheduler, two workers
on independent guarded 32 KiB stacks, 2 ms timer preemption, absolute sleep, idle,
exit and reclamation. Switching occurs at the exception epilogue after lock/borrow
lifetimes end. Single-core execution and no nested interrupts are explicit limits.
The user-system image uses serialized process execution rather than this worker probe.

## 7. Processes and native ABI

Complete: independently owned address spaces and initial frames, native SVC calls,
checked cross-page user copies, generation-checked lifecycle, Waiting parent,
child exit/fault recovery and resource reclamation. Root/ASID readback and global
TLB completion precede child frame/ASID reuse. Nine deliberate fault cases verify
that user code cannot access kernel memory, retired low aliases or guard pages.
The syscall ABI remains native and unstable, with documented capacities and errors.

## 8. Executables and minimal system

Complete: strict ELF loader, deterministic newc initramfs, native argc/argv/envp/auxv,
read-only archive plus writable tmp files, process-local descriptors, console
input/output, independent child programs and a deterministic terminal result.
QEMU exercises descriptor/file/byte exhaustion, failed-copy offset rollback,
valid and invalid cross-page copies, and complete child resource restoration.

## 9. Release preparation and acceptance

Technical evidence includes canonical CI, focused target tests, unsafe review,
exact tool/source provenance, retained transcripts and repeatable artifact hashes.
The package is the tested debug image. The reviewable contract explicitly excludes
unsupported architectures, devices and execution modes.

After technical gates pass:

1. Set `0.1.0-pre.1`, move accumulated completed changes into that Changelog entry,
   and create its annotated candidate tag.
2. Submit the concrete candidate and test report for owner review.
3. Implement requested corrections and issue `0.1.0-pre.2`, etc. as needed.
4. Only after explicit acceptance, set `0.1.0`, finalize its Changelog and create
   the annotated `v0.1.0` tag. Follow the `dev` to `main` review flow.

Passing automated tests authorizes a candidate, not the final release.

## First-release exclusions

Persistent storage, networking, SMP, loadable modules, concurrent userspace
scheduling, demand paging, signals, dynamic linking/TLS, Linux syscall/binary
compatibility, POSIX completeness, real hardware, RISC-V and x86_64 are deferred.
Future capacity increases and new execution contexts require fresh validation.
