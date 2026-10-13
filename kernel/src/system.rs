//! Serialized two-process native system. A parent waits while its child runs.
//!
//! All mutable state belongs to CPU 0 with DAIF.I masked. EL0 never borrows
//! kernel objects. Root switches invalidate translations before child reaping;
//! no resource or frame reference escapes one synchronous dispatch operation.
use super::*;
use core::sync::atomic::AtomicUsize;
use phoenix_kernel::arch::aarch64::diagnostics as cpu;
use phoenix_kernel::process::{ProcessTable, ProcessTermination};
use phoenix_kernel::vfs::{FileTable, RamFs};

struct Resources {
    space: InitAddressSpace,
    asid: AddressSpaceId,
    files: FileTable<'static, 8>,
    suspended: Option<ExceptionFrame>,
}
struct State {
    processes: ProcessTable<Resources, 2>,
    current: ProcessId,
    parent: Option<ProcessId>,
    allocator: FrameAllocator<INIT_MEMORY_RANGES>,
    asids: AsidAllocator,
    tables: &'static FinalKernelTables,
    layout: FinalDirectMapLayout<INIT_MEMORY_RANGES>,
    archive: Initramfs<'static>,
    ram: RamFs<8, 1024>,
    children: usize,
    faults: usize,
    test: bool,
    free_without_child: usize,
}
static mut STATE: MaybeUninit<State> = MaybeUninit::uninit();
static mut TABLES: MaybeUninit<FinalKernelTables> = MaybeUninit::uninit();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static READ_BYTES: AtomicUsize = AtomicUsize::new(0);

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
    let (plan, map, gic, test) = {
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
            info.bootargs() == Some("phoenix.system-test"),
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
    let init = load(archive, "init", &mut allocator, &mut memory).expect("system init image");
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
        })
        .unwrap_or_else(|_| panic!("init process slot"));
    processes.make_ready(id).unwrap();
    processes.start(id).unwrap();
    let free_without_child = allocator.total_free_frames();
    let runtime = State {
        processes,
        current: id,
        parent: None,
        allocator,
        asids,
        tables,
        layout,
        archive,
        ram: RamFs::new(),
        children: 0,
        faults: 0,
        test,
        free_without_child,
    };
    let pointer = &raw mut STATE;
    // SAFETY: Published once before first EL0 entry, retained for the entire boot.
    unsafe {
        (*pointer).write(runtime);
    }
    ACTIVE.store(true, Ordering::Release);
    events::record(2, id.slot() as u64, id.generation());
    let runtime = state();
    let resource = runtime.processes.get(id).unwrap().resources();
    let _ = writeln!(
        console,
        "PHOENIX_SYSTEM_ENTER pid={}.{} asid={}",
        id.slot(),
        id.generation(),
        asid.raw()
    );
    console.sink_mut().flush();
    let frame = ExceptionFrame::user_process(
        resource.space.entry().as_usize() as u64,
        resource.space.stack_pointer().as_usize() as u64,
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
            finish(
                runtime,
                frame,
                SyscallReturn::success(args[0])
                    .unwrap_or_else(|_| SyscallReturn::error(Errno::InvalidArgument)),
            );
            return;
        }
        if number == 5 {
            if let Err(error) = run_child(runtime, frame, args[0], args[1]) {
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
        finish(runtime, frame, SyscallReturn::error(Errno::BadAddress));
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
) -> Result<(), Errno> {
    if runtime.parent.is_some() {
        return Err(Errno::TryAgain);
    }
    let length = usize::try_from(length)
        .ok()
        .filter(|len| (1..=128).contains(len))
        .ok_or(Errno::InvalidArgument)?;
    let mut path = [0u8; 128];
    copy_from(runtime, pointer, &mut path[..length])?;
    let path = core::str::from_utf8(&path[..length]).map_err(|_| Errno::InvalidArgument)?;
    let mut memory = memory(runtime);
    let space = load(runtime.archive, path, &mut runtime.allocator, &mut memory)?;
    let asid = match runtime.asids.allocate() {
        Ok(asid) => asid,
        Err(_) => {
            space.release_unpublished(&mut runtime.allocator).unwrap();
            return Err(Errno::NoMemory);
        }
    };
    let initial = ExceptionFrame::user_process(
        space.entry().as_usize() as u64,
        space.stack_pointer().as_usize() as u64,
    );
    let resources = Resources {
        space,
        asid,
        files: FileTable::new(),
        suspended: None,
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
    let parent = runtime.current;
    runtime.processes.resources_mut(parent).unwrap().suspended = Some(*frame);
    runtime.processes.wait_for_child(parent).unwrap();
    runtime.processes.make_ready(child).unwrap();
    runtime.processes.start(child).unwrap();
    runtime.current = child;
    runtime.parent = Some(parent);
    events::record(2, child.slot() as u64, child.generation());
    let resource = runtime.processes.get(child).unwrap().resources();
    // SAFETY: Process table retains both roots. No user borrow is live, IRQs
    // remain masked, and global invalidation precedes return into this child.
    unsafe {
        resource.space.activate_for_return(resource.asid);
    }
    *frame = initial;
    Ok(())
}

fn finish(runtime: &mut State, frame: &mut ExceptionFrame, result: SyscallReturn) {
    let child = runtime.current;
    if let Some(parent) = runtime.parent.take() {
        let resource = runtime.processes.get(parent).unwrap().resources();
        // SAFETY: The retained parent's mapping is complete. The full switch
        // invalidates all child translations before its unique owner is reaped.
        unsafe {
            resource.space.activate_for_return(resource.asid);
        }
        runtime.current = parent;
        let reaped = runtime.processes.reap(child).unwrap();
        let was_fault = matches!(reaped.termination(), ProcessTermination::Faulted(_));
        let resource = reaped.into_resources();
        // SAFETY: Child is terminal and no longer current, all translations
        // were invalidated above, and no borrow of its frames remains.
        unsafe { resource.space.release_retired(&mut runtime.allocator) }.unwrap();
        // SAFETY: Parent root is current and global TLBI completed before reap;
        // the child root owner has been consumed and can never activate again.
        unsafe { runtime.asids.release_retired(resource.asid) }.unwrap();
        assert_eq!(
            runtime.allocator.total_free_frames(),
            runtime.free_without_child,
            "child frame leak"
        );
        *frame = runtime
            .processes
            .resources_mut(parent)
            .unwrap()
            .suspended
            .take()
            .unwrap();
        syscall::set_return(frame, result);
        runtime.processes.resume(parent).unwrap();
        runtime.children += 1;
        events::record(3, child.generation(), resource.asid.raw() as u64);
        let mut console = Console::new(EarlyPl011::new());
        let _ = writeln!(
            console,
            "child: reaped={}.{} fault={was_fault} frames=restored",
            child.slot(),
            child.generation()
        );
        return;
    }
    let mut console = Console::new(EarlyPl011::new());
    let passed = result == SyscallReturn::success(42).unwrap()
        && runtime.children == 11
        && runtime.faults == 9
        && READ_BYTES.load(Ordering::Relaxed) >= 3;
    events::dump(&mut console);
    let _ = writeln!(
        console,
        "system: children={} faults={} console-read={} frame-accounting=ok",
        runtime.children,
        runtime.faults,
        READ_BYTES.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        console,
        "{}",
        if passed {
            "PHOENIX_SYSTEM_OK"
        } else {
            "PHOENIX_SYSTEM_FAIL"
        }
    );
    console.sink_mut().flush();
    if runtime.test {
        // SAFETY: Only the explicit system-test boot argument enables this path.
        unsafe {
            cpu::test_exit(usize::from(!passed));
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
