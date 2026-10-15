# AArch64 Interrupt Runtime Publication

> Construction-stage mechanism record: runtime-status and remaining-work lists below
> describe the earlier integration boundary. Current support, limits and evidence are
> maintained in [0.1-CONTRACT.md](0.1-CONTRACT.md) and
> [RUNTIME_VALIDATION.md](RUNTIME_VALIDATION.md); those records supersede historical
> pending statements here. The ownership design remains applicable unless the
> current contract explicitly refines it.

Phoenix has a Host Tested, allocation-free type state that jointly owns one prepared GICv3
controller and one locked physical-timer service at a pinned address. It resolves the publication
cycle between the two devices: vector lookup is established while GIC delivery is closed, and
timer programming becomes possible only after matching GIC activation succeeds. Both final-map
boot paths now construct and publish the real pinned owner and IRQ lookup, but do not activate GIC
delivery, bind/schedule the installed one-shot consumer, program a deadline, or unmask `DAIF.I`;
target publication and delivery remain Runtime Pending.

## Current implementation

`kernel/src/arch/aarch64/interrupt_runtime.rs` defines:

- `InterruptRuntime<B, CAPACITY, H>`, a non-`Send`, non-`Sync`, non-`Unpin` slot containing its
  lifecycle under an outer `IrqSpinLock`;
- an unsafe one-way `publish` transition from `Unpublished` to `Prepared`, which requires a pinned
  permanent address plus already-installed current-CPU vector lookup and exception-stack state;
- a non-transferable `PublishedInterruptRuntime` handle for phase observation, activation, timer
  scheduling/cancellation, one complete interrupt dispatch cycle, and deactivation;
- an activation transition that verifies the timer service PPI exactly matches the prepared GIC
  PPI, constructs the GIC dispatcher proof only after publication, and creates a private timer
  programming permit only after both Group-1 delivery gates open successfully;
- one dispatch operation that distinguishes special, timer, and unexpected INTIDs, reprograms the
  timer before combined EOI/deactivation, and returns due events only after both locks release; and
- `schedule_one_shot`, which holds the outer IRQ-save lock across timer scheduling and exact
  consumer binding, cancels the new timer on binding failure, and closes the runtime if that
  cancellation cannot safely restore the prior state; and
- a deactivation transition that rejects a nonempty timer queue, explicitly disarms empty timer
  hardware, revokes its programming permit, then closes the CPU and distributor GIC gates.

The stable slot owns every phase in place. An activation or shutdown failure retains the device
owners in an inaccessible terminal state instead of manufacturing a valid prepared owner. An
acknowledgement or EOI failure closes the active state to all later safe operations. The lower GIC
owner now remembers a real acknowledged INTID until EOI succeeds, so dropping a completion token
cannot permit another acknowledge or a shutdown.

`kernel/src/timer_service.rs` separates initially disarmed ownership from programming authority.
Constructing `LockedTimerQueue` no longer assumes that vectors and GIC delivery are already live.
Every command-capable operation requires the private, non-transferable `TimerProgrammingPermit`
held only by the active runtime. `quiesce` rejects pending timers and applies an explicit `Disarm`
transaction before the permit is revoked.

That module also supplies `OneShotTimerConsumer`, an allocation-free IRQ-safe sink for the first
bounded event. It binds one exact `TimerId` generation and `TimerEvent`, accepts only a one-record
matching due batch, and retains the complete fired record. Validation failure or repeated delivery
does not manufacture completion.

On AArch64, `PhysicalTimer` is now deliberately non-`Send` and non-`Sync` and implements
`TimerHardware`. `Arm` programs `CNTP_CVAL_EL0` while signaling is masked, then unmasks the timer;
`Disarm` disables and masks it. The unsafe hardware call remains reachable only through the locked
service and a live runtime permit. Register operations are infallible at the Rust API boundary;
architectural traps remain outside that error model.

`kernel/src/main.rs` now replaces the former prepared-GIC-only static with a one-event boot runtime
and a separate permanent published handle. After final TTBR1 activation and bounded GIC
preparation, the single boot CPU claims `PhysicalTimer`, explicitly disables/masks it, reads the
initial physical counter, builds the locked service, pins the combined runtime in static storage,
publishes its handle, and installs a permanent initially unbound one-shot consumer. A
Release/Acquire atomic flag prevents the common exception dispatcher from reading the handle before
initialization. `PHOENIX_GIC_PREPARED` followed by
`PHOENIX_IRQ_RUNTIME_PUBLISHED` is emitted only after this closed-delivery publication completes.

Every IRQ vector already reaches `phoenix_exception_dispatch`. In a final-map build it now checks
the publication flag and delegates IRQ slots to `dispatch_one`; synchronous syscall and user-fault
ordering is unchanged. A special INTID or empty timer batch may return. An unexpected completed
INTID, a timer batch rejected by the exact one-shot consumer, an unpublished lookup, or any runtime
failure emits the normal `PHOENIX_EXCEPTION` terminal path with IRQ-specific detail. The consumer
is invoked only after `dispatch_one` has completed EOI and released both device locks. This policy
is dormant because boot leaves the consumer unbound and never activates delivery or unmasks IRQs.

## Invariants and failure semantics

- The runtime address is pinned before publication and remains valid through its final active
  interval. The unsafe publisher must make every current-CPU EL1 IRQ vector able to recover the
  same slot before activation.
- Static runtime initialization precedes handle initialization, which precedes the Release
  publication flag. The dispatcher performs an Acquire load before borrowing the handle. Both GIC
  gates and `DAIF.I` remain closed throughout the sequence.
- Publication, activation, dispatch, scheduling, cancellation, and deactivation are single-CPU
  operations. Their handles and the concrete GIC/timer backends cannot move to another CPU.
- The outer runtime lock is always acquired before the inner timer lock. Neither path acquires them
  in reverse order, sleeps, allocates, context-switches, or invokes a callback while guarded.
- One-shot binding follows `runtime -> timer` scheduling and then `runtime -> consumer`, with the
  timer guard released before the consumer guard is acquired. On binding failure, the consumer
  guard is released before cancellation reacquires the timer lock. No path holds both inner locks.
- Post-EOI consumption acquires only the consumer lock after `dispatch_one` has released both
  device locks. No consumer-held path acquires the runtime or timer lock.
- The outer lock masks local IRQ delivery before the distributor gate opens and retains masking
  until the complete transition is published in the slot. `activate` and `deactivate` preserve the
  caller's exact preceding mask state; they do not independently enable `DAIF.I`.
- The timer PPI identity and trigger must exactly match the prepared GIC configuration. A mismatch
  leaves the slot prepared and touches neither controller.
- No safe timer programming method is reachable before successful activation or after permit
  revocation. The timer is rearmed or disarmed before EOI, preventing a level-triggered source from
  being completed while stale comparator state remains.
- Special INTIDs 1020 through 1023 require no EOI. Every other INTID creates one remembered
  completion obligation. A dropped or twice-failed token leaves the GIC owner unusable for a later
  acknowledge or shutdown.
- An unexpected real INTID is completed before it is returned to policy code. The current runtime
  reports it without deciding whether the kernel should continue; the current boot wrapper then
  chooses the terminal `PHOENIX_EXCEPTION` path.
- A recoverable timer programming failure rolls the queue back and reapplies prior hardware state.
  Failed recovery, GIC acknowledge failure, or EOI failure makes the runtime unavailable.
- A one-shot binding failure is recoverable only if cancellation removes the newly scheduled ID
  and restores hardware state. Any cancellation failure makes the runtime unavailable even when
  the timer service itself recovered, because safe code can no longer prove that every armed event
  has its matching consumer.
- Deactivation never abandons queued events. `PendingTimers` leaves delivery active; callers must
  cancel or dispatch every owned timer before retrying.
- Due batches contain data only. Callbacks run after the runtime and timer guards have released and
  after successful GIC completion.
- A one-shot completion requires exactly one due record matching both the bound timer generation
  and event value. Invalid cardinality, stale IDs, wrong events, and repeated delivery preserve the
  prior state and return explicit errors.

## Validation

Host tests cover one-way publication, PPI mismatch without hardware mutation, distributor-before-
CPU activation, timer reprogramming before EOI, timer-disarm-before-CPU-before-distributor
shutdown, exact prior IRQ-mask restoration, pending-timer shutdown rejection and retry, terminal
activation failure, and terminal EOI failure. The GIC tests separately verify that abandoning an
acknowledgement blocks both a second acknowledge and shutdown without closing active gates.
One-shot consumer tests cover one-way binding, exact retained completion, empty/multiple batches,
stale IDs, wrong events, and repeated consumption. Runtime tests additionally cover atomic
schedule/bind success, cancellation after rejected binding, terminal cancellation/recovery failure,
and consumer acquisition strictly after EOI.

Canonical checks cross-compile `PhysicalTimer`'s `TimerHardware` adapter, the permit-gated service,
and the runtime state machine for every AArch64 kernel variant. The final-map and loaded-init
variants additionally Cross Compile the static owner, physical counter/timer initialization,
Release/Acquire handle publication, installed consumer, and common IRQ dispatcher branch; static
ELF inspection keeps requiring both publication markers. These results are Host Tested, Cross
Compiled, and ELF Inspected only; no new QEMU command was run.

## TODO

- Add an explicit final transition that unmasks `DAIF.I` only after runtime publication and
  activation, and a symmetric removal protocol that prevents new handler entry.
- Invoke the bounded schedule/bind transaction from boot after activation while `DAIF.I` remains
  masked, before any IRQ can be recognized.
- Replace the current terminal diagnostic policy with the scheduler-era policy for an unexpected
  completed INTID or acknowledge/EOI failure.
- Connect returned timer events to sleeping kernel threads and the tickless scheduler.
- Add a bounded target probe for publication, one timer event, EOI, interrupt return, lock
  exclusion, and continued execution.

## Deliberately skipped

This checkpoint does not bind the boot consumer, activate the retained boot GIC, issue `Arm`,
program a target comparator, unmask `DAIF.I`, observe or return through a real IRQ, or run QEMU. It
does not add nested
interrupts, FIQ/NMI handling, SMP/per-CPU slots, scheduler callbacks, dynamic timer capacity,
allocation in interrupt context, or a general interrupt-controller abstraction. The final-map and
loaded-init paths retain the combined runtime in its published `Prepared` phase with delivery
closed and the physical timer disabled/masked.

## Runtime Pending

`RUN-SYNC-001`, `RUN-IRQ-001`, and `RUN-TIME-001` in `docs/RUNTIME_VALIDATION.md` remain Runtime
Pending. Host mocks and cross-compilation cannot prove that the pinned board accepts the register
sequence, that a physical timer PPI reaches the published slot, that `DAIF.I` restoration excludes
same-CPU re-entry, that EOI permits later delivery, or that target execution returns and continues.
