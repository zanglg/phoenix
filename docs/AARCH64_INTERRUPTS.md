# AArch64 Interrupt Topology

> Construction-stage mechanism record: runtime-status and remaining-work lists below
> describe the earlier integration boundary. Current support, limits and evidence are
> maintained in [0.1-CONTRACT.md](0.1-CONTRACT.md) and
> [RUNTIME_VALIDATION.md](RUNTIME_VALIDATION.md); those records supersede historical
> pending statements here. The ownership design remains applicable unless the
> current contract explicitly refines it.

Phoenix currently discovers, maps, and prepares the GICv3 and architected-timer interrupt
topology emitted for the pinned AArch64 QEMU `virt` platform. Host Tested delivery and pinned
runtime state machines model publication, activation, timer dispatch, EOI/deactivation, and
shutdown. Both final-map boot paths now publish the pinned runtime and dispatcher lookup but
deliberately retain its prepared phase with delivery closed.
Discovery is strict, allocation-free, and isolated under the platform implementation; generic
DTB traversal remains independent of GIC concepts.

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

`kernel/src/arch/aarch64/gicv3.rs` provides the architecture-side preparation and delivery state
machines:

- `PrivatePeripheralInterrupt` admits only INTIDs 16 through 31 and carries the checked trigger;
- `GicV3RegisterAccess` is an unsafe backend contract for unique ordered GICD, selected-GICR, and
  current-CPU `ICC_*_EL1` access;
- `GicV3Configuration` carries the timer PPI, a deliverable priority below `0xff`, and a nonzero
  per-poll read budget;
- `prepare` consumes the backend and returns `PreparedGicV3` only while every delivery gate remains
  closed;
- `PublishedIrqDispatcher` is a non-`Send`, non-`Sync` unsafe proof that every EL1 IRQ vector can
  reach a permanent current-CPU dispatcher, stack, controller-lookup, and synchronization state;
- `activate` consumes both prepared ownership and that proof, opens only Non-secure Group-1
  distribution followed by the current CPU Group-1 interface, and returns `ActiveGicV3`; and
- one `acknowledge` result is either a special INTID with no EOI obligation or a unique token that
  must perform combined EOI/deactivation before another acknowledgement or shutdown.

`kernel/src/arch/aarch64/interrupt_runtime.rs` composes that protocol with the locked timer
service. Its non-transferable, pinned slot is published while delivery is closed, verifies the
timer PPI match before activation, creates timer-programming authority only after GIC activation,
and performs timer reprogramming before EOI. Empty-service deactivation disarms the timer before
closing the CPU and distributor gates. The complete contract is documented in
`docs/AARCH64_INTERRUPT_RUNTIME.md`.

Preparation validates GICD/GICR PIDR2 as GICv3 or GICv4, enables and reads back
`ICC_SRE_EL1.SRE`, closes CPU Group-1 and priority gates, disables distributor groups without ever
clearing an already enabled `ARE_NS`, enables affinity routing when needed, wakes the selected
redistributor, and bounds every `RWP`/`ChildrenAsleep` poll. It then disables and clears all SGIs
and PPIs, assigns them to non-secure Group 1, updates only the selected PPI's trigger and priority
fields, enables that PPI, selects combined EOI/deactivation, and leaves both distributor and CPU
Group-1 delivery disabled.

Activation is a separate unsafe transition and never changes `DAIF.I`. It first requires the CPU
gate and both visible distributor group gates to be closed, affinity routing to remain enabled,
and `ICC_CTLR_EL1.EOImode` to retain the prepared combined priority-drop/deactivation mode. It
then enables `GICD_CTLR.EnableGrp1A`, the affinity-routing alias for Non-secure Group 1 (and the
single-Security-state Group-1 bit), waits with the retained finite budget, verifies the result, and
only then enables `ICC_IGRPEN1_EL1`. Activation failure attempts best-effort closure and retains
the primary error, any recovery error, the backend, configuration, and dispatcher proof.

An active owner reads `ICC_IAR1_EL1`, keeps only the architectural 24-bit INTID, returns IDs
1020 through 1023 as special without writing EOI, and classifies every other value as the configured
timer or an unexpected interrupt. Both classifications carry the same exclusive acknowledgement
token: even an unexpected real INTID must be completed. Completion writes `ICC_EOIR1_EL1` in
combined mode and returns the still-outstanding token on access failure. The active owner also
remembers that INTID until completion succeeds; dropping a token therefore blocks both another
acknowledge and shutdown instead of forgetting the hardware obligation. Unsafe shutdown requires
the caller to mask `DAIF.I`, prevent handler entry, and have no outstanding token; it closes the CPU
gate before both distributor aliases and returns prepared ownership plus the dispatcher proof.
Shutdown failure retains the backend as explicitly indeterminate rather than manufacturing a
prepared state.

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

After each final-map path installs its permanent memory owner and activates TTBR1, it checks that
`DAIF.I` is still set, constructs `MappedGicV3Registers` from the retained tables and copied
discovery evidence, and calls `prepare` with the DTB-selected non-secure physical timer PPI. Every
hardware wait receives a fixed 100,000-read budget. It then claims and disables the physical
timer, reads its initial counter, builds the one-event locked service, pins the combined runtime in
static storage, and Release-publishes a permanent handle used by the common IRQ dispatcher. Only
then are `PHOENIX_GIC_PREPARED` and `PHOENIX_IRQ_RUNTIME_PUBLISHED` emitted. Both Group-1 delivery
gates, timer signaling, and the architectural IRQ mask remain closed.

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
  requires the exact active tables, masked IRQs until dispatcher-backed activation, and no
  concurrent CPU or handler access. The concrete backend itself is neither `Send` nor `Sync`.
- Delivery activation requires a non-transferable published-dispatcher proof and keeps `DAIF.I`
  under caller control. Distributor delivery opens before the CPU gate; shutdown closes the CPU
  gate before distributor delivery. Every wait remains bounded.
- `EnableGrp1A` is used only while affinity routing is enabled. The legacy `EnableGrp1` alias and
  the Group-0 path remain closed; activation readback rejects any contradictory gate state.
- Combined EOI/deactivation is revalidated before activation. IDs 1020 through 1023 are never
  written to `ICC_EOIR1_EL1`; every other acknowledged ID, including an unexpected one, retains an
  exclusive completion obligation recorded in both the token and active owner. An EOI failure or
  dropped token cannot silently discard that obligation or authorize another acknowledgement.
- The pinned runtime must be reachable from every current-CPU EL1 IRQ vector before it constructs
  the dispatcher proof. Its only nested lock order is outer runtime/GIC before inner timer.
- Timer programming requires a private permit created after successful GIC activation. Dispatch
  reprograms or disarms the level-triggered source before EOI; shutdown rejects queued timers and
  disarms empty hardware before closing delivery gates.
- Failed activation retains both primary and recovery evidence. Failed shutdown retains an
  indeterminate active owner and cannot be mistaken for `PreparedGicV3`.
- `InterruptTopology` is descriptive evidence only. Possessing it does not authorize MMIO access,
  map device pages, enable the distributor/redistributor, unmask `DAIF.I`, or satisfy the unsafe
  preconditions of `PhysicalTimer::unmask`.
- Combined boot ownership is published exactly once and outlives the active final table hierarchy.
  A copied topology/layout is evidence, not a second register capability. A preparation failure
  attempts closed-gate recovery and then terminates boot before publishing the runtime handle.
- Ordered `PHOENIX_GIC_PREPARED`/`PHOENIX_IRQ_RUNTIME_PUBLISHED` means bounded GIC preparation plus
  pinned runtime/IRQ-lookup publication returned success with GIC delivery and timer signaling
  closed; it does not mean an IRQ was delivered, acknowledged, or returned from.

The parser contains no unsafe code and retains no reference into the DTB after returning copied
topology values.

## Synchronization and ownership boundary

The final-map boot paths construct and permanently retain the unique owner binding the mapped
GICD, selected boot CPU GICR, current CPU interface, and active final tables. Preparation occurs
with IRQs masked and before delivery is enabled. The owner is not exposed to general kernel code.
The delivery API additionally makes the dispatcher publication proof and concrete mapped backend
non-transferable across CPUs. The Host Tested pinned runtime now owns the GIC and timer behind an
outer IRQ-save lock and acquires the timer's inner lock only in that direction. It returns due data
after EOI and both locks release. One-shot scheduling retains the outer runtime guard while it
releases the timer guard and acquires the consumer guard; rollback releases the consumer before
reacquiring the timer, so both inner locks are never held together. Delivery-time consumption is
acquired only after EOI and all device locks release. No callback, scheduler action, allocator use,
console output, or inverted lock acquisition may occur while an interrupt is outstanding. Boot now
adopts the owner and lookup only in the closed-delivery `Prepared` phase.

The current boot path retains the combined runtime in `Prepared`, so it does not create the
internal `PublishedIrqDispatcher` activation proof, open either GIC gate, program/unmask the
timer, or unmask `DAIF.I`. IRQ slots can look up the runtime and apply the documented dispatch/
fatal policy, but no target IRQ can intentionally reach them; FIQ slots remain purely fatal.

## Validation

Host tests construct complete DTBs and cover the expected GICD/GICR ranges, all four timer PPI
roles, inherited and local parent links, disabled nodes, duplicate controllers, short register and
interrupt lists, range alignment, non-PPI entries, reserved flag bits, and duplicate INTIDs.
Architecture tests additionally cover PPI/priority validation, level/edge read-modify-write fields,
closed delivery gates, SRE/PIDR rejection, repeated preparation without ARE rollback, bounded wake
timeout, injected access failure, stage attribution, and best-effort recovery. Delivery tests cover
distributor-before-CPU activation, CPU-before-distributor shutdown, exact gate-bit isolation,
combined-mode rejection, finite activation and recovery timeouts, special-ID handling, 24-bit INTID
masking, timer and unexpected classification, EOI ordering, retained EOI retry ownership, and
indeterminate shutdown failure. They also verify that dropping an acknowledgement blocks later
acknowledgement and shutdown. Interrupt-runtime tests cover one-way pinned publication, PPI
mismatch, ordered activation/deactivation, timer-before-EOI dispatch, pending-event shutdown
rejection, terminal activation/EOI failures, atomic one-shot schedule/bind with cancellation
rollback, permanent closure after rollback failure, and exact consumption after EOI. Canonical CI
also covers MPIDR packing, VLPI-aware redistributor stride, exact affinity selection, `Last`,
duplicate/missing/truncated/access failures, bootstrap-window bounds, register-range checks, and
all 48 final GIC L3 leaves in the pinned 8-table/815-leaf plan. It cross-compiles the volatile
MMIO/system-register backend including `ICC_IAR1_EL1` and `ICC_EOIR1_EL1`, `ICC_SRE_EL2` setup,
checked `DAIF.I` observation, one-time retained preparation in both final-map consumers, and the
static runtime/handle publication plus both ordered progress-marker contracts.
Manual ELF disassembly of `boot_drop_el2_to_el1` also confirms the ordered
`MRS ICC_SRE_EL2`/`ORR #0xf`/`MSR ICC_SRE_EL2`/`ISB` sequence in the built default kernel.
These checks do not prove that the recorded QEMU board emits the expected nodes, that selection or
preparation succeeds on target, that any final GIC register can be accessed, or that the Host
Tested delivery transaction can handle a real target interrupt.

## TODO

- Activate the already-published runtime from boot without weakening the masked-transition
  contract, then invoke the Host Tested schedule/bind transaction before local IRQ recognition.
- Define the fatal/recovery policy after an unexpected real INTID is completed and preserve full
  IRQ entry/exit register-state evidence.
- Add the final boot transition that schedules a bounded event, activates the published runtime,
  and unmasks `DAIF.I` only after every handler invariant is live.
- Add a bounded target probe that records the actual topology, one timer interrupt, ordered event
  dispatch, EOI, and continued execution.

## Deliberately skipped

This checkpoint does not support GICv2, multiple redistributor regions, legacy GICC/GICH/GICV
ranges, ITS/MSI/LPI, shared peripheral interrupts, four-cell PPI partitions, `interrupts-extended`,
the optional VHE virtual timer interrupt, SMP routing, priority policy, or nested interrupts. The
concrete backend, physical-timer adapter, and boot preparation path are Cross Compiled but have not
executed on target. Boot Cross Compiles the publication transition and lookup, while Host Tested
APIs model distributor/CPU delivery, timer dispatch, EOI/deactivation, and shutdown. No boot path
calls activation or unmasks IRQs, so Phoenix still performs none of those delivery operations on
target and cannot claim a return through an IRQ vector. Those behaviors are not inferred from
valid topology, offline mappings, the Host Tested register protocol, or the unobserved publication
markers.

## Runtime Pending

`RUN-IRQ-001` and `RUN-TIME-001` in `docs/RUNTIME_VALIDATION.md` remain Runtime Pending. The prior
positive QEMU runs predate boot-time GIC selection and the current eight-table final image.
Host-built fixtures cannot prove the content of QEMU's generated DTB, redistributor selection,
EL2/EL1 SRE behavior, preparation completion, delivery-gate semantics, PPI delivery,
acknowledgement/EOI ordering, or interrupt return.
