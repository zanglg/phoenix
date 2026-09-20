# AArch64 Interrupt Topology

Phoenix currently discovers and maps, but does not initialize or activate, the GICv3 and
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
path constructs this backend or invokes `prepare` yet.

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

The parser contains no unsafe code and retains no reference into the DTB after returning copied
topology values.

## Synchronization and ownership boundary

The concrete backend exists but is not yet constructed by the boot path. Its future retained owner
must uniquely bind the mapped GICD, selected boot CPU GICR, current CPU interface, and active final
tables. Preparation must occur while IRQs remain masked and before delivery is enabled. Once IRQ
dispatch exists, normal context and handlers must share state only through an IRQ-safe lock or a
documented single-owner protocol with acquire/release ordering. No callback, scheduler action,
allocator use, or lock acquisition with an inverted order may occur while acknowledging an
interrupt.

The current boot path selects and maps a redistributor but does not construct the register owner,
run preparation, open Group-1 delivery, unmask `DAIF.I`, or define the interrupt-context lock
hierarchy.

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
MMIO/system-register backend and both final-map boot consumers. These checks do not prove that the
recorded QEMU board emits the expected nodes, that selection succeeds on target, or that any final
GIC register can be accessed.

## TODO

- Configure `ICC_SRE_EL2` during the EL2 bootstrap so EL1 system-register access cannot trap.
- Construct and permanently retain the concrete backend after TTBR1 publication, then invoke the
  failure-retaining preparation transition while IRQs remain masked.
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
concrete backend is Cross Compiled but is not constructed or exercised. Phoenix does not run
preparation, enable distributor/CPU group delivery, acknowledge an interrupt, write EOI,
deactivate, or return through an IRQ vector. Those behaviors are not inferred from valid topology,
offline mappings, or a Host Tested register protocol.

## Runtime Pending

`RUN-IRQ-001` and `RUN-TIME-001` in `docs/RUNTIME_VALIDATION.md` remain Runtime Pending. The prior
positive QEMU runs predate boot-time GIC selection and the current eight-table final image.
Host-built fixtures cannot prove the content of QEMU's generated DTB, redistributor selection, GIC
register semantics, PPI delivery, acknowledgement ordering, or interrupt return.
