//! CPU-local runtime logging. Never acquire device locks while holding LOG.
use super::*;
use phoenix_kernel::arch::aarch64::{LocalIrq, diagnostics};
use phoenix_kernel::{event_log::EventLog, sync::IrqSpinLock};
static LOG: IrqSpinLock<EventLog<32>> = IrqSpinLock::new(EventLog::new());

pub(super) fn record(kind: u16, first: u64, second: u64) {
    LOG.lock(&LocalIrq::new())
        .record(diagnostics::counter().raw(), kind, [first, second]);
}

pub(super) fn dump(console: &mut Console<EarlyPl011>) {
    // Copy under a nonblocking lock, then release it before serial MMIO. Panic
    // during a log mutation can report unavailability without recursive deadlock.
    let snapshot = {
        let irq = LocalIrq::new();
        let Some(log) = LOG.try_lock(&irq) else {
            let _ = writeln!(console, "events: unavailable");
            return;
        };
        let mut records = [None; 32];
        for (index, entry) in log.entries().enumerate() {
            records[index] = Some(entry);
        }
        (records, log.lost())
    };
    for event in snapshot.0.into_iter().flatten() {
        let _ = writeln!(
            console,
            "event: seq={} ticks={} kind={} a={:#x} b={:#x}",
            event.sequence, event.ticks, event.kind, event.values[0], event.values[1]
        );
    }
    let _ = writeln!(console, "events: lost={}", snapshot.1);
}
