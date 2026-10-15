//! Fixed-capacity structured diagnostics with explicit loss accounting.
/// One complete structured record, independent of any borrowed runtime object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    /// Monotonic sequence number within this log.
    pub sequence: u64,
    /// Raw architectural counter observation.
    pub ticks: u64,
    /// Stable local event code; see the runtime diagnostics contract.
    pub kind: u16,
    /// Event-specific integer payloads; never borrowed pointers.
    pub values: [u64; 2],
}
/// Overwrite-oldest ring; its owner supplies synchronization.
pub struct EventLog<const N: usize> {
    entries: [Option<Event>; N],
    cursor: usize,
    next: u64,
    lost: u64,
}
impl<const N: usize> Default for EventLog<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> EventLog<N> {
    /// Empty allocation-free log.
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            cursor: 0,
            next: 0,
            lost: 0,
        }
    }
    /// Record or explicitly count an unrecordable/overwritten event.
    pub fn record(&mut self, ticks: u64, kind: u16, values: [u64; 2]) {
        if N == 0 || self.next == u64::MAX {
            self.lost = self.lost.saturating_add(1);
            return;
        }
        if self.entries[self.cursor].is_some() {
            self.lost = self.lost.saturating_add(1);
        }
        self.entries[self.cursor] = Some(Event {
            sequence: self.next,
            ticks,
            kind,
            values,
        });
        self.next += 1;
        self.cursor = (self.cursor + 1) % N;
    }
    /// Retained records in increasing sequence order.
    pub fn entries(&self) -> impl Iterator<Item = Event> + '_ {
        (0..N).filter_map(|offset| self.entries[(self.cursor + offset) % N])
    }
    /// Number of overwritten or rejected events, saturating at u64::MAX.
    pub const fn lost(&self) -> u64 {
        self.lost
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrapping_retains_order_and_reports_loss_without_allocating() {
        let mut log = EventLog::<2>::new();
        for n in 0..5 {
            log.record(n * 10, 1, [n, 0]);
        }
        let mut entries = log.entries();
        assert_eq!(entries.next().unwrap().sequence, 3);
        assert_eq!(entries.next().unwrap().values, [4, 0]);
        assert!(entries.next().is_none());
        assert_eq!(log.lost(), 3);
    }
    #[test]
    fn zero_capacity_and_sequence_exhaustion_are_bounded() {
        let mut zero = EventLog::<0>::new();
        zero.record(0, 0, [0, 0]);
        assert_eq!(zero.entries().count(), 0);
        assert_eq!(zero.lost(), 1);
        let mut log = EventLog::<1>::new();
        log.next = u64::MAX;
        log.record(0, 0, [0, 0]);
        assert_eq!(log.entries().count(), 0);
        assert_eq!(log.lost(), 1);
    }
}
