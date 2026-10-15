# Phoenix Feature Catalog

This catalog records the known capability surface of a potentially large kernel. It is broader
than the active plan and is expected to grow. An entry here is not a promise or authorization to
implement it.

## States

- **Captured**: recorded but not yet analyzed.
- **Deferred**: understood to be useful, but deliberately unscheduled.
- **Planned**: placed on the ordered path to a release.
- **In Progress**: implementation is active.
- **Implemented**: implemented and validated at the stated scope.
- **Rejected**: considered and deliberately excluded, with a reason.

Use `Unscheduled` as the release target when timing is unknown. Stable IDs should not be reused
after an entry is removed or rejected.

## Inbox

Put quick, unclassified discoveries here. Periodically move them into the catalog with an ID,
state, dependencies, and motivation.

_No unclassified entries._

## Foundation and release engineering

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| BUILD-001 | Stable Rust workspace with declared MSRV | Implemented | 0.1.0 | Initial engineering baseline |
| BUILD-002 | Target-aware rust-analyzer configuration | Implemented | 0.1.0 | Default target lives in `.cargo/lsp.toml` |
| BUILD-003 | Host-side canonical validation | Implemented | 0.1.0 | Provided by xtask |
| BUILD-004 | Kernel artifact and image packaging | Implemented | 0.1.0 | ELF, raw image, linker map, and static inspection |
| BUILD-005 | Reproducible release artifacts and provenance | Implemented | 0.1.0 | System artifact directory, exact tool/source provenance and SHA-256 checksums; reproduction uses the recorded checkout path and tools |
| BUILD-006 | Explicit host/static/runtime validation states | Implemented | 0.1.0 | Runtime debt is recorded separately, never implied by compilation |
| BUILD-008 | Automated QEMU runtime CI | Implemented | 0.1.0 | Recorded Rust/QEMU, all target suites, serial logs and clean-source artifact upload |
| BUILD-007 | Early-stack footprint regression check | Captured | Unscheduled | A first final-map QEMU run exceeded the former 512 KiB stack; automate a build-time bound before shrinking the current 1 MiB temporary stack |

## Boot and firmware

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| BOOT-001 | AArch64 QEMU `virt` direct boot | Implemented | 0.1.0 | Pinned virt-9.1 board; current transcripts in docs/RUNTIME_VALIDATION.md |
| BOOT-002 | Early stack, BSS initialization, panic, and halt | Implemented | 0.1.0 | Full BSS, stack bounds/alignment, intentional panic and guest test exit are Runtime Verified |
| BOOT-003 | Device Tree discovery and ownership | Implemented | 0.1.0 | Strict DTB parsing and reserved lifetime; real boot DTB feeds the reservation-filtered allocator |
| BOOT-004 | UEFI boot | Deferred | Unscheduled | Future firmware path |
| BOOT-005 | Multiboot-compatible x86_64 boot path | Deferred | Unscheduled | Decide with x86_64 port |
| BOOT-006 | Shutdown and reboot | Deferred | Unscheduled | Platform-specific mechanisms |

## Architecture and CPU

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| ARCH-001 | AArch64 execution and exception-level normalization | Implemented | 0.1.0 | EL2 and direct EL1 boot both verified; EL1 kernel, soft-float and one CPU |
| ARCH-002 | Exception vectors and register context | Implemented | 0.1.0 | All GPRs, SP_EL0, status and ERET round-trip; controlled faults and epilogue context switching verified |
| ARCH-003 | CPU feature discovery | Deferred | Unscheduled | Avoid assuming emulator-only features |
| ARCH-004 | FP/SIMD ownership and context policy | Deferred | Unscheduled | Kernel starts without FP/SIMD use |
| ARCH-005 | AArch64 ASID allocation and TTBR0 encoding | Implemented | 0.1.0 | Root/ASID readback; retired child IDs reused only after global TLB completion; host exhaustion/reuse and 705 target child lifetimes tested |
| PORT-001 | RISC-V 64 port | Deferred | Unscheduled | Initial platform: QEMU `virt` |
| PORT-002 | x86_64 port | Deferred | Unscheduled | Initial platform: QEMU `q35` |

## Memory management

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| MM-001 | Physical page allocator | Implemented | 0.1.0 | DTB reservations, range endpoint accesses, exhaustion/recovery and child frame accounting |
| MM-002 | Virtual memory and kernel address space | Implemented | 0.1.0 | Eight-table final TTBR1, exact device pages, protected text/rodata, owned RAM capability and transactional L3 updates; RO/NX/low-alias faults verified |
| MM-003 | Kernel heap and fallible allocation policy | Implemented | 0.1.0 | Focused four-page/four-record heap with two guards; fallible allocation and target byte/metadata exhaustion; no global allocator or concurrent consumer |
| MM-004 | Kernel stacks and guard pages | Implemented | 0.1.0 | Owned 32 KiB worker stacks, mapped guard faults, actual entry/preemption and retirement before reclamation; no emergency exception stack |
| MM-005 | User address spaces and safe user copies | Implemented | 0.1.0 | Isolated ELF/stack/table ownership and complete checked cross-page copies; terminal user faults return to parent |
| MM-006 | Shared memory and copy-on-write | Deferred | Unscheduled | Requires process VM |
| MM-007 | Huge pages and block mappings | Captured | Unscheduled | Revisit after basic paging |
| MM-008 | DMA, cache coherence, and IOMMU policy | Deferred | Unscheduled | Required for robust device support |
| MM-009 | OOM and resource exhaustion behavior | Implemented | 0.1.0 | Explicit bounded allocator/heap/process/ASID/file limits; host rollback and target exhaustion/recovery tests |
| MM-010 | Checked physical/virtual addresses, pages, frames, and ranges | Implemented | 0.1.0 | Host Tested and Cross Compiled |

## Concurrency, time, and SMP

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| CONC-001 | Atomics and memory-ordering policy | Implemented | 0.1.0 | Acquire/Release publication and IRQ-save exclusion; context switch/TLB ownership contracts in 0.1-CONTRACT.md |
| CONC-002 | Spinlocks and IRQ-safe synchronization | Implemented | 0.1.0 | Target IRQ deferred across lock, then exact restoration and post-EOI callback; no migration or nested IRQs |
| SCHED-001 | Kernel threads and context switching | Implemented | 0.1.0 | Focused two-worker integration with retained stack owners and complete exception-frame switching |
| SCHED-002 | Scheduler, preemption, and idle | Implemented | 0.1.0 | Bounded round robin, 2 ms timer preemption, idle, sleep, exit and stack reap; user-system execution remains synchronous |
| SCHED-003 | Cooperative user-process scheduling | Implemented | 0.2.0 | Two-context round robin, block/wake, single-child spawn/wait/yield; 64 QEMU rounds, 257 reaps and 128 faults; development version, no release tag |
| SCHED-004 | User timer preemption | Implemented | 0.2.0 | EL0-only IRQ enable, non-yielding progress, 31-GPR/NZCV/SP/private-stack checks and real handoff counters; no-timer guest success rejected |
| SCHED-005 | User sleep and no-runnable idle | Implemented | 0.2.0 | Bounded native sleep, rounded-up deadlines, naked EL1h WFI on retained stack; 32 balanced wakes, idle IRQs and repeated slot reuse verified |
| TIME-001 | Monotonic clock and generic timer | Implemented | 0.1.0 | Read-only physical counter, checked frequency conversion and real GIC timer delivery |
| TIME-002 | Timer queue and sleep | Implemented | 0.1.0 | Transactional deadline queue and one-shot identity checks; post-EOI callbacks and thread sleep verified |
| SMP-001 | Secondary CPU bring-up and per-CPU data | Deferred | Unscheduled | Preserve interfaces before enabling SMP |
| SMP-002 | IPI and TLB shootdown | Deferred | Unscheduled | Coordinate with virtual memory design |

## Processes and userspace

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| PROC-001 | Process and thread lifecycle | Implemented | 0.1.0 | Generation IDs, Waiting parent, terminal child outcomes, root switch, exact frame-set/ASID/slot recovery, 705 lifetimes, stale PID rejection and stack/FD reuse |
| ABI-001 | Native Phoenix syscall ABI | Implemented | 0.1.0 | Revision 0 SVC calls 0–5; bounded serialized system dispatcher with explicit errors; native unstable ABI |
| ABI-002 | ELF loader, user stack and auxiliary vector | Implemented | 0.1.0 | Strict static ELF subset, guarded four-page stack and native argc/argv/envp/minimal auxv; TLS tracked separately |
| ABI-003 | Separately linked first native user program | Implemented | 0.1.0 | Independent init/child/fault ELF programs; normal execution and nine isolated user-fault cases verified |
| ABI-004 | Userspace TLS | Deferred | Unscheduled | Requires a defined thread-pointer ABI and per-thread ownership; not needed by the initial static ELF programs |
| PROC-002 | Signals and exception delivery | In Progress | Unscheduled | Terminal child fault classification and parent notification implemented; signals and faulting-instruction resumption deferred |
| IPC-001 | Pipes, message passing, and shared memory IPC | Deferred | Unscheduled | Split when designs become concrete |
| IPC-002 | Wait, poll, and event readiness | Captured | Unscheduled | Coordinate with file descriptors |
| SYNC-001 | Futex-like userspace synchronization | Captured | Unscheduled | Requires stable user-memory semantics |
| SEC-001 | Credentials, permissions, and capabilities | Deferred | Unscheduled | Must inform VFS interfaces |

## Filesystems and storage

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| VFS-001 | VFS, path lookup, and file descriptor model | Implemented | 0.1.0 | Fixed root with archive/tmp lookup, process-local descriptors and transactional offsets; no general mount mutation or credentials |
| FS-001 | Initramfs and in-memory filesystem | Implemented | 0.1.0 | Strict deterministic newc plus eight writable 1024-byte tmp files; capacity failure and recovery verified |
| BLOCK-001 | Block layer, buffering, and cache | Deferred | Unscheduled | Define flush and error semantics |
| FS-002 | Persistent filesystem | Captured | Unscheduled | Format not selected |
| STORAGE-001 | Partition discovery | Captured | Unscheduled | GPT is a likely first format |

## Devices and drivers

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| DEV-001 | Minimal device and driver lifecycle | Deferred | Unscheduled | Do not build a framework before two users exist |
| DEV-002 | Early and runtime console | Implemented | 0.1.0 | Polled PL011 output/flush and nonblocking input; panic and system console verified |
| IRQ-001 | Interrupt controller abstraction and GIC | Implemented | 0.1.0 | Pinned GICv3/timer publication, activation, PPI delivery, EOI, handler return and shutdown verified |
| FW-001 | PSCI integration | Deferred | Unscheduled | Power and SMP services |
| VIRTIO-001 | VirtIO transport and device discovery | Deferred | Unscheduled | MMIO first; PCI may follow |
| PCI-001 | PCI discovery and configuration | Deferred | Unscheduled | Important for x86_64 and future platforms |
| USB-001 | USB host and input/storage classes | Captured | Unscheduled | Far-future capability |
| RNG-001 | Entropy collection and random generator | Deferred | Unscheduled | Security-sensitive initialization |

## Networking

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| NET-001 | Network-device interface and packet buffers | Deferred | Unscheduled | Coordinate DMA ownership |
| NET-002 | Ethernet, ARP, IPv4, and ICMP | Deferred | Unscheduled | Initial network layer |
| NET-003 | UDP and socket interface | Deferred | Unscheduled | First transport path |
| NET-004 | TCP | Deferred | Unscheduled | Requires timers and robust testing |
| NET-005 | IPv6 | Captured | Unscheduled | Scope not defined |
| NET-006 | Routing and packet filtering | Captured | Unscheduled | Security model dependency |

## Reliability, security, and observability

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| OBS-001 | Structured logging and runtime ring buffer | Implemented | 0.1.0 | 32-record IRQ-locked structured ring, monotonic sequence and loss count; nonblocking panic snapshot |
| OBS-002 | Panic register dump, symbols, and stack traces | Implemented | 0.1.0 | Fatal control-register report, sixteen-frame validated trace and exact-ELF offline symbol resolution |
| OBS-003 | Tracing and profiling | Captured | Unscheduled | Avoid committing to a format early |
| TEST-001 | QEMU boot smoke and focused probes with timeout and sentinels | Implemented | 0.1.0 | Five legacy runs, thirteen runtime cases, five system scenarios and target hardening; exact fault oracle, wrong-fault rejection and QEMU CI |
| TEST-002 | Host-side unit and property tests | Implemented | 0.1.0 | Console, build identity, config, and ELF-layout logic |
| TEST-003 | Fuzzing and fault injection | Deferred | Unscheduled | Add with parsers and failure paths |
| HARD-001 | Unsafe-code review and invariant audit | Implemented | Continuous | Current unsafe ownership/order self-review in docs/UNSAFE_REVIEW.md; no independent security audit implied |
| HARD-002 | Resource limits and fault isolation | In Progress | Unscheduled | Bounded native-system limits, cross-page validation and fault containment verified; broader security model deferred |
| HARD-003 | Secure boot and image verification | Captured | Unscheduled | Firmware-policy dependency |
| POWER-001 | Idle states and power management | Captured | Unscheduled | Beyond the initial halt path |

## Catalog review

Periodically, and before the first release:

1. classify Inbox entries;
2. add newly discovered capability domains or dependencies;
3. verify that `Planned` items appear in `ROADMAP.md`;
4. verify that implemented support claims match README and `CHANGELOG.md`;
5. reconsider, but do not automatically promote, deferred work.
