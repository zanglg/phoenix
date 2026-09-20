# AArch64 Interrupt Topology

Phoenix currently discovers, maps, and prepares—but does not activate—the GICv3 and
architected-timer interrupt topology emitted for the pinned AArch64 QEMU `virt` platform.
Discovery is strict, allocation-free, and isolated under the platform implementation; generic DTB
traversal remains independent of GIC concepts.

## Current implementation

`kernel/src/platform/aarch64/qemu_virt/interrupts.rs` consumes a validated `DeviceTree` event
stream and produces one copyable `InterruptTopology`. The result contains:

- the nonzero GICv3 phandle selected by the root `interrupt-parent`;
- checked, non-overlapping distributor and redistributor physical MMIO ranges; and
- the secure physical, non-secure physical, virtual, and EL2 physical timer PPIs in the order
  defined by the ARM architected-timer binding.

The pinned platform contract requires root `#address-cells = <2>` and `#size-cells = <2>`, one
direct `intc`/`interrupt-controller` child compatible with `arm,gic-v3`, one direct `timer` child
compatible with `arm,armv8-timer`, and a three-cell GIC interrupt format. The GIC node must expose
an empty `interrupt-controller` marker, a nonzero `phandle` or matching `linux,phandle`, exactly
one distributor range and one redistributor range, and at most one redistributor region.

The timer must resolve to that exact GIC through its own or the root's `interrupt-parent`. Phoenix
requires the pinned four-entry `interrupts` form and validates every entry as a distinct standard
PPI. The second entry becomes the non-secure physical PPI paired with the current `CNTP_*` owner.

`kernel/src/arch/aarch64/gicv3.rs` now provides the architecture-side preparation state machine:

- `PrivatePeripheralInterrupt` admits only INTIDs 16 through 31 and carries the checked trigger;
- `GicV3RegisterAccess` is an unsafe backend contract for unique ordered GICD, selected-GICR, and
  current-CPU `ICC_*_EL1` access;
- `GicV3Configuration` carries the timer PPI, a deliverable priority below `0xff`, and a nonzero
  per-poll read budget; and
- `prepare` consumes the backend and returns `PreparedGicV3` only while every delivery gate remains
  closed.

Preparation validates GICD/GICR PIDR2 as GICv3 or GICv4, enables and reads back
`ICC_SRE_EL1.SRE`, closes CPU Group-1 and priority gates, disables distributor groups without ever
clearing an already enabled `ARE_NS`, enables affinity routing when needed, wakes the selected
redistributor, and bounds every `RWP`/`ChildrenAsleep` poll. It then disables and clears all SGIs
and PPIs, assigns them to non-secure Group 1, updates only the selected PPI's trigger and priority
fields, enables that PPI, selects combined EOI/deactivation, and leaves both distributor and CPU
Group-1 delivery disabled.

The final-map and loaded-init builders now connect topology discovery to the real boot DTB. While
the bootstrap low Device-nGnRnE identity window is still active, a checked exclusive reader:

- converts the current `MPIDR_EL1` affinity to the packed `GICR_TYPER` form;
- validates every candidate's PIDR2 revision, follows 128 KiB RD/SGI frames while skipping the
  additional 128 KiB VLPI frame when advertised, and requires an architectural `Last` marker;
- rejects a missing, duplicate, truncated, inaccessible, or unsupported-affinity match; and
- retains only the matching 128 KiB RD/SGI physical pair plus copied topology evidence.

The final TTBR1 plan maps the complete DTB-declared GICD range and only that selected GICR pair as
individual EL1-only Device-nGnRnE RW/NX 4 KiB pages. The platform retains the checked physical and
virtual layout beside the permanent table owner. `MappedGicV3Registers` is the concrete non-copying
AArch64 backend: its unsafe constructor binds that active table owner and exact layout, every MMIO
access is aligned, range checked, and volatile, each `ICC_*_EL1` variant names one fixed system
register, and `DSB SY; ISB` implements the preparation protocol's synchronization hook. No boot
path can construct the backend before TTBR1 publication.

When Phoenix enters at EL2, `boot.S` preserves the existing `ICC_SRE_EL2` value while setting
`SRE`, `DFB`, `DIB`, and `Enable`, then executes an `ISB` before dropping to EL1. A direct EL1
entry remains a firmware contract: the higher exception levels must already permit EL1 system-
register access. The preparation transaction still sets and reads back `ICC_SRE_EL1.SRE`; an
unpermitted access can trap and therefore is not represented as an ordinary Rust error.

After each final-map path installs its permanent runtime owner and activates TTBR1, it checks that
`DAIF.I` is still set, constructs `MappedGicV3Registers` from the retained tables and copied
discovery evidence, and calls `prepare` with the DTB-selected non-secure physical timer PPI. Every
hardware wait receives a fixed 100,000-read budget. Success is installed once in a separate static
`PreparedGicV3<MappedGicV3Registers<...>>`, preserving the table borrow and unique register owner
for the rest of boot. Only then is `PHOENIX_GIC_PREPARED` emitted. Both Group-1 delivery gates and
the architectural IRQ mask remain closed.

## Invariants and rejection policy

- Duplicate root properties, required nodes, or consumed node properties are rejected.
- Missing, malformed, disabled, or incompatible GIC/timer nodes are rejected rather than replaced
  with fixed addresses or interrupt IDs.
- GICD starts on a 64 KiB boundary and spans at least 64 KiB. GICR starts on a 128 KiB boundary
  and spans at least 128 KiB. Both ranges must be 4 KiB sized, below the current 48-bit physical
  limit, nonempty, non-overflowing, representable by `usize`, and mutually disjoint.
- A three-cell GICv3 PPI uses type `1`, a relative number in `0..=15`, and only trigger value `1`
  (edge rising) or `4` (level high). Reserved flag bits are rejected. PPI affinity is not encoded
  in the third cell of this GICv3 contract.
- The four timer INTIDs must be unique. The parser never silently substitutes the conventional
  non-secure physical INTID 30 when firmware describes something else.
- Preparation requires externally masked IRQs, one uniquely owned backend, and the current CPU's
  exact redistributor. The unsafe backend contract requires volatile, correctly sized, ordered
  access and forbids concurrent handler/CPU mutation.
- Every hardware-state wait consumes a finite caller-selected read budget. Timeout and backend
  access failures identify their exact preparation stage; there is no unbounded early-boot spin.
- A failed preparation retains backend ownership, the primary error, and the first error from
  best-effort recovery. Recovery closes CPU/distributor group gates and disables private sources,
  but never clears `ARE_NS`; a recovery error explicitly means hardware may remain partially
  programmed and external IRQ masking must continue.
- Redistributor discovery is read-only and bounded by the DTB range and bootstrap device window.
  `GICR_TYPER.Last` terminates the scan; Phoenix neither assumes that frame zero owns the boot CPU
  nor maps every redistributor in the region.
- Final GIC mappings are exact L3 device leaves. The mapping proof checks every page's physical
  output, level, and attributes before table allocation, and the retained layout does not itself
  authorize a register access.
- Concrete backend construction remains unsafe because uniqueness across GICD, the selected GICR,
  and current-CPU `ICC_*_EL1` state cannot be proven by the copyable offline layout. Its contract
  requires the exact active tables, masked IRQs, and no concurrent CPU or handler access.
- `InterruptTopology` is descriptive evidence only. Possessing it does not authorize MMIO access,
  map device pages, enable the distributor/redistributor, unmask `DAIF.I`, or satisfy the unsafe
  preconditions of `PhysicalTimer::unmask`.
- Prepared boot ownership is published exactly once and outlives the active final table hierarchy.
  A copied topology/layout is evidence, not a second register capability. A preparation failure
  attempts closed-gate recovery and then terminates boot without publishing a prepared owner.
- `PHOENIX_GIC_PREPARED` means the bounded transaction returned success with delivery closed; it
  does not mean an IRQ was delivered, acknowledged, or returned from.

The parser contains no unsafe code and retains no reference into the DTB after returning copied
topology values.

## Synchronization and ownership boundary

The final-map boot paths now construct and permanently retain the unique owner binding the mapped
GICD, selected boot CPU GICR, current CPU interface, and active final tables. Preparation occurs
with IRQs masked and before delivery is enabled. The owner is not exposed to general kernel code.
Once IRQ dispatch exists, normal context and handlers must share state only through an IRQ-safe
lock or a documented single-owner protocol with acquire/release ordering. No callback, scheduler
action, allocator use, or lock acquisition with an inverted order may occur while acknowledging an
interrupt.

The current boot path still does not open Group-1 delivery, unmask `DAIF.I`, acknowledge an INTID,
or define the interrupt-context lock hierarchy.

## Validation

Host tests construct complete DTBs and cover the expected GICD/GICR ranges, all four timer PPI
roles, inherited and local parent links, disabled nodes, duplicate controllers, short register and
interrupt lists, range alignment, non-PPI entries, reserved flag bits, and duplicate INTIDs.
Architecture tests additionally cover PPI/priority validation, level/edge read-modify-write fields,
closed delivery gates, SRE/PIDR rejection, repeated preparation without ARE rollback, bounded wake
timeout, injected access failure, stage attribution, and best-effort recovery. Canonical CI
also covers MPIDR packing, VLPI-aware redistributor stride, exact affinity selection, `Last`,
duplicate/missing/truncated/access failures, bootstrap-window bounds, register-range checks, and
all 48 final GIC L3 leaves in the pinned 8-table/815-leaf plan. It cross-compiles the volatile
MMIO/system-register backend, `ICC_SRE_EL2` setup, checked `DAIF.I` observation, one-time retained
preparation in both final-map consumers, and the `PHOENIX_GIC_PREPARED` static/protocol contract.
Manual ELF disassembly of `boot_drop_el2_to_el1` also confirms the ordered
`MRS ICC_SRE_EL2`/`ORR #0xf`/`MSR ICC_SRE_EL2`/`ISB` sequence in the built default kernel.
These checks do not prove that the recorded QEMU board emits the expected nodes, that selection or
preparation succeeds on target, or that any final GIC register can be accessed.

## TODO

- Add a separate type-state transition that opens Group-1 delivery only after IRQ dispatch is
  published, then define a recoverable shutdown transition.
- Define IRQ entry/exit, spurious-ID handling, priority, acknowledge/deactivate/EOI ordering, and
  explicit behavior for an unexpected enabled interrupt.
- Add IRQ-safe synchronization, connect the timer queue, and program the discovered non-secure
  physical PPI only after the handler is ready.
- Add a bounded target probe that records the actual topology, one timer interrupt, ordered event
  dispatch, EOI, and continued execution.

## Deliberately skipped

This checkpoint does not support GICv2, multiple redistributor regions, legacy GICC/GICH/GICV
ranges, ITS/MSI/LPI, shared peripheral interrupts, four-cell PPI partitions, `interrupts-extended`,
the optional VHE virtual timer interrupt, SMP routing, priority policy, or nested interrupts. The
concrete backend and boot preparation path are Cross Compiled but have not executed on target.
Phoenix does not enable distributor/CPU group delivery, acknowledge an interrupt, write EOI,
deactivate, or return through an IRQ vector. Those behaviors are not inferred from valid topology,
offline mappings, a Host Tested register protocol, or the unobserved preparation marker.

## Runtime Pending

`RUN-IRQ-001` and `RUN-TIME-001` in `docs/RUNTIME_VALIDATION.md` remain Runtime Pending. The prior
positive QEMU runs predate boot-time GIC selection and the current eight-table final image.
Host-built fixtures cannot prove the content of QEMU's generated DTB, redistributor selection,
EL2/EL1 SRE behavior, preparation completion, GIC register semantics, PPI delivery,
acknowledgement ordering, or interrupt return.
