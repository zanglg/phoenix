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
| BUILD-005 | Reproducible release artifacts and provenance | Deferred | Unscheduled | Revisit before public releases |
| BUILD-006 | Explicit host/static/runtime validation states | Implemented | 0.1.0 | Runtime debt is recorded separately, never implied by compilation |

## Boot and firmware

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| BOOT-001 | AArch64 QEMU `virt` direct boot | In Progress | 0.1.0 | Image builds; emulator execution is unverified |
| BOOT-002 | Early stack, BSS initialization, panic, and halt | In Progress | 0.1.0 | Implemented and statically checked; runtime unverified |
| BOOT-003 | Device Tree discovery and ownership | In Progress | 0.1.0 | Parsing is Host Tested; checked target borrowing through the bootstrap mapping is Cross Compiled and Runtime Pending |
| BOOT-004 | UEFI boot | Deferred | Unscheduled | Future firmware path |
| BOOT-005 | Multiboot-compatible x86_64 boot path | Deferred | Unscheduled | Decide with x86_64 port |
| BOOT-006 | Shutdown and reboot | Deferred | Unscheduled | Platform-specific mechanisms |

## Architecture and CPU

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| ARCH-001 | AArch64 execution and exception-level normalization | Planned | 0.1.0 | Primary architecture |
| ARCH-002 | Exception vectors and register context | In Progress | 0.1.0 | Vector/frame ABI, assembly entry, and ESR decoding are built; runtime handling remains unverified |
| ARCH-003 | CPU feature discovery | Deferred | Unscheduled | Avoid assuming emulator-only features |
| ARCH-004 | FP/SIMD ownership and context policy | Deferred | Unscheduled | Kernel starts without FP/SIMD use |
| PORT-001 | RISC-V 64 port | Deferred | Unscheduled | Initial platform: QEMU `virt` |
| PORT-002 | x86_64 port | Deferred | Unscheduled | Initial platform: QEMU `q35` |

## Memory management

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| MM-001 | Physical page allocator | In Progress | 0.1.0 | Allocator/map logic is Host Tested; target DTB borrowing and linker-bound derivation are Cross Compiled and Runtime Pending |
| MM-002 | Virtual memory and kernel address space | In Progress | 0.1.0 | Descriptors, plans, and checked bootstrap private-frame access are built; final TTBR1 and runtime validation remain |
| MM-003 | Kernel heap and fallible allocation policy | Planned | 0.1.0 | Define allocation-failure behavior |
| MM-004 | Kernel stacks and guard pages | Planned | 0.1.0 | Include exception-context requirements |
| MM-005 | User address spaces and safe user copies | In Progress | 0.1.0 | Plans/materialization/ownership are Host Tested; ownership-gated ASID-zero activation is Cross Compiled; user copies and runtime evidence remain |
| MM-006 | Shared memory and copy-on-write | Deferred | Unscheduled | Requires process VM |
| MM-007 | Huge pages and block mappings | Captured | Unscheduled | Revisit after basic paging |
| MM-008 | DMA, cache coherence, and IOMMU policy | Deferred | Unscheduled | Required for robust device support |
| MM-009 | OOM and resource exhaustion behavior | Deferred | Unscheduled | Must not be postponed to final hardening |
| MM-010 | Checked physical/virtual addresses, pages, frames, and ranges | Implemented | 0.1.0 | Host Tested and Cross Compiled |

## Concurrency, time, and SMP

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| CONC-001 | Atomics and memory-ordering policy | Planned | 0.1.0 | Document per abstraction |
| CONC-002 | Spinlocks and IRQ-safe synchronization | Planned | 0.1.0 | Define interrupt-context rules |
| SCHED-001 | Kernel threads and context switching | Planned | 0.1.0 | Architecture boundary required |
| SCHED-002 | Scheduler, preemption, and idle | Planned | 0.1.0 | Policy remains open |
| TIME-001 | Monotonic clock and generic timer | Planned | 0.1.0 | Define precision and wrap behavior |
| TIME-002 | Timer queue and sleep | Planned | 0.1.0 | Tickless design remains open |
| SMP-001 | Secondary CPU bring-up and per-CPU data | Deferred | Unscheduled | Preserve interfaces before enabling SMP |
| SMP-002 | IPI and TLB shootdown | Deferred | Unscheduled | Coordinate with virtual memory design |

## Processes and userspace

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| PROC-001 | Process and thread lifecycle | Planned | 0.1.0 | Includes identifiers and teardown |
| ABI-001 | Native Phoenix syscall ABI | In Progress | 0.1.0 | Revision-0 values and probe-only unknown/exit dispatch are built; production dispatch is Runtime Pending |
| ABI-002 | ELF loader, user stack, TLS, and auxiliary vector | In Progress | 0.1.0 | ELF planning/population and native argc/argv/envp/minimal-auxv stack construction are Host Tested; target execution and TLS remain |
| PROC-002 | Signals and exception delivery | Deferred | Unscheduled | Requires process lifecycle |
| IPC-001 | Pipes, message passing, and shared memory IPC | Deferred | Unscheduled | Split when designs become concrete |
| IPC-002 | Wait, poll, and event readiness | Captured | Unscheduled | Coordinate with file descriptors |
| SYNC-001 | Futex-like userspace synchronization | Captured | Unscheduled | Requires stable user-memory semantics |
| SEC-001 | Credentials, permissions, and capabilities | Deferred | Unscheduled | Must inform VFS interfaces |

## Filesystems and storage

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| VFS-001 | VFS, path lookup, and file descriptor model | Planned | 0.1.0 | Permission hooks designed together |
| FS-001 | Initramfs and in-memory filesystem | Planned | 0.1.0 | Initial userspace root |
| BLOCK-001 | Block layer, buffering, and cache | Deferred | Unscheduled | Define flush and error semantics |
| FS-002 | Persistent filesystem | Captured | Unscheduled | Format not selected |
| STORAGE-001 | Partition discovery | Captured | Unscheduled | GPT is a likely first format |

## Devices and drivers

| ID | Capability | State | Release | Notes |
| --- | --- | --- | --- | --- |
| DEV-001 | Minimal device and driver lifecycle | Deferred | Unscheduled | Do not build a framework before two users exist |
| DEV-002 | Early and runtime console | In Progress | 0.1.0 | Polled PL011 implemented; runtime unverified |
| IRQ-001 | Interrupt controller abstraction and GIC | Planned | 0.1.0 | Initial AArch64 controller |
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
| OBS-001 | Structured logging and runtime ring buffer | Planned | 0.1.0 | Builds on early console |
| OBS-002 | Panic register dump, symbols, and stack traces | In Progress | 0.1.0 | Fatal exception path reports vector, PC, status, ESR, and FAR; symbolization and stack traces remain |
| OBS-003 | Tracing and profiling | Captured | Unscheduled | Avoid committing to a format early |
| TEST-001 | QEMU boot smoke and focused probes with timeout and sentinels | In Progress | 0.1.0 | Boot, memory, and EL0 harness variants are implemented; QEMU execution remains Runtime Pending |
| TEST-002 | Host-side unit and property tests | Implemented | 0.1.0 | Console, build identity, config, and ELF-layout logic |
| TEST-003 | Fuzzing and fault injection | Deferred | Unscheduled | Add with parsers and failure paths |
| HARD-001 | Unsafe-code review and invariant audit | Deferred | Continuous | Applies as unsafe code appears |
| HARD-002 | Resource limits and fault isolation | Deferred | Unscheduled | Design with processes and drivers |
| HARD-003 | Secure boot and image verification | Captured | Unscheduled | Firmware-policy dependency |
| POWER-001 | Idle states and power management | Captured | Unscheduled | Beyond the initial halt path |

## Catalog review

Periodically, and before the first release:

1. classify Inbox entries;
2. add newly discovered capability domains or dependencies;
3. verify that `Planned` items appear in `ROADMAP.md`;
4. verify that implemented support claims match README and `CHANGELOG.md`;
5. reconsider, but do not automatically promote, deferred work.
