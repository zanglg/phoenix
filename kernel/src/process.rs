//! Architecture-neutral process identity, lifecycle, and bounded registry.

use core::num::NonZeroU64;

/// Failure while constructing a process identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessIdError {
    /// Generation zero is reserved so an all-zero value is never valid.
    ZeroGeneration,
}

/// Stable identity of one incarnation of a process-table slot.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProcessId {
    slot: usize,
    generation: NonZeroU64,
}

impl ProcessId {
    /// Combine a zero-based table slot and a nonzero generation.
    pub const fn new(slot: usize, generation: u64) -> Result<Self, ProcessIdError> {
        let Some(generation) = NonZeroU64::new(generation) else {
            return Err(ProcessIdError::ZeroGeneration);
        };
        Ok(Self { slot, generation })
    }

    /// Return the zero-based process-table slot.
    pub const fn slot(self) -> usize {
        self.slot
    }

    /// Return the nonzero slot generation.
    pub const fn generation(self) -> u64 {
        self.generation.get()
    }
}

/// Opaque status supplied when a process exits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitStatus(u64);

impl ExitStatus {
    /// Retain one native ABI exit status without imposing Unix truncation.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the native ABI value supplied by the process.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Lifecycle phase without phase-specific data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessPhase {
    /// Resources are being constructed and cannot be selected to run.
    Created,
    /// Construction is complete and the process may be selected.
    Ready,
    /// The process currently owns an execution context.
    Running,
    /// Execution ended and resources await an explicit reap.
    Exited,
}

/// Complete lifecycle state of one process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    /// Resources are being constructed and cannot be selected to run.
    Created,
    /// Construction is complete and the process may be selected.
    Ready,
    /// The process currently owns an execution context.
    Running,
    /// Execution ended with the retained native status.
    Exited(ExitStatus),
}

impl ProcessState {
    /// Return this state's data-free phase.
    pub const fn phase(self) -> ProcessPhase {
        match self {
            Self::Created => ProcessPhase::Created,
            Self::Ready => ProcessPhase::Ready,
            Self::Running => ProcessPhase::Running,
            Self::Exited(_) => ProcessPhase::Exited,
        }
    }

    /// Return an exit status only after the process has exited.
    pub const fn exit_status(self) -> Option<ExitStatus> {
        match self {
            Self::Exited(status) => Some(status),
            Self::Created | Self::Ready | Self::Running => None,
        }
    }
}

/// Rejected process-state transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessTransitionError {
    process_id: ProcessId,
    from: ProcessPhase,
    to: ProcessPhase,
}

impl ProcessTransitionError {
    /// Return the process whose transition was rejected.
    pub const fn process_id(self) -> ProcessId {
        self.process_id
    }

    /// Return the phase observed before the rejected transition.
    pub const fn from(self) -> ProcessPhase {
        self.from
    }

    /// Return the requested phase.
    pub const fn to(self) -> ProcessPhase {
        self.to
    }
}

/// Identity and lifecycle state independent of owned process resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessControl {
    id: ProcessId,
    state: ProcessState,
}

impl ProcessControl {
    /// Create a process that is not yet runnable.
    pub const fn new(id: ProcessId) -> Self {
        Self {
            id,
            state: ProcessState::Created,
        }
    }

    /// Return this process incarnation's identifier.
    pub const fn id(self) -> ProcessId {
        self.id
    }

    /// Return the current lifecycle state.
    pub const fn state(self) -> ProcessState {
        self.state
    }

    /// Publish a completely constructed process as runnable.
    pub fn make_ready(&mut self) -> Result<(), ProcessTransitionError> {
        self.transition(ProcessPhase::Created, ProcessState::Ready)
    }

    /// Select one ready process as the active execution context.
    pub fn start(&mut self) -> Result<(), ProcessTransitionError> {
        self.transition(ProcessPhase::Ready, ProcessState::Running)
    }

    /// Record terminal status for the active process.
    pub fn exit(&mut self, status: ExitStatus) -> Result<(), ProcessTransitionError> {
        self.transition(ProcessPhase::Running, ProcessState::Exited(status))
    }

    fn transition(
        &mut self,
        expected: ProcessPhase,
        next: ProcessState,
    ) -> Result<(), ProcessTransitionError> {
        let from = self.state.phase();
        if from != expected {
            return Err(ProcessTransitionError {
                process_id: self.id,
                from,
                to: next.phase(),
            });
        }
        self.state = next;
        Ok(())
    }
}

/// One lifecycle-controlled process and its owned resource aggregate.
#[derive(Debug, Eq, PartialEq)]
pub struct Process<T> {
    control: ProcessControl,
    resources: T,
}

impl<T> Process<T> {
    /// Return this process incarnation's identifier.
    pub const fn id(&self) -> ProcessId {
        self.control.id()
    }

    /// Return the current lifecycle state.
    pub const fn state(&self) -> ProcessState {
        self.control.state()
    }

    /// Borrow the process-owned resources immutably.
    pub const fn resources(&self) -> &T {
        &self.resources
    }

    /// Borrow the process-owned resources mutably without changing lifecycle state.
    pub fn resources_mut(&mut self) -> &mut T {
        &mut self.resources
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ProcessSlot<T> {
    Vacant { generation: NonZeroU64 },
    Occupied(Process<T>),
    Retired,
}

/// Error while looking up a process identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessLookupError {
    /// The identifier names a slot outside this table.
    SlotOutOfRange,
    /// The slot has no current process at the requested generation.
    Vacant,
    /// The slot now belongs to another generation or is permanently retired.
    StaleIdentifier,
}

/// Failure to reap one process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessReapError {
    /// Identifier lookup failed.
    Lookup(ProcessLookupError),
    /// Only an exited process can release its resource aggregate.
    NotExited,
}

/// Full-table error that returns ownership of the uninserted resources.
#[derive(Debug, Eq, PartialEq)]
pub struct ProcessTableFull<T> {
    resources: T,
}

impl<T> ProcessTableFull<T> {
    /// Recover resources that could not be inserted.
    pub fn into_resources(self) -> T {
        self.resources
    }
}

/// Resources and exit information returned by a successful reap.
#[derive(Debug, Eq, PartialEq)]
pub struct ReapedProcess<T> {
    id: ProcessId,
    status: ExitStatus,
    resources: T,
}

impl<T> ReapedProcess<T> {
    /// Return the exact process incarnation that was removed.
    pub const fn id(&self) -> ProcessId {
        self.id
    }

    /// Return its retained exit status.
    pub const fn status(&self) -> ExitStatus {
        self.status
    }

    /// Recover the process-owned resource aggregate.
    pub fn into_resources(self) -> T {
        self.resources
    }
}

/// Fixed-capacity process registry with generation-checked slot reuse.
#[derive(Debug, Eq, PartialEq)]
pub struct ProcessTable<T, const CAPACITY: usize> {
    slots: [ProcessSlot<T>; CAPACITY],
}

impl<T, const CAPACITY: usize> ProcessTable<T, CAPACITY> {
    /// Create an empty table whose first incarnation uses generation one.
    pub fn new() -> Self {
        Self {
            slots: [const {
                ProcessSlot::Vacant {
                    generation: NonZeroU64::MIN,
                }
            }; CAPACITY],
        }
    }

    /// Insert resources into the lowest reusable slot as a created process.
    pub fn insert(&mut self, resources: T) -> Result<ProcessId, ProcessTableFull<T>> {
        let Some((slot_index, slot)) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| matches!(slot, ProcessSlot::Vacant { .. }))
        else {
            return Err(ProcessTableFull { resources });
        };
        let generation = match slot {
            ProcessSlot::Vacant { generation } => *generation,
            ProcessSlot::Occupied(_) | ProcessSlot::Retired => {
                unreachable!("vacant slot selected above")
            }
        };
        let id = ProcessId {
            slot: slot_index,
            generation,
        };
        *slot = ProcessSlot::Occupied(Process {
            control: ProcessControl::new(id),
            resources,
        });
        Ok(id)
    }

    /// Borrow the current process for an exact generation-checked identifier.
    pub fn get(&self, id: ProcessId) -> Result<&Process<T>, ProcessLookupError> {
        match self.slots.get(id.slot()) {
            None => Err(ProcessLookupError::SlotOutOfRange),
            Some(ProcessSlot::Vacant { generation }) if *generation == id.generation => {
                Err(ProcessLookupError::Vacant)
            }
            Some(ProcessSlot::Occupied(process)) if process.id() == id => Ok(process),
            Some(ProcessSlot::Vacant { .. } | ProcessSlot::Occupied(_) | ProcessSlot::Retired) => {
                Err(ProcessLookupError::StaleIdentifier)
            }
        }
    }

    /// Mutably borrow resources for an exact identifier without changing state.
    pub fn resources_mut(&mut self, id: ProcessId) -> Result<&mut T, ProcessLookupError> {
        self.get_mut(id).map(|process| &mut process.resources)
    }

    /// Transition a created process to ready.
    pub fn make_ready(&mut self, id: ProcessId) -> Result<(), ProcessTableStateError> {
        self.get_mut(id)
            .map_err(ProcessTableStateError::Lookup)?
            .control
            .make_ready()
            .map_err(ProcessTableStateError::Transition)
    }

    /// Transition a ready process to running.
    pub fn start(&mut self, id: ProcessId) -> Result<(), ProcessTableStateError> {
        self.get_mut(id)
            .map_err(ProcessTableStateError::Lookup)?
            .control
            .start()
            .map_err(ProcessTableStateError::Transition)
    }

    /// Transition a running process to exited while retaining all resources.
    pub fn exit(
        &mut self,
        id: ProcessId,
        status: ExitStatus,
    ) -> Result<(), ProcessTableStateError> {
        self.get_mut(id)
            .map_err(ProcessTableStateError::Lookup)?
            .control
            .exit(status)
            .map_err(ProcessTableStateError::Transition)
    }

    /// Remove one exited process and return its resources to the caller.
    pub fn reap(&mut self, id: ProcessId) -> Result<ReapedProcess<T>, ProcessReapError> {
        let status = self
            .get(id)
            .map_err(ProcessReapError::Lookup)?
            .state()
            .exit_status()
            .ok_or(ProcessReapError::NotExited)?;
        let replacement = match id.generation().checked_add(1).and_then(NonZeroU64::new) {
            Some(generation) => ProcessSlot::Vacant { generation },
            None => ProcessSlot::Retired,
        };
        let removed = core::mem::replace(&mut self.slots[id.slot()], replacement);
        let ProcessSlot::Occupied(process) = removed else {
            unreachable!("successful lookup proves an occupied matching slot")
        };
        Ok(ReapedProcess {
            id,
            status,
            resources: process.resources,
        })
    }

    /// Return the number of currently occupied process slots.
    pub fn len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| matches!(slot, ProcessSlot::Occupied(_)))
            .count()
    }

    /// Return whether no process currently occupies a slot.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get_mut(&mut self, id: ProcessId) -> Result<&mut Process<T>, ProcessLookupError> {
        match self.slots.get_mut(id.slot()) {
            None => Err(ProcessLookupError::SlotOutOfRange),
            Some(ProcessSlot::Vacant { generation }) if *generation == id.generation => {
                Err(ProcessLookupError::Vacant)
            }
            Some(ProcessSlot::Occupied(process)) if process.id() == id => Ok(process),
            Some(ProcessSlot::Vacant { .. } | ProcessSlot::Occupied(_) | ProcessSlot::Retired) => {
                Err(ProcessLookupError::StaleIdentifier)
            }
        }
    }
}

impl<T, const CAPACITY: usize> Default for ProcessTable<T, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Lookup or transition failure from a table-owned process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessTableStateError {
    /// Identifier lookup failed before any state changed.
    Lookup(ProcessLookupError),
    /// The identifier was current but the requested transition was invalid.
    Transition(ProcessTransitionError),
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;

    use super::{
        ExitStatus, ProcessControl, ProcessId, ProcessIdError, ProcessLookupError, ProcessPhase,
        ProcessReapError, ProcessSlot, ProcessState, ProcessTable, ProcessTableStateError,
    };

    #[test]
    fn lifecycle_accepts_only_created_ready_running_exited() {
        assert_eq!(ProcessId::new(0, 0), Err(ProcessIdError::ZeroGeneration));
        let id = ProcessId::new(7, 3).unwrap();
        let mut control = ProcessControl::new(id);
        assert_eq!(control.state(), ProcessState::Created);
        assert_eq!(control.start().unwrap_err().from(), ProcessPhase::Created);
        assert_eq!(control.state(), ProcessState::Created);

        control.make_ready().unwrap();
        control.start().unwrap();
        control.exit(ExitStatus::new(42)).unwrap();
        assert_eq!(control.state(), ProcessState::Exited(ExitStatus::new(42)));
        let error = control.exit(ExitStatus::new(1)).unwrap_err();
        assert_eq!(error.process_id(), id);
        assert_eq!(error.from(), ProcessPhase::Exited);
        assert_eq!(error.to(), ProcessPhase::Exited);
    }

    #[test]
    fn table_preserves_resources_and_rejects_invalid_identifiers() {
        let mut table = ProcessTable::<u32, 2>::new();
        let first = table.insert(10).unwrap();
        let second = table.insert(20).unwrap();
        assert_eq!(first, ProcessId::new(0, 1).unwrap());
        assert_eq!(second, ProcessId::new(1, 1).unwrap());
        assert_eq!(table.len(), 2);
        assert_eq!(table.insert(30).unwrap_err().into_resources(), 30);
        assert_eq!(table.get(first).unwrap().resources(), &10);
        *table.resources_mut(first).unwrap() = 11;
        assert_eq!(table.get(first).unwrap().resources(), &11);
        assert_eq!(
            table.get(ProcessId::new(2, 1).unwrap()),
            Err(ProcessLookupError::SlotOutOfRange)
        );
        assert_eq!(
            table.get(ProcessId::new(0, 2).unwrap()),
            Err(ProcessLookupError::StaleIdentifier)
        );
    }

    #[test]
    fn reaping_requires_exit_and_invalidates_the_old_generation() {
        let mut table = ProcessTable::<u32, 1>::new();
        let first = table.insert(55).unwrap();
        assert_eq!(table.reap(first), Err(ProcessReapError::NotExited));
        assert_eq!(
            table.start(first),
            Err(ProcessTableStateError::Transition(
                super::ProcessTransitionError {
                    process_id: first,
                    from: ProcessPhase::Created,
                    to: ProcessPhase::Running,
                }
            ))
        );
        table.make_ready(first).unwrap();
        table.start(first).unwrap();
        table.exit(first, ExitStatus::new(9)).unwrap();
        let reaped = table.reap(first).unwrap();
        assert_eq!(reaped.id(), first);
        assert_eq!(reaped.status().raw(), 9);
        assert_eq!(reaped.into_resources(), 55);
        assert!(table.is_empty());
        assert_eq!(table.get(first), Err(ProcessLookupError::StaleIdentifier));

        let second = table.insert(66).unwrap();
        assert_eq!(second, ProcessId::new(0, 2).unwrap());
        assert_eq!(
            table.get(ProcessId::new(0, 3).unwrap()),
            Err(ProcessLookupError::StaleIdentifier)
        );
    }

    #[test]
    fn maximum_generation_retires_instead_of_wrapping() {
        let mut table = ProcessTable::<u8, 1> {
            slots: [ProcessSlot::Vacant {
                generation: NonZeroU64::MAX,
            }],
        };
        let id = table.insert(7).unwrap();
        assert_eq!(id.generation(), u64::MAX);
        table.make_ready(id).unwrap();
        table.start(id).unwrap();
        table.exit(id, ExitStatus::new(0)).unwrap();
        assert_eq!(table.reap(id).unwrap().into_resources(), 7);
        assert_eq!(table.insert(8).unwrap_err().into_resources(), 8);
        assert_eq!(table.get(id), Err(ProcessLookupError::StaleIdentifier));
    }
}
