//! First retained, single-CPU kernel thread runtime and timer-preemption probe.
use super::{
    Aarch64KernelLeafTlb, Aarch64KernelStackMapper, FinalKernelTables, KERNEL_MAP_MEMORY_RANGES,
};
use core::fmt::Write;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use phoenix_kernel::arch::aarch64::{diagnostics as cpu, exception::ExceptionFrame, thread};
use phoenix_kernel::console::Console;
use phoenix_kernel::memory::{FrameAllocator, KernelStackKind};
use phoenix_kernel::platform::aarch64::qemu_virt::{
    EarlyPl011, FinalDirectMapLayout, FinalDirectMapMemory, QemuVirtKernelStackPool,
};
use phoenix_kernel::scheduler::{Scheduler, ThreadId, ThreadState};
use phoenix_kernel::time::{TimerEvent, TimerId};
use phoenix_kernel::timer_service::FiredTimerBatch;

const EVENT: TimerEvent = TimerEvent::new(0x544852);
const QUANTUM_NS: u64 = 2_000_000;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PROGRESS: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static PREEMPTIONS: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static SLEPT: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
struct Runtime {
    policy: Scheduler<3>,
    contexts: [*mut ExceptionFrame; 3],
    ids: [ThreadId; 3],
    timer: Option<TimerId>,
    ticks: usize,
    reschedule: bool,
}
static mut RUNTIME: MaybeUninit<Runtime> = MaybeUninit::uninit();

/// Only reached with IRQs masked after the publication flag became visible.
fn runtime() -> &'static mut Runtime {
    assert!(ACTIVE.load(Ordering::Acquire));
    assert!(phoenix_kernel::arch::aarch64::irqs_masked());
    let slot = &raw mut RUNTIME;
    // SAFETY: Only CPU 0 uses this permanent slot. Every call is in a masked,
    // non-reentrant section; callers end the unique borrow before enabling IRQs,
    // switching stacks, or returning from exception dispatch.
    unsafe { (*slot).assume_init_mut() }
}

pub(super) fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub(super) fn timer_batch(batch: &FiredTimerBatch<1>) {
    let state = runtime();
    assert_eq!(batch.len(), 1);
    let fired = batch.iter().next().unwrap();
    assert_eq!(Some(fired.id()), state.timer);
    assert_eq!(fired.event(), EVENT);
    state.ticks += 1;
    state.reschedule = true;
    if let Some(current) = state.policy.current().filter(|id| id.slot() != 0) {
        PREEMPTIONS[current.slot() - 1].fetch_add(1, Ordering::Relaxed);
    }
    arm(state);
}

fn arm(state: &mut Runtime) {
    let deadline = cpu::frequency()
        .unwrap()
        .deadline_after_nanoseconds(cpu::counter(), QUANTUM_NS)
        .unwrap();
    state.timer = Some(
        super::boot_interrupt_handle()
            .unwrap()
            .schedule(&super::LocalIrq::new(), deadline, EVENT)
            .expect("schedule thread tick"),
    );
}

pub(super) fn dispatch(
    frame: &mut ExceptionFrame,
    slot: usize,
    irq: bool,
) -> Option<*mut ExceptionFrame> {
    if !active() {
        return None;
    }
    let syndrome = frame.syndrome().raw();
    let immediate = syndrome & 0xffff;
    let svc = slot == 4 && syndrome >> 26 == 0x15 && (0x100..=0x102).contains(&immediate);
    if !irq && !svc {
        return None;
    }
    let state = runtime();
    if irq && !state.reschedule {
        return Some(frame);
    }
    state.reschedule = false;
    let old = state.policy.current().expect("current thread");
    state.contexts[old.slot()] = frame;
    let now = cpu::counter();
    if svc {
        match immediate {
            0x101 => {
                assert_ne!(old.slot(), 0, "idle must remain runnable");
                let duration = frame.general(0).unwrap();
                let until = cpu::frequency()
                    .unwrap()
                    .deadline_after_nanoseconds(now, duration)
                    .unwrap();
                state.policy.sleep(until).unwrap();
            }
            0x102 => {
                assert_ne!(old.slot(), 0);
                state.policy.exit().unwrap();
            }
            _ => (),
        }
    }
    let next = state.policy.select(now).expect("retained idle thread");
    super::events::record(5, old.slot() as u64, next.slot() as u64);
    let context = state.contexts[next.slot()];
    assert!(!context.is_null());
    Some(context)
}

extern "C" fn worker(slot: usize) -> ! {
    let index = slot - 1;
    // No yield in this loop: both workers must survive actual timer preemption.
    while PREEMPTIONS[index].load(Ordering::Relaxed) < 2 {
        PROGRESS[index].fetch_add(1, Ordering::Relaxed);
    }
    let before = cpu::counter();
    thread::sleep(6_000_000);
    let elapsed = cpu::counter().duration_since(before).unwrap();
    assert!(
        elapsed
            >= cpu::frequency()
                .unwrap()
                .ticks_for_nanoseconds_ceil(6_000_000)
                .unwrap()
    );
    SLEPT[index].store(true, Ordering::Relaxed);
    thread::exit()
}

pub(super) fn run(
    tables: &'static FinalKernelTables,
    layout: FinalDirectMapLayout<KERNEL_MAP_MEMORY_RANGES>,
    allocator: &mut FrameAllocator<KERNEL_MAP_MEMORY_RANGES>,
    pool: &mut QemuVirtKernelStackPool,
    console: &mut Console<EarlyPl011>,
) {
    let free_before = allocator.total_free_frames();
    // SAFETY: The exact retained hierarchy is active; frames below are allocated
    // exclusively from its proven managed domain and CPU 0 is the only user.
    let mut memory = unsafe { FinalDirectMapMemory::assume_active(tables, layout) };
    let a = pool
        .allocate(allocator, &mut memory, KernelStackKind::Thread)
        .expect("thread A stack");
    let b = pool
        .allocate(allocator, &mut memory, KernelStackKind::Thread)
        .expect("thread B stack");
    let mut tlb = Aarch64KernelLeafTlb;
    // SAFETY: CPU 0 owns these table frames and stacks. The mapper is retained
    // until retirement; no worker or IRQ mutates mappings or this memory owner.
    let mut mapper = unsafe { Aarch64KernelStackMapper::new(tables, &mut memory, &mut tlb) };
    let a = a
        .publish(&mut mapper)
        .unwrap_or_else(|_| panic!("map thread A"));
    let b = b
        .publish(&mut mapper)
        .unwrap_or_else(|_| panic!("map thread B"));
    let mut policy = Scheduler::new();
    let ids = [
        policy.spawn().unwrap(),
        policy.spawn().unwrap(),
        policy.spawn().unwrap(),
    ];
    assert_eq!(policy.select(cpu::counter()), Some(ids[0]));
    // SAFETY: Both private RW/NX stacks are mapped, guarded, aligned and kept
    // owned by this suspended boot frame until both workers have exited.
    let contexts = unsafe {
        [
            core::ptr::null_mut(),
            thread::initialize(a.initial_stack_pointer().as_usize(), worker, 1),
            thread::initialize(b.initial_stack_pointer().as_usize(), worker, 2),
        ]
    };
    let handle = super::boot_interrupt_handle().unwrap();
    let irq = super::LocalIrq::new();
    handle.activate(&irq).expect("thread GIC activation");
    let mut state = Runtime {
        policy,
        contexts,
        ids,
        timer: None,
        ticks: 0,
        reschedule: false,
    };
    arm(&mut state);
    let slot = &raw mut RUNTIME;
    // SAFETY: One-time initialization while IRQs remain masked, preceding the
    // Release flag. The slot never moves and all stack owners remain retained.
    unsafe {
        (*slot).write(state);
    }
    ACTIVE.store(true, Ordering::Release);
    let timeout = cpu::frequency()
        .unwrap()
        .deadline_after_nanoseconds(cpu::counter(), 1_000_000_000)
        .unwrap();
    // SAFETY: Contexts, dispatcher, GIC and the first deadline are published;
    // no lock guard or borrowed runtime state survives this transition.
    unsafe {
        cpu::enable_irqs();
    }
    thread::yield_now();
    loop {
        cpu::disable_irqs();
        let done = {
            let state = runtime();
            state.ids[1..]
                .iter()
                .all(|id| state.policy.state(*id) == Ok(ThreadState::Exited))
        };
        if done {
            break;
        }
        assert!(cpu::counter() < timeout, "thread scheduling timeout");
        // SAFETY: Same live runtime as above; the state borrow has ended.
        unsafe {
            cpu::enable_irqs();
        }
        thread::idle();
    }
    let state = runtime();
    if let Some(id) = state.timer.take() {
        handle.cancel(&irq, id).expect("cancel final tick");
    }
    handle.deactivate(&irq).expect("close thread IRQ gates");
    let ticks = state.ticks;
    for id in state.ids[1..].iter().copied() {
        state.policy.reap(id).unwrap();
    }
    // No further handler can enter; all dead stack frame pointers are revoked
    // before TLB retirement and frame return. The boot context remains current.
    state.contexts[1..].fill(core::ptr::null_mut());
    ACTIVE.store(false, Ordering::Release);
    // Both workers have exited; the current boot stack is separate. Revoke
    // translations through the retained mapper before returning physical frames.
    let a = a
        .unpublish(&mut mapper)
        .unwrap_or_else(|_| panic!("unmap thread A"));
    let b = b
        .unpublish(&mut mapper)
        .unwrap_or_else(|_| panic!("unmap thread B"));
    a.release_unpublished(pool, allocator)
        .unwrap_or_else(|_| panic!("release thread A"));
    b.release_unpublished(pool, allocator)
        .unwrap_or_else(|_| panic!("release thread B"));
    assert_eq!(allocator.total_free_frames(), free_before);
    for index in 0..2 {
        assert!(PROGRESS[index].load(Ordering::Relaxed) > 0);
        assert!(PREEMPTIONS[index].load(Ordering::Relaxed) >= 2);
        assert!(SLEPT[index].load(Ordering::Relaxed));
    }
    let _ = writeln!(
        console,
        "threads: workers=2 preemption=ok sleep=ok exit=ok reclaimed=16 ticks={ticks}"
    );
}
