//! Two-process native system with EL0 timer preemption and bounded round robin.
//!
//! All mutable state belongs to CPU 0 with DAIF.I masked. EL0 never borrows
//! kernel objects. Root switches invalidate translations before child reaping;
//! no resource or frame reference escapes one synchronous dispatch operation.
//! EL0 alone may run with IRQ enabled. Every vector masks IRQs before Rust;
//! timer callbacks complete EOI/unlock before selecting a different user frame.
use super::*;
use core::sync::atomic::AtomicUsize;
use phoenix_kernel::arch::aarch64::diagnostics as cpu;
use phoenix_kernel::process::{ProcessTable, ProcessTermination};
use phoenix_kernel::scheduler::{Scheduler, ThreadId, ThreadState};
use phoenix_kernel::vfs::{FileTable, RamFs};
mod hardening;

struct Resources {
    space: InitAddressSpace,
    asid: AddressSpaceId,
    files: FileTable<'static, 8>,
    suspended: Option<ExceptionFrame>,
    thread: Option<ThreadId>,
}
struct State {
    processes: ProcessTable<Resources, 2>,
    current: ProcessId,
    root: ProcessId,
    child: Option<ProcessId>,
    queue: Scheduler<2>,
    scheduled: [Option<ProcessId>; 2],
    preempt: bool,
    timer: Option<phoenix_kernel::time::TimerId>,
    ticks: [usize; 2],
    timer_switches: usize,
    idle: bool,
    idle_top: Option<usize>,
    idle_ticks: usize,
    child_parent_phases: [usize; 3],
    allocator: FrameAllocator<INIT_MEMORY_RANGES>,
    asids: AsidAllocator,
    tables: &'static FinalKernelTables,
    layout: FinalDirectMapLayout<INIT_MEMORY_RANGES>,
    archive: Initramfs<'static>,
    ram: RamFs<8, 1024>,
    children: usize,
    faults: usize,
    test: bool,
    allocator_without_child: FrameAllocator<INIT_MEMORY_RANGES>,
    asids_without_child: u16,
}
static mut STATE: MaybeUninit<State> = MaybeUninit::uninit();
static mut TABLES: MaybeUninit<FinalKernelTables> = MaybeUninit::uninit();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static READ_BYTES: AtomicUsize = AtomicUsize::new(0);
const SCHEDULER_EVENT: phoenix_kernel::time::TimerEvent =
    phoenix_kernel::time::TimerEvent::new(0x555352);
const QUANTUM_NS: u64 = 2_000_000;

pub(super) fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}
fn state() -> &'static mut State {
    assert!(active() && irqs_masked());
    let pointer = &raw mut STATE;
    // SAFETY: Only masked, non-reentrant boot/exception code on CPU 0 calls this.
    // No borrow survives return to EL0 or a second call to this accessor.
    unsafe { (*pointer).assume_init_mut() }
}

fn load<
    M: phoenix_kernel::process_image::ProcessImageMemory
        + phoenix_kernel::arch::aarch64::user_page_table::TranslationTableMemory,
>(
    archive: Initramfs<'static>,
    path: &str,
    allocator: &mut FrameAllocator<INIT_MEMORY_RANGES>,
    memory: &mut M,
) -> Result<InitAddressSpace, Errno> {
    let bytes = archive
        .executable_file(path)
        .map_err(|_| Errno::NoSuchFile)?;
    let image = ElfImage::parse(bytes).map_err(|_| Errno::InvalidArgument)?;
    let stack = UserStackLayout::new(UserAddr::new(DEFAULT_USER_STACK_TOP).unwrap(), 4, 1).unwrap();
    let initial = InitialStackImage::<INIT_STACK_BYTES>::new(stack, image.entry(), &[path], &[])
        .map_err(|_| Errno::InvalidArgument)?;
    let plan = ProcessImagePlan::<INIT_IMAGE_MAPPINGS, INIT_IMAGE_PAGES>::with_initial_stack(
        image, &initial,
    )
    .map_err(|_| Errno::InvalidArgument)?;
    let before = *allocator;
    // No owner is published during this transaction. Every temporary owner is
    // dropped before restoring the allocator on any error; no other allocation
    // can race the snapshot while this single-CPU dispatcher masks IRQs.
    let result = (|| {
        let allocated = plan.allocate(allocator).map_err(|_| Errno::NoMemory)?;
        let populated = allocated.populate(memory).map_err(|_| Errno::InputOutput)?;
        let mut tables = UserPageTablePlan::<INIT_TABLE_PAGES, INIT_TABLE_LEAVES>::new()
            .map_err(|_| Errno::NoMemory)?;
        tables
            .map_process_image(&populated)
            .map_err(|_| Errno::InvalidArgument)?;
        let tables = tables.allocate(allocator).map_err(|_| Errno::NoMemory)?;
        let tables = tables.materialize(memory).map_err(|_| Errno::InputOutput)?;
        PreparedUserAddressSpace::new(populated, tables).map_err(|_| Errno::InvalidArgument)
    })();
    if result.is_err() {
        *allocator = before;
    }
    result
}

pub(super) fn boot(argument: usize, console: &mut Console<EarlyPl011>) -> ! {
    let archive = Initramfs::from_bytes(INITRAMFS).expect("system archive");
    // SAFETY: Bootstrap mappings still cover all privately allocated RAM.
    let mut memory = unsafe { BootstrapPhysicalMemory::assume_bootstrap_mapping() };
    let (plan, map, gic, test, stress, initial_path, preempt) = {
        let tree = memory
            .device_tree(PhysAddr::new(argument))
            .expect("system DTB");
        let info = tree.boot_info().expect("system boot information");
        // SAFETY: Sole CPU owns bootstrap device access, with delivery closed.
        let gic = unsafe { discover_boot_gicv3(tree) }.expect("system GIC discovery");
        let plan = final_kernel_page_table_plan(info, gic.layout()).expect("system final map");
        let map = memory_map_from_boot_info::<INIT_MEMORY_RANGES>(
            info,
            kernel_physical_range().unwrap(),
            PhysAddr::new(argument),
        )
        .expect("system memory map");
        (
            plan,
            map,
            gic,
            matches!(
                info.bootargs(),
                Some(
                    "phoenix.system-test"
                        | "phoenix.system-stress"
                        | "phoenix.system-cooperative"
                        | "phoenix.system-preempt"
                        | "phoenix.system-preempt-off"
                        | "phoenix.system-cooperative-preempt"
                        | "phoenix.system-sleep"
                )
            ),
            info.bootargs() == Some("phoenix.system-stress"),
            match info.bootargs() {
                Some("phoenix.system-stress") => "stress",
                Some("phoenix.system-cooperative" | "phoenix.system-cooperative-preempt") => {
                    "cooperate"
                }
                Some("phoenix.system-preempt" | "phoenix.system-preempt-off") => "preempt",
                Some("phoenix.system-sleep") => "sleeptest",
                _ => "init",
            },
            !matches!(
                info.bootargs(),
                Some("phoenix.system-cooperative" | "phoenix.system-preempt-off")
            ),
        )
    };
    let mut allocator = map.into_allocator();
    let layout =
        final_direct_map_layout(&plan, allocator.managed_frames()).expect("system direct map");
    let tables = plan
        .allocate(&mut allocator)
        .expect("system table frames")
        .materialize(&mut memory)
        .unwrap_or_else(|_| panic!("system table materialization"));
    let init = load(archive, initial_path, &mut allocator, &mut memory).expect("system init image");
    drop(memory);
    let pointer = &raw mut TABLES;
    // SAFETY: One-time pinned initialization; all translations and capabilities
    // below borrow this permanent table owner, disjoint from mutable State.
    let tables = unsafe {
        (*pointer).write(tables);
        &*(*pointer).as_ptr()
    };
    // SAFETY: Plan covers current stack, image, vectors and exact device pages.
    unsafe {
        tables.activate();
        prepare_and_publish_boot_interrupt_runtime(tables, gic);
    }
    let mut asids = AsidAllocator::new();
    let asid = asids.allocate().unwrap();
    let mut processes = ProcessTable::new();
    let id = processes
        .insert(Resources {
            space: init,
            asid,
            files: FileTable::new(),
            suspended: None,
            thread: None,
        })
        .unwrap_or_else(|_| panic!("init process slot"));
    processes.make_ready(id).unwrap();
    processes.start(id).unwrap();
    let mut queue = Scheduler::new();
    let thread = queue.spawn().unwrap();
    processes.resources_mut(id).unwrap().thread = Some(thread);
    let mut scheduled = [None; 2];
    scheduled[thread.slot()] = Some(id);
    assert_eq!(queue.select(cpu::counter()), Some(thread));
    let allocator_without_child = allocator;
    let asids_without_child = asids.remaining();
    let runtime = State {
        processes,
        current: id,
        root: id,
        child: None,
        queue,
        scheduled,
        preempt,
        timer: None,
        ticks: [0; 2],
        timer_switches: 0,
        idle: false,
        idle_top: None,
        idle_ticks: 0,
        child_parent_phases: [0; 3],
        allocator,
        asids,
        tables,
        layout,
        archive,
        ram: RamFs::new(),
        children: 0,
        faults: 0,
        test,
        allocator_without_child,
        asids_without_child,
    };
    let pointer = &raw mut STATE;
    // SAFETY: Published once before first EL0 entry, retained for the entire boot.
    unsafe {
        (*pointer).write(runtime);
    }
    ACTIVE.store(true, Ordering::Release);
    events::record(2, id.slot() as u64, id.generation());
    let runtime = state();
    if stress {
        hardening::run(runtime, console);
    }
    if preempt {
        boot_interrupt_handle()
            .unwrap()
            .activate(&LocalIrq::new())
            .expect("user GIC activation");
        arm(runtime);
    }
    let resource = runtime.processes.get(id).unwrap().resources();
    let _ = writeln!(
        console,
        "PHOENIX_SYSTEM_ENTER pid={}.{} asid={}",
        id.slot(),
        id.generation(),
        asid.raw()
    );
    console.sink_mut().flush();
    let frame = initial_frame(
        resource.space.entry().as_usize() as u64,
        resource.space.stack_pointer().as_usize() as u64,
        preempt,
    );
    // SAFETY: Process table retains the root; no borrow survives entry. The
    // complete initial frame resides on the permanently owned construction stack.
    unsafe {
        resource.space.activate_for_return(asid);
    }
    // SAFETY: Same root/stack ownership; no mutable runtime access remains live.
    unsafe {
        phoenix_kernel::arch::aarch64::thread::enter_user(&frame);
    }
}

fn initial_frame(entry: u64, stack: u64, preempt: bool) -> ExceptionFrame {
    if preempt {
        ExceptionFrame::preemptible_user_process(entry, stack)
    } else {
        ExceptionFrame::user_process(entry, stack)
    }
}

fn arm(runtime: &mut State) {
    let deadline = cpu::frequency()
        .unwrap()
        .deadline_after_nanoseconds(cpu::counter(), QUANTUM_NS)
        .unwrap();
    runtime.timer = Some(
        boot_interrupt_handle()
            .unwrap()
            .schedule(&LocalIrq::new(), deadline, SCHEDULER_EVENT)
            .expect("user scheduler tick"),
    );
}

fn interrupt(runtime: &mut State, frame: &mut ExceptionFrame, slot: VectorSlot) {
    assert!(runtime.preempt && irqs_masked());
    if runtime.idle {
        assert_eq!(
            slot,
            VectorSlot::new(VectorOrigin::CurrentSpX, ExceptionKind::Irq)
        );
        assert_eq!(
            frame.saved_program_status() & 0x8f,
            5,
            "idle must be EL1h with I clear"
        );
        let entry = phoenix_kernel::arch::aarch64::thread::idle_forever as *const () as u64;
        assert!(
            (entry..entry + 8).contains(&frame.program_counter()),
            "IRQ outside idle loop"
        );
        assert_eq!(
            Some(frame as *mut ExceptionFrame as usize + ExceptionFrame::SIZE),
            runtime.idle_top
        );
    } else {
        assert_eq!(
            slot,
            VectorSlot::new(VectorOrigin::LowerAArch64, ExceptionKind::Irq)
        );
        assert_eq!(
            frame.saved_program_status() & 0x8f,
            0,
            "IRQ must interrupt EL0 with I clear"
        );
    }
    // dispatch_one releases both runtime/timer locks and completes GIC EOI before
    // returning a batch. No interrupt-runtime guard may survive select below.
    match boot_interrupt_handle()
        .unwrap()
        .dispatch_one(&LocalIrq::new())
        .expect("user IRQ dispatch")
    {
        InterruptDispatch::Spurious { .. } => (),
        InterruptDispatch::Timer(batch) if batch.is_empty() => (),
        InterruptDispatch::Timer(batch) => {
            assert_eq!(batch.len(), 1);
            let fired = batch.iter().next().unwrap();
            assert_eq!(Some(fired.id()), runtime.timer.take());
            assert_eq!(fired.event(), SCHEDULER_EVENT);
            if runtime.idle {
                runtime.idle_ticks += 1;
            } else {
                runtime.ticks[runtime.current.slot()] += 1;
            }
            arm(runtime);
            let old = (!runtime.idle).then_some(runtime.current);
            select(runtime, frame);
            runtime.timer_switches +=
                usize::from(old != (!runtime.idle).then_some(runtime.current));
        }
        InterruptDispatch::Unexpected { intid } => panic!("unexpected user IRQ {intid}"),
    }
}

fn memory(
    runtime: &State,
) -> FinalDirectMapMemory<'static, FINAL_KERNEL_TABLE_CAPACITY, INIT_MEMORY_RANGES> {
    // SAFETY: Permanent table owner is active. Each subsequent operation proves
    // frame ownership through the current address space or unpublished loader.
    unsafe { FinalDirectMapMemory::assume_active(runtime.tables, runtime.layout) }
}
fn copy_from(runtime: &State, address: u64, output: &mut [u8]) -> Result<(), Errno> {
    let resource = runtime.processes.get(runtime.current).unwrap().resources();
    resource
        .space
        .copy_from_user(&mut memory(runtime), address, output)
        .map_err(|_| Errno::BadAddress)
}
fn copy_to(runtime: &State, address: u64, input: &[u8]) -> Result<(), Errno> {
    let resource = runtime.processes.get(runtime.current).unwrap().resources();
    resource
        .space
        .copy_to_user(&mut memory(runtime), address, input)
        .map_err(|_| Errno::BadAddress)
}

pub(super) fn dispatch(frame: &mut ExceptionFrame, slot: Option<VectorSlot>) {
    let runtime = state();
    let slot = slot.expect("system exception vector");
    if slot.kind() == ExceptionKind::Irq {
        interrupt(runtime, frame, slot);
        return;
    }
    assert!(!runtime.idle, "non-IRQ exception from idle");
    if slot == VectorSlot::new(VectorOrigin::LowerAArch64, ExceptionKind::Synchronous)
        && frame.syndrome().exception_class() == ExceptionClass::SupervisorCallAArch64
        && frame.syndrome().supervisor_call_immediate() == Some(0)
    {
        let request = syscall::request_from_frame(frame);
        let args = request.arguments();
        let number = request.operation().number();
        if number == 0 {
            runtime
                .processes
                .exit(runtime.current, ExitStatus::new(args[0]))
                .unwrap();
            finish(runtime, frame);
            return;
        }
        if number == 5 || number == 6 {
            if let Err(error) = run_child(runtime, frame, args[0], args[1], number == 5) {
                syscall::set_return(frame, SyscallReturn::error(error));
            } else if number == 6 {
                syscall::set_return(frame, SyscallReturn::success(0).unwrap());
            }
            return;
        }
        if number == 7 {
            if let Err(error) = wait_child(runtime, frame) {
                syscall::set_return(frame, SyscallReturn::error(error));
            }
            return;
        }
        if number == 8 {
            syscall::set_return(frame, SyscallReturn::success(0).unwrap());
            select(runtime, frame);
            return;
        }
        if number == 9 {
            if let Err(error) = sleep(runtime, frame, args[0]) {
                syscall::set_return(frame, SyscallReturn::error(error));
            }
            return;
        }
        let result = io(runtime, number, args);
        syscall::set_return(
            frame,
            match result {
                Ok(value) => SyscallReturn::success(value).unwrap(),
                Err(error) => SyscallReturn::error(error),
            },
        );
        return;
    }
    if let Some(fault) = classify_user_fault(slot, frame) {
        events::record(4, runtime.current.generation(), fault.architecture_code());
        runtime.processes.fault(runtime.current, fault).unwrap();
        runtime.faults += 1;
        let mut console = Console::new(EarlyPl011::new());
        let _ = writeln!(
            console,
            "PHOENIX_USER_FAULT pid={}.{} kind={:?} pc={:#x} address={:?} esr={:#x}",
            runtime.current.slot(),
            runtime.current.generation(),
            fault.kind(),
            fault.instruction_pointer(),
            fault.fault_address(),
            fault.architecture_code()
        );
        finish(runtime, frame);
        return;
    }
    panic!(
        "unexpected system exception: slot={slot:?} pc={:#x} esr={:#x}",
        frame.program_counter(),
        frame.syndrome().raw()
    );
}

fn run_child(
    runtime: &mut State,
    frame: &mut ExceptionFrame,
    pointer: u64,
    length: u64,
    wait: bool,
) -> Result<(), Errno> {
    if runtime.current != runtime.root || runtime.child.is_some() {
        return Err(Errno::TryAgain);
    }
    let length = usize::try_from(length)
        .ok()
        .filter(|len| (1..=128).contains(len))
        .ok_or(Errno::InvalidArgument)?;
    let mut path = [0u8; 128];
    copy_from(runtime, pointer, &mut path[..length])?;
    let path = core::str::from_utf8(&path[..length]).map_err(|_| Errno::InvalidArgument)?;
    spawn_child(runtime, frame, path, wait)
}

fn spawn_child(
    runtime: &mut State,
    frame: &mut ExceptionFrame,
    path: &str,
    wait: bool,
) -> Result<(), Errno> {
    let mut memory = memory(runtime);
    let space = load(runtime.archive, path, &mut runtime.allocator, &mut memory)?;
    let asid = match runtime.asids.allocate() {
        Ok(asid) => asid,
        Err(_) => {
            space.release_unpublished(&mut runtime.allocator).unwrap();
            return Err(Errno::NoMemory);
        }
    };
    let initial = initial_frame(
        space.entry().as_usize() as u64,
        space.stack_pointer().as_usize() as u64,
        runtime.preempt,
    );
    let resources = Resources {
        space,
        asid,
        files: FileTable::new(),
        suspended: Some(initial),
        thread: None,
    };
    let child = match runtime.processes.insert(resources) {
        Ok(id) => id,
        Err(error) => {
            error
                .into_resources()
                .space
                .release_unpublished(&mut runtime.allocator)
                .unwrap();
            // SAFETY: This ASID was allocated but its root was never installed.
            unsafe { runtime.asids.release_retired(asid) }.unwrap();
            return Err(Errno::TryAgain);
        }
    };
    // Process and queue slots have the same fixed capacity and are published /
    // retired together. No handler can observe the intermediate binding while
    // local IRQs are masked. A successful insertion proves queue capacity.
    let thread = runtime
        .queue
        .spawn()
        .expect("process/queue capacity invariant");
    runtime.processes.resources_mut(child).unwrap().thread = Some(thread);
    runtime.scheduled[thread.slot()] = Some(child);
    runtime.processes.make_ready(child).unwrap();
    runtime.child = Some(child);
    events::record(2, child.slot() as u64, child.generation());
    if wait {
        wait_child(runtime, frame).expect("new child belongs to running root");
    }
    Ok(())
}

fn thread(runtime: &State, id: ProcessId) -> ThreadId {
    runtime
        .processes
        .get(id)
        .unwrap()
        .resources()
        .thread
        .unwrap()
}

fn activate(runtime: &State, id: ProcessId) {
    let resource = runtime.processes.get(id).unwrap().resources();
    // SAFETY: The table retains this complete root. Local IRQs are masked and
    // no user-memory borrow survives dispatch. Global TLBI completes before any
    // retired root's frames or ASID may be returned by reap_child.
    unsafe { resource.space.activate_for_return(resource.asid) };
}

/// Copy complete contexts at the outer exception boundary. The shared kernel
/// stack never contains a suspended Rust continuation; only owned frame values
/// survive this call. No lock guard or State borrow survives assembly ERET.
fn select(runtime: &mut State, frame: &mut ExceptionFrame) {
    let old = runtime.current;
    if !runtime.idle
        && runtime.processes.get(old).unwrap().state()
            == phoenix_kernel::process::ProcessState::Running
    {
        runtime.processes.resources_mut(old).unwrap().suspended = Some(*frame);
        runtime.processes.deschedule(old).unwrap();
    }
    let now = cpu::counter();
    let next = runtime.queue.select(now);
    // Policy wakes every expired sleeper before selection; mirror those events
    // in the resource-owning lifecycle before any selected process can run.
    for id in runtime.scheduled.into_iter().flatten() {
        if let phoenix_kernel::process::ProcessState::Sleeping(until) =
            runtime.processes.get(id).unwrap().state()
            && until <= now
        {
            runtime.processes.wake_sleep(id, now).unwrap();
            let mut console = Console::new(EarlyPl011::new());
            let _ = writeln!(
                console,
                "PHOENIX_WAKE pid={}.{} deadline={} now={}",
                id.slot(),
                id.generation(),
                until.raw(),
                now.raw()
            );
        }
    }
    let Some(next) = next else {
        assert!(runtime.preempt, "idle requires an active wake source");
        let top = frame as *mut ExceptionFrame as usize + ExceptionFrame::SIZE;
        if let Some(expected) = runtime.idle_top {
            assert_eq!(top, expected, "idle stack drift");
        }
        runtime.idle_top = Some(top);
        runtime.idle = true;
        *frame = ExceptionFrame::kernel_thread(
            phoenix_kernel::arch::aarch64::thread::idle_forever as *const () as u64,
            0,
        );
        return;
    };
    let id = runtime.scheduled[next.slot()].expect("published process binding");
    assert_eq!(thread(runtime, id), next, "stale queue binding");
    runtime.processes.start(id).unwrap();
    activate(runtime, id);
    runtime.current = id;
    runtime.idle = false;
    *frame = runtime
        .processes
        .resources_mut(id)
        .unwrap()
        .suspended
        .take()
        .unwrap();
    events::record(5, old.slot() as u64, id.slot() as u64);
}

fn sleep(runtime: &mut State, frame: &mut ExceptionFrame, nanoseconds: u64) -> Result<(), Errno> {
    if nanoseconds > 1_000_000_000 {
        return Err(Errno::InvalidArgument);
    }
    if nanoseconds == 0 {
        syscall::set_return(frame, SyscallReturn::success(0).unwrap());
        select(runtime, frame);
        return Ok(());
    }
    if !runtime.preempt {
        return Err(Errno::NotImplemented);
    }
    let now = cpu::counter();
    let frequency = cpu::frequency().unwrap();
    let until = frequency
        .deadline_after_nanoseconds(now, nanoseconds)
        .map_err(|_| Errno::InvalidArgument)?;
    let id = runtime.current;
    syscall::set_return(frame, SyscallReturn::success(0).unwrap());
    runtime.processes.resources_mut(id).unwrap().suspended = Some(*frame);
    runtime.processes.sleep(id, until).unwrap();
    runtime.queue.sleep(until).unwrap();
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(
        console,
        "PHOENIX_SLEEP pid={}.{} start={} ns={} hz={} deadline={}",
        id.slot(),
        id.generation(),
        now.raw(),
        nanoseconds,
        frequency.ticks_per_second(),
        until.raw()
    );
    select(runtime, frame);
    Ok(())
}

fn wait_child(runtime: &mut State, frame: &mut ExceptionFrame) -> Result<(), Errno> {
    if runtime.current != runtime.root {
        return Err(Errno::NoChild);
    }
    let child = runtime.child.ok_or(Errno::NoChild)?;
    if runtime
        .processes
        .get(child)
        .unwrap()
        .state()
        .termination()
        .is_some()
    {
        let result = reap_child(runtime);
        syscall::set_return(frame, result);
    } else {
        let parent = runtime.current;
        runtime.processes.resources_mut(parent).unwrap().suspended = Some(*frame);
        runtime.processes.wait_for_child(parent).unwrap();
        runtime.queue.block().unwrap();
        select(runtime, frame);
    }
    Ok(())
}

fn reap_child(runtime: &mut State) -> SyscallReturn {
    assert_eq!(runtime.current, runtime.root);
    let child = runtime.child.take().expect("owned child");
    let id = thread(runtime, child);
    assert_eq!(runtime.queue.state(id), Ok(ThreadState::Exited));
    let reaped = runtime.processes.reap(child).unwrap();
    let result = termination_result(reaped.termination());
    let was_fault = matches!(reaped.termination(), ProcessTermination::Faulted(_));
    let resource = reaped.into_resources();
    // SAFETY: Child is terminal and no longer current, all translations
    // were invalidated above, and no borrow of its frames remains.
    unsafe { resource.space.release_retired(&mut runtime.allocator) }.unwrap();
    // SAFETY: Parent root is current and global TLBI completed before reap;
    // the child root owner has been consumed and can never activate again.
    unsafe { runtime.asids.release_retired(resource.asid) }.unwrap();
    runtime.queue.reap(id).unwrap();
    runtime.scheduled[id.slot()] = None;
    assert!(
        runtime.queue.state(id).is_err(),
        "retired queue ID still resolves"
    );
    assert_eq!(
        runtime.scheduled.iter().flatten().count(),
        1,
        "queue binding leak"
    );
    assert_eq!(
        runtime.allocator, runtime.allocator_without_child,
        "child changed free-frame ownership"
    );
    assert_eq!(
        runtime.asids.remaining(),
        runtime.asids_without_child,
        "child ASID leak"
    );
    assert_eq!(runtime.processes.len(), 1, "child process slot leak");
    assert!(
        runtime.processes.get(child).is_err(),
        "retired PID still resolves"
    );
    runtime.children += 1;
    events::record(3, child.generation(), resource.asid.raw() as u64);
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(
        console,
        "child: reaped={}.{} fault={was_fault} frames=restored asid={} slots=restored",
        child.slot(),
        child.generation(),
        resource.asid.raw()
    );
    result
}

fn termination_result(termination: ProcessTermination) -> SyscallReturn {
    match termination {
        ProcessTermination::Exited(status) => SyscallReturn::success(status.raw())
            .unwrap_or_else(|_| SyscallReturn::error(Errno::InvalidArgument)),
        ProcessTermination::Faulted(_) => SyscallReturn::error(Errno::BadAddress),
    }
}

fn finish(runtime: &mut State, frame: &mut ExceptionFrame) {
    runtime.queue.exit().unwrap();
    if runtime.current != runtime.root {
        assert_eq!(runtime.child, Some(runtime.current));
        let parent = runtime.root;
        let phase = match runtime.processes.get(parent).unwrap().state() {
            phoenix_kernel::process::ProcessState::Waiting => 0,
            phoenix_kernel::process::ProcessState::Sleeping(_) => 1,
            phoenix_kernel::process::ProcessState::Ready => 2,
            other => panic!("invalid parent state at child completion: {other:?}"),
        };
        runtime.child_parent_phases[phase] += 1;
        if runtime.processes.get(parent).unwrap().state()
            == phoenix_kernel::process::ProcessState::Waiting
        {
            activate(runtime, parent);
            runtime.current = parent;
            let result = reap_child(runtime);
            let saved = runtime
                .processes
                .resources_mut(parent)
                .unwrap()
                .suspended
                .as_mut()
                .unwrap();
            syscall::set_return(saved, result);
            runtime.processes.wake(parent).unwrap();
            runtime.queue.wake(thread(runtime, parent)).unwrap();
        }
        select(runtime, frame);
        return;
    }
    let result = termination_result(
        runtime
            .processes
            .get(runtime.root)
            .unwrap()
            .state()
            .termination()
            .unwrap(),
    );
    if runtime.preempt {
        let handle = boot_interrupt_handle().unwrap();
        let irq = LocalIrq::new();
        if let Some(timer) = runtime.timer.take() {
            handle
                .cancel(&irq, timer)
                .expect("cancel user scheduler tick");
        }
        handle.deactivate(&irq).expect("close user IRQ delivery");
    }
    let mut console = Console::new(EarlyPl011::new());
    let faulted = matches!(
        runtime.processes.get(runtime.root).unwrap().state(),
        phoenix_kernel::process::ProcessState::Faulted(_)
    );
    events::dump(&mut console);
    let _ = writeln!(
        console,
        "PHOENIX_CHILD_DONE waiting={} sleeping={} ready={}",
        runtime.child_parent_phases[0],
        runtime.child_parent_phases[1],
        runtime.child_parent_phases[2]
    );
    let _ = writeln!(console, "PHOENIX_IDLE ticks={}", runtime.idle_ticks);
    let _ = writeln!(
        console,
        "PHOENIX_SCHED root-ticks={} child-ticks={} switches={} child-ready={}",
        runtime.ticks[0],
        runtime.ticks[1],
        runtime.timer_switches,
        runtime
            .child
            .is_some_and(|child| runtime.processes.get(child).unwrap().state()
                == phoenix_kernel::process::ProcessState::Ready)
    );
    let _ = writeln!(
        console,
        "system: children={} faults={} console-read={} frame-accounting=ok",
        runtime.children,
        runtime.faults,
        READ_BYTES.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        console,
        "PHOENIX_SYSTEM_EXIT status={} fault={faulted}",
        result.raw() as i64
    );
    console.sink_mut().flush();
    if runtime.test {
        // SAFETY: Only the explicit system-test boot argument enables this path.
        unsafe {
            cpu::test_exit(usize::from(
                faulted || result != SyscallReturn::success(0).unwrap(),
            ));
        }
    }
    halt()
}

fn io(runtime: &mut State, number: u64, args: [u64; 6]) -> Result<u64, Errno> {
    let [fd, pointer, length, ..] = args;
    let mut buffer = [0u8; 256];
    match number {
        1 => {
            let length = usize::try_from(length)
                .ok()
                .filter(|length| *length <= buffer.len())
                .ok_or(Errno::InvalidArgument)?;
            copy_from(runtime, pointer, &mut buffer[..length])?;
            if fd == 1 || fd == 2 {
                let mut console = EarlyPl011::new();
                for byte in &buffer[..length] {
                    console.write_byte(*byte);
                }
                console.flush();
                Ok(length as u64)
            } else {
                let files = &mut runtime
                    .processes
                    .resources_mut(runtime.current)
                    .unwrap()
                    .files;
                files
                    .write(&mut runtime.ram, fd, &buffer[..length])
                    .map(|count| count as u64)
            }
        }
        2 => {
            let length = usize::try_from(length)
                .ok()
                .filter(|length| *length <= buffer.len())
                .ok_or(Errno::InvalidArgument)?;
            if fd == 0 {
                // Prevalidate the entire writable request before consuming UART
                // input. Copying identical bytes preserves the user's contents.
                copy_from(runtime, pointer, &mut buffer[..length])?;
                copy_to(runtime, pointer, &buffer[..length])?;
                if length == 0 {
                    return Ok(0);
                }
                let mut console = EarlyPl011::new();
                let mut count = 0;
                while count < length {
                    let Some(byte) = console.read_byte() else {
                        break;
                    };
                    buffer[count] = byte;
                    count += 1;
                }
                if count == 0 {
                    return Err(Errno::TryAgain);
                }
                copy_to(runtime, pointer, &buffer[..count])?;
                READ_BYTES.fetch_add(count, Ordering::Relaxed);
                Ok(count as u64)
            } else {
                let read = runtime
                    .processes
                    .get(runtime.current)
                    .unwrap()
                    .resources()
                    .files
                    .read(&runtime.ram, fd, &mut buffer[..length])?;
                copy_to(runtime, pointer, &buffer[..read.len()])?;
                runtime
                    .processes
                    .resources_mut(runtime.current)
                    .unwrap()
                    .files
                    .commit_read(read)?;
                Ok(read.len() as u64)
            }
        }
        3 => {
            let length = usize::try_from(pointer)
                .ok()
                .filter(|length| (1..=128).contains(length))
                .ok_or(Errno::InvalidArgument)?;
            copy_from(runtime, fd, &mut buffer[..length])?;
            let path =
                core::str::from_utf8(&buffer[..length]).map_err(|_| Errno::InvalidArgument)?;
            runtime
                .processes
                .resources_mut(runtime.current)
                .unwrap()
                .files
                .open(runtime.archive, &mut runtime.ram, path, args[2])
        }
        4 => {
            runtime
                .processes
                .resources_mut(runtime.current)
                .unwrap()
                .files
                .close(fd)?;
            Ok(0)
        }
        _ => Err(Errno::NotImplemented),
    }
}
