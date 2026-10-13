//! Opt-in QEMU conformance probes. The default kernel never enters semihosting.
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};
use phoenix_kernel::arch::aarch64::{diagnostics as cpu, exception::ExceptionFrame};
use phoenix_kernel::console::Console;
use phoenix_kernel::memory::PhysAddr;
use phoenix_kernel::platform::aarch64::qemu_virt::{BootstrapPhysicalMemory, EarlyPl011};
use phoenix_kernel::sync::IrqSpinLock;
use phoenix_kernel::time::TimerEvent;
use phoenix_kernel::timer_service::OneShotTimerConsumerState;

static PROBE: AtomicUsize = AtomicUsize::new(0);
static EXPECTED_FAULT: AtomicUsize = AtomicUsize::new(0);
static FAULTS: AtomicUsize = AtomicUsize::new(0);
static mut ZERO_BSS: [u64; 16] = [0; 16];
static IRQ_LOCK: IrqSpinLock<usize> = IrqSpinLock::new(0);
const NAMES: &[&str] = &[
    "state",
    "panic",
    "registers",
    "guard",
    "readonly",
    "nx",
    "timer",
    "threads",
    "heap-low",
    "heap-high",
    "stack-guards",
    "fatal",
];

pub(super) fn select(argument: usize) {
    // SAFETY: Called once before the temporary high RAM mapping is retired.
    let memory = unsafe { BootstrapPhysicalMemory::assume_bootstrap_mapping() };
    let tree = memory
        .device_tree(PhysAddr::new(argument))
        .expect("probe DTB");
    let info = tree.boot_info().expect("probe boot information");
    if let Some(name) = info
        .bootargs()
        .and_then(|args| args.strip_prefix("phoenix.probe="))
    {
        let id = NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .expect("unknown probe")
            + 1;
        PROBE.store(id, Ordering::Release);
        let mut console = Console::new(EarlyPl011::new());
        let _ = writeln!(console, "PHOENIX_TEST_BEGIN {name}");
        if name == "panic" {
            panic!("deliberate runtime probe");
        }
    }
}

pub(super) fn panic_exit() {
    if PROBE.load(Ordering::Acquire) != 0 {
        let deliberate = PROBE.load(Ordering::Relaxed) == 2;
        if deliberate {
            success();
        }
        // SAFETY: Probe selection requires the opt-in semihosting test harness.
        unsafe {
            cpu::test_exit(1);
        }
    }
}

fn success() -> ! {
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "PHOENIX_TEST_OK");
    console.sink_mut().flush();
    // SAFETY: This function is reachable only in an explicitly selected test.
    unsafe {
        cpu::test_exit(0);
    }
}

pub(super) fn exception(frame: &mut ExceptionFrame, slot: usize) -> bool {
    let probe = PROBE.load(Ordering::Acquire);
    if probe == 3 && frame.program_counter() == (&raw const cpu::phoenix_register_probe_trap) as u64
    {
        assert_eq!(slot, 4);
        assert_eq!(frame.syndrome().raw(), 0xf2000123);
        assert_eq!(frame.saved_program_status(), 0xa00003c5);
        assert_eq!(frame.sp_el0(), 0x1230);
        for index in 0..31 {
            assert_eq!(frame.general(index), Some(0x100 + index as u64));
        }
        frame.set_program_counter(frame.program_counter() + 4);
        FAULTS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    if ((4..=6).contains(&probe) || (9..=11).contains(&probe))
        && EXPECTED_FAULT.load(Ordering::Acquire) != 0
    {
        assert_eq!(slot, 4);
        let esr = frame.syndrome().raw();
        assert_eq!(esr >> 26, if probe == 6 { 0x21 } else { 0x25 });
        assert_eq!(esr & 0x3c, if probe == 4 || probe >= 9 { 0x4 } else { 0xc });
        assert_eq!(
            frame.fault_address(),
            EXPECTED_FAULT.load(Ordering::Relaxed) as u64
        );
        frame.set_program_counter((&raw const cpu::phoenix_fault_probe_resume) as u64);
        EXPECTED_FAULT.store(0, Ordering::Release);
        FAULTS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

pub(super) fn after_map(console: &mut Console<EarlyPl011>) {
    let probe = PROBE.load(Ordering::Acquire);
    if probe == 0 {
        return;
    }
    match probe {
        1 => {
            unsafe extern "C" {
                static __boot_stack_bottom: u8;
                static __boot_stack_top: u8;
                static __exception_vectors: u8;
            }
            let (el, sp, vectors) = cpu::boot_state();
            assert_eq!(el, 1);
            assert_eq!(sp & 15, 0);
            assert!(
                ((&raw const __boot_stack_bottom) as usize..(&raw const __boot_stack_top) as usize)
                    .contains(&sp)
            );
            assert_eq!(vectors, (&raw const __exception_vectors) as usize);
            for index in 0..16 {
                // SAFETY: Immutable BSS witness lies in retained readable kernel data.
                assert_eq!(
                    unsafe {
                        core::ptr::read_volatile((&raw const ZERO_BSS).cast::<u64>().add(index))
                    },
                    0
                );
            }
            let _ = writeln!(
                console,
                "state: EL={el} SP={sp:#x} VBAR={vectors:#x} BSS=zero"
            );
        }
        3 => {
            // SAFETY: Exact BRK handler above validates every saved GPR and
            // control field; the assembly then checks all restored GPRs.
            assert_eq!(unsafe { cpu::phoenix_register_probe() }, 0);
            assert_eq!(FAULTS.load(Ordering::Relaxed), 1);
            let _ = writeln!(console, "registers: saved=31 restored=31 eret=ok");
        }
        4..=6 | 9..=10 => {
            unsafe extern "C" {
                static __boot_stack_guard_start: u8;
                static __text_start: u8;
            }
            let scratch = 0u64;
            let address = match probe {
                4 => (&raw const __boot_stack_guard_start) as usize,
                5 => (&raw const __text_start) as usize,
                9 => phoenix_kernel::platform::aarch64::qemu_virt::KERNEL_HEAP_GUARD,
                10 => phoenix_kernel::platform::aarch64::qemu_virt::KERNEL_HEAP_START + 4 * 4096,
                _ => (&raw const scratch) as usize,
            };
            EXPECTED_FAULT.store(address, Ordering::Release);
            // SAFETY: An exact current-EL fault handler is armed, the live stack
            // is separate from the guard, and no write can reach protected text.
            unsafe {
                cpu::phoenix_fault_probe(address, if probe >= 9 { 0 } else { probe - 4 });
            }
            assert_eq!(
                EXPECTED_FAULT.load(Ordering::Acquire),
                0,
                "access unexpectedly succeeded"
            );
            assert_eq!(FAULTS.load(Ordering::Relaxed), 1);
            let _ = writeln!(
                console,
                "fault: {} address={address:#x} recovered=1",
                NAMES[probe - 1]
            );
        }
        7 => timer(console),
        8 => (),
        11 => assert_eq!(FAULTS.load(Ordering::Relaxed), 2),
        12 => {
            // SAFETY: This opt-in harness deliberately enters the fatal report;
            // only its exact trap can be converted into a successful test exit.
            unsafe { cpu::phoenix_fatal_probe() };
            panic!("fatal probe returned");
        }
        _ => unreachable!(),
    }
    success()
}

fn timer(console: &mut Console<EarlyPl011>) {
    let irq = phoenix_kernel::arch::aarch64::LocalIrq::new();
    assert!(phoenix_kernel::arch::aarch64::irqs_masked());
    let handle = super::boot_interrupt_handle().expect("published IRQ runtime");
    handle.activate(&irq).expect("activate GIC");
    let frequency = cpu::frequency().expect("counter frequency");
    let start = cpu::counter();
    let deadline = frequency
        .deadline_after_nanoseconds(start, 20_000_000)
        .expect("deadline");
    let timeout = frequency
        .deadline_after_nanoseconds(start, 1_000_000_000)
        .expect("timeout");
    let event = TimerEvent::new(0x5048);
    // SAFETY: Published runtime owns activated GIC and bound consumer; no lock
    // or mask token is outstanding, and the exception stack is mapped.
    unsafe {
        cpu::enable_irqs();
    }
    // A separate probe lock is acquired before the deadline. Its guard must
    // prevent handler entry while the timer becomes pending.
    let id = {
        let mut guard = IRQ_LOCK.lock(&irq);
        let id = handle
            .schedule_one_shot(&irq, &super::BOOT_TIMER_CONSUMER, deadline, event)
            .expect("bind timer");
        let mut previous = cpu::counter();
        while previous < deadline {
            let next = cpu::counter();
            assert!(next >= previous, "counter regressed");
            previous = next;
        }
        assert!(matches!(
            super::BOOT_TIMER_CONSUMER.state(&irq),
            OneShotTimerConsumerState::Bound { .. }
        ));
        *guard = 1;
        id
    };
    super::events::record(6, deadline.raw(), event.raw());
    loop {
        if let OneShotTimerConsumerState::Fired { timer } = super::BOOT_TIMER_CONSUMER.state(&irq) {
            assert_eq!(timer.id(), id);
            assert_eq!(timer.event(), event);
            assert_eq!(timer.deadline(), deadline);
            break;
        }
        assert!(cpu::counter() < timeout, "timer IRQ timeout");
        core::hint::spin_loop();
    }
    // Handler completed and returned; first close local entry, then disarm and
    // close both controller gates. Permanent lookup remains valid but dormant.
    cpu::disable_irqs();
    handle.deactivate(&irq).expect("deactivate GIC");
    assert_eq!(handle.phase(&irq), super::InterruptRuntimePhase::Prepared);
    assert_eq!(
        *IRQ_LOCK.lock(&irq),
        2,
        "IRQ did not acquire the released lock"
    );
    let _ = writeln!(
        console,
        "timer: intid=30 event={} irq=1 eoi=ok return=ok lock=ok gates=closed",
        event.raw()
    );
}

pub(super) fn irq_completed() {
    if PROBE.load(Ordering::Acquire) == 7 {
        let irq = phoenix_kernel::arch::aarch64::LocalIrq::new();
        let mut guard = IRQ_LOCK.lock(&irq);
        assert_eq!(*guard, 1);
        *guard = 2;
    }
}

pub(super) fn threads_selected() -> bool {
    PROBE.load(Ordering::Acquire) == 8
}

pub(super) fn stack_guards(lower: usize, upper: usize) {
    if PROBE.load(Ordering::Acquire) != 11 {
        return;
    }
    for address in [lower, upper] {
        EXPECTED_FAULT.store(address, Ordering::Release);
        // SAFETY: Called with the stack's usable leaves still published. The
        // active boot stack is separate and the exact recovery handler is armed.
        unsafe { cpu::phoenix_fault_probe(address, 0) };
        assert_eq!(EXPECTED_FAULT.load(Ordering::Acquire), 0);
    }
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "stack: guards=2 recovered=2 while-mapped=yes");
}

pub(super) fn fatal_exit(frame: &ExceptionFrame, slot: usize) {
    if PROBE.load(Ordering::Acquire) == 12 {
        assert_eq!(slot, 4);
        assert_eq!(
            frame.program_counter(),
            (&raw const cpu::phoenix_fatal_probe_trap) as u64
        );
        assert_eq!(frame.syndrome().raw(), 0xf2000456);
        success();
    }
}
