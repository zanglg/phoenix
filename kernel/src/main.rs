#![no_main]
#![no_std]

#[cfg(not(target_arch = "aarch64"))]
compile_error!("the Phoenix kernel binary currently supports only AArch64");

use core::fmt::Write;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use core::mem::MaybeUninit;
use core::panic::PanicInfo;
#[cfg(feature = "loaded-init-probe")]
use core::sync::atomic::{AtomicBool, Ordering};

use phoenix_kernel::arch::aarch64::exception::{ExceptionFrame, VectorSlot};
use phoenix_kernel::arch::aarch64::{halt, install_exception_vectors};
use phoenix_kernel::build_info::BuildInfo;
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::console::ByteSink;
use phoenix_kernel::console::Console;
use phoenix_kernel::platform::aarch64::qemu_virt::EarlyPl011;

#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use phoenix_kernel::memory::FrameAllocator;
#[cfg(any(
    feature = "boot-memory-probe",
    feature = "kernel-map-probe",
    feature = "loaded-init-probe"
))]
use phoenix_kernel::memory::{PhysAddr, memory_map_from_boot_info};
#[cfg(any(
    feature = "boot-memory-probe",
    feature = "kernel-map-probe",
    feature = "loaded-init-probe"
))]
use phoenix_kernel::platform::aarch64::qemu_virt::{
    BootstrapPhysicalMemory, kernel_physical_range,
};
#[cfg(feature = "boot-memory-probe")]
use phoenix_kernel::process_image::ProcessImageMemory;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use phoenix_kernel::{
    arch::aarch64::kernel_page_table::MaterializedKernelPageTables,
    platform::aarch64::qemu_virt::{FINAL_KERNEL_TABLE_CAPACITY, final_kernel_page_table_plan},
};

#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::abi::{
    ConsoleWriteRequest, FileCloseRequest, FileOpenRequest, FileReadRequest,
};
#[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
use phoenix_kernel::abi::{Errno, NATIVE_SVC_IMMEDIATE, NativeSyscall, SyscallReturn};
#[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
use phoenix_kernel::arch::aarch64::el0;
#[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
use phoenix_kernel::arch::aarch64::exception::{ExceptionClass, ExceptionKind, VectorOrigin};
#[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
use phoenix_kernel::arch::aarch64::syscall;
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::arch::aarch64::user_page_table::{PreparedUserAddressSpace, UserPageTablePlan};
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::elf::ElfImage;
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::file::{FileError, ReadOnlyFileTable};
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::initramfs::Initramfs;
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::platform::aarch64::qemu_virt::{
    BootstrapMemoryError, read_direct_mapped_frame, write_direct_mapped_frame,
};
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::process_image::ProcessImagePlan;
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::user::{DEFAULT_USER_STACK_TOP, UserAddr, UserStackLayout};
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::user_copy::{UserMemoryReader, UserMemoryWriter};
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::user_stack::InitialStackImage;

core::arch::global_asm!(include_str!("arch/aarch64/boot.S"));
core::arch::global_asm!(include_str!("arch/aarch64/vectors.S"));
#[cfg(feature = "el0-probe")]
core::arch::global_asm!(include_str!("arch/aarch64/user_probe.S"));

const BOOT_SUCCESS_SENTINEL: &str = "PHOENIX_BOOT_OK";
const PANIC_SENTINEL: &str = "PHOENIX_PANIC";
const EXCEPTION_SENTINEL: &str = "PHOENIX_EXCEPTION";
#[cfg(feature = "boot-memory-probe")]
const MEMORY_SUCCESS_SENTINEL: &str = "PHOENIX_MEMORY_OK";
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
const KERNEL_MAP_ENTER_SENTINEL: &str = "PHOENIX_KERNEL_MAP_ENTER";
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
const KERNEL_MAP_SUCCESS_SENTINEL: &str = "PHOENIX_KERNEL_MAP_OK";
#[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
const EL0_ENTER_SENTINEL: &str = "PHOENIX_EL0_ENTER";
#[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
const EL0_SUCCESS_SENTINEL: &str = "PHOENIX_EL0_OK";
#[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
const EL0_FAILURE_SENTINEL: &str = "PHOENIX_EL0_FAIL";
#[cfg(feature = "loaded-init-probe")]
const INIT_ENTER_SENTINEL: &str = "PHOENIX_INIT_ENTER";
#[cfg(feature = "loaded-init-probe")]
const INIT_KERNEL_MAP_ENTER_SENTINEL: &str = "PHOENIX_INIT_KERNEL_MAP_ENTER";
#[cfg(feature = "loaded-init-probe")]
const INIT_KERNEL_MAP_SENTINEL: &str = "PHOENIX_INIT_KERNEL_MAP_OK";
#[cfg(feature = "loaded-init-probe")]
const INIT_SUCCESS_SENTINEL: &str = "PHOENIX_INIT_OK";
#[cfg(feature = "loaded-init-probe")]
const INIT_FAILURE_SENTINEL: &str = "PHOENIX_INIT_FAIL";

#[cfg(feature = "loaded-init-probe")]
static INITRAMFS: &[u8] = include_bytes!(env!("PHOENIX_INITRAMFS"));

#[cfg(feature = "loaded-init-probe")]
const INIT_MEMORY_RANGES: usize = 16;
#[cfg(feature = "loaded-init-probe")]
const INIT_IMAGE_MAPPINGS: usize = 2;
#[cfg(feature = "loaded-init-probe")]
const INIT_IMAGE_PAGES: usize = 5;
#[cfg(feature = "loaded-init-probe")]
const INIT_TABLE_PAGES: usize = 5;
#[cfg(feature = "loaded-init-probe")]
const INIT_TABLE_LEAVES: usize = INIT_IMAGE_PAGES;
#[cfg(feature = "loaded-init-probe")]
const INIT_STACK_BYTES: usize = 512;
#[cfg(feature = "loaded-init-probe")]
const INIT_WRITE_LIMIT: usize = 256;
#[cfg(feature = "loaded-init-probe")]
const INIT_READ_LIMIT: usize = 256;
#[cfg(feature = "loaded-init-probe")]
const INIT_PATH_LIMIT: usize = 128;
#[cfg(feature = "loaded-init-probe")]
const INIT_OPEN_FILES: usize = 4;
#[cfg(feature = "loaded-init-probe")]
type InitAddressSpace = PreparedUserAddressSpace<
    INIT_IMAGE_MAPPINGS,
    INIT_IMAGE_PAGES,
    INIT_TABLE_PAGES,
    INIT_TABLE_LEAVES,
>;
#[cfg(feature = "loaded-init-probe")]
struct InitRuntime {
    address_space: InitAddressSpace,
    kernel_tables: FinalKernelTables,
    files: ReadOnlyFileTable<'static, INIT_OPEN_FILES>,
    _allocator: FrameAllocator<INIT_MEMORY_RANGES>,
}
#[cfg(feature = "loaded-init-probe")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveInitMemoryError {
    UnownedFrame,
    Platform,
}
#[cfg(feature = "loaded-init-probe")]
struct ActiveInitMemory {
    address_space: &'static InitAddressSpace,
}
#[cfg(feature = "loaded-init-probe")]
impl ActiveInitMemory {
    const fn new(address_space: &'static InitAddressSpace) -> Self {
        Self { address_space }
    }

    fn validate_frame(
        &self,
        frame: phoenix_kernel::memory::PageFrame,
    ) -> Result<(), ActiveInitMemoryError> {
        if self.address_space.owns_frame(frame) {
            Ok(())
        } else {
            Err(ActiveInitMemoryError::UnownedFrame)
        }
    }
}
#[cfg(feature = "loaded-init-probe")]
impl UserMemoryReader for ActiveInitMemory {
    type Error = ActiveInitMemoryError;

    fn read_frame(
        &mut self,
        frame: phoenix_kernel::memory::PageFrame,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.validate_frame(frame)?;
        // SAFETY: the retained address-space owner proves this exact frame is
        // a live private user leaf. The final TTBR1 plan maps every allocator
        // frame through the fixed normal-memory direct map.
        unsafe { read_direct_mapped_frame(frame, offset, output) }
            .map_err(|_error: BootstrapMemoryError| ActiveInitMemoryError::Platform)
    }
}
#[cfg(feature = "loaded-init-probe")]
impl UserMemoryWriter for ActiveInitMemory {
    type Error = ActiveInitMemoryError;

    fn write_frame(
        &mut self,
        frame: phoenix_kernel::memory::PageFrame,
        offset: usize,
        input: &[u8],
    ) -> Result<(), Self::Error> {
        self.validate_frame(frame)?;
        // SAFETY: the retained owner proves this exact frame is a live private
        // user leaf; copy_to_user separately proves its user mapping is writable.
        unsafe { write_direct_mapped_frame(frame, offset, input) }
            .map_err(|_error: BootstrapMemoryError| ActiveInitMemoryError::Platform)
    }
}
#[cfg(feature = "loaded-init-probe")]
static mut INIT_RUNTIME: MaybeUninit<InitRuntime> = MaybeUninit::uninit();
#[cfg(feature = "loaded-init-probe")]
static INIT_WROTE_OUTPUT: AtomicBool = AtomicBool::new(false);

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
const KERNEL_MAP_MEMORY_RANGES: usize = 16;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
type FinalKernelTables = MaterializedKernelPageTables<FINAL_KERNEL_TABLE_CAPACITY>;
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
struct KernelMapRuntime {
    tables: FinalKernelTables,
    _allocator: FrameAllocator<KERNEL_MAP_MEMORY_RANGES>,
}
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
static mut KERNEL_MAP_RUNTIME: MaybeUninit<KernelMapRuntime> = MaybeUninit::uninit();

/// Rust entry point reached by the AArch64 bootstrap.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_argument: usize) -> ! {
    // SAFETY: bootstrap maps the complete linked image as executable normal
    // memory, provides an aligned EL1 stack, and keeps exceptions masked.
    unsafe {
        install_exception_vectors();
    }
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{}", BuildInfo::CURRENT);
    let _ = writeln!(console, "boot argument: {boot_argument:#018x}");
    let _ = writeln!(console, "{BOOT_SUCCESS_SENTINEL}");
    console.sink_mut().flush();

    #[cfg(feature = "boot-memory-probe")]
    run_boot_memory_probe(boot_argument, &mut console);

    #[cfg(all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    ))]
    run_kernel_map_probe(boot_argument, &mut console);

    #[cfg(feature = "loaded-init-probe")]
    run_loaded_init_probe(boot_argument, &mut console);

    #[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
    {
        // SAFETY: this is the single-core boot path and no root has been
        // published; the returned tables are activated immediately below.
        let probe = unsafe { el0::prepare_probe() }
            .unwrap_or_else(|error| panic!("could not prepare EL0 probe: {error:?}"));
        let _ = writeln!(console, "{EL0_ENTER_SENTINEL}");
        console.sink_mut().flush();
        // SAFETY: `prepare_probe` just populated the uniquely owned static
        // root; the bootstrap stack and exception vectors are active.
        unsafe {
            el0::activate_and_enter(probe);
        }
    }

    #[cfg(not(any(
        feature = "el0-probe",
        feature = "kernel-map-probe",
        feature = "loaded-init-probe"
    )))]
    halt()
}

#[cfg(feature = "boot-memory-probe")]
fn run_boot_memory_probe(boot_argument: usize, console: &mut Console<EarlyPl011>) {
    const MEMORY_MAP_CAPACITY: usize = 16;
    const PROBE_BYTES: &[u8] = b"Phoenix bootstrap memory probe";

    // SAFETY: this function is called directly from the single-CPU bootstrap
    // while its documented TTBR1 RAM alias is active. The allocated probe
    // frame remains private until it is returned below.
    let mut physical_memory = unsafe { BootstrapPhysicalMemory::assume_bootstrap_mapping() };
    let device_tree_start = PhysAddr::new(boot_argument);
    let map = {
        let tree = physical_memory
            .device_tree(device_tree_start)
            .unwrap_or_else(|error| panic!("could not borrow boot DTB: {error:?}"));
        let info = tree
            .boot_info()
            .unwrap_or_else(|error| panic!("could not extract boot information: {error:?}"));
        let kernel_image = kernel_physical_range()
            .unwrap_or_else(|error| panic!("could not derive kernel image range: {error:?}"));
        let _ = writeln!(
            console,
            "DTB: size={} boot-cpu={}",
            info.device_tree_size(),
            info.boot_cpu_id()
        );
        memory_map_from_boot_info::<MEMORY_MAP_CAPACITY>(info, kernel_image, device_tree_start)
            .unwrap_or_else(|error| panic!("could not construct boot memory map: {error:?}"))
    };

    let expected_free_frames = map.total_free_frames();
    let mut allocator = map.into_allocator();
    let frame = allocator
        .allocate()
        .unwrap_or_else(|error| panic!("could not allocate memory-probe frame: {error:?}"))
        .unwrap_or_else(|| panic!("boot memory map unexpectedly has no free frame"));

    ProcessImageMemory::clear_frame(&mut physical_memory, frame)
        .unwrap_or_else(|error| panic!("could not clear memory-probe frame: {error:?}"));
    ProcessImageMemory::write_frame(&mut physical_memory, frame, 0, PROBE_BYTES)
        .unwrap_or_else(|error| panic!("could not write memory-probe frame: {error:?}"));
    let mut observed = [0_u8; PROBE_BYTES.len()];
    physical_memory
        .read_frame(frame, 0, &mut observed)
        .unwrap_or_else(|error| panic!("could not read memory-probe frame: {error:?}"));
    assert_eq!(observed, PROBE_BYTES, "bootstrap RAM readback mismatch");

    ProcessImageMemory::clear_frame(&mut physical_memory, frame)
        .unwrap_or_else(|error| panic!("could not scrub memory-probe frame: {error:?}"));
    allocator
        .deallocate(frame)
        .unwrap_or_else(|error| panic!("could not release memory-probe frame: {error:?}"));
    assert_eq!(
        allocator.total_free_frames(),
        expected_free_frames,
        "memory-probe frame was not restored"
    );

    let _ = writeln!(
        console,
        "memory: free-frames={} probe-frame={:#018x}",
        expected_free_frames,
        frame.start_address().as_usize()
    );
    let _ = writeln!(console, "{MEMORY_SUCCESS_SENTINEL}");
    console.sink_mut().flush();
}

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
fn run_kernel_map_probe(boot_argument: usize, console: &mut Console<EarlyPl011>) -> ! {
    // SAFETY: the single boot CPU still uses the documented coarse higher-half
    // RAM alias. Allocated table frames remain private until publication.
    let mut physical_memory = unsafe { BootstrapPhysicalMemory::assume_bootstrap_mapping() };
    let device_tree_start = PhysAddr::new(boot_argument);
    let (plan, map) = {
        let tree = physical_memory
            .device_tree(device_tree_start)
            .unwrap_or_else(|error| panic!("could not borrow boot DTB: {error:?}"));
        let info = tree
            .boot_info()
            .unwrap_or_else(|error| panic!("could not extract boot information: {error:?}"));
        let kernel_image = kernel_physical_range()
            .unwrap_or_else(|error| panic!("could not derive kernel image range: {error:?}"));
        let plan = final_kernel_page_table_plan(info)
            .unwrap_or_else(|error| panic!("could not plan final kernel page tables: {error:?}"));
        let map = memory_map_from_boot_info::<KERNEL_MAP_MEMORY_RANGES>(
            info,
            kernel_image,
            device_tree_start,
        )
        .unwrap_or_else(|error| panic!("could not construct boot memory map: {error:?}"));
        (plan, map)
    };

    let mapping_count = plan.mappings().len();
    let table_count = plan.required_table_frames();
    let mut allocator = map.into_allocator();
    let allocated = plan
        .allocate(&mut allocator)
        .unwrap_or_else(|error| panic!("could not allocate final kernel tables: {error:?}"));
    let materialized = allocated
        .materialize(&mut physical_memory)
        .unwrap_or_else(|error| {
            panic!(
                "could not materialize final kernel table {:?} entry {:?} at {:?}: {:?}",
                error.table(),
                error.index(),
                error.stage(),
                error.error()
            )
        });
    drop(physical_memory);
    let root = materialized.root_frame().start_address().as_usize();
    // SAFETY: the probe initializes this single global owner exactly once on
    // the boot CPU and never replaces or frees it afterward.
    let tables = unsafe {
        install_kernel_map_runtime(KernelMapRuntime {
            tables: materialized,
            _allocator: allocator,
        })
    };

    let _ = writeln!(
        console,
        "kernel-map: root={root:#018x} tables={table_count} leaves={mapping_count}"
    );
    let _ = writeln!(console, "{KERNEL_MAP_ENTER_SENTINEL}");
    console.sink_mut().flush();

    // SAFETY: the validated plan maps the running kernel image with exact
    // text/rodata/data permissions, preserves the live boot stack and vector
    // table, maps all pinned QEMU RAM, and provides the PL011 device page. The
    // static runtime permanently retains the hierarchy on the single boot CPU.
    unsafe {
        tables.activate();
    }

    let _ = writeln!(console, "{KERNEL_MAP_SUCCESS_SENTINEL}");
    console.sink_mut().flush();
    halt()
}

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
unsafe fn install_kernel_map_runtime(runtime: KernelMapRuntime) -> &'static FinalKernelTables {
    let slot = &raw mut KERNEL_MAP_RUNTIME;
    // SAFETY: the caller guarantees one-time initialization and permanent
    // retention. The raw pointer avoids a reference to uninitialized storage.
    unsafe {
        (*slot).write(runtime);
        let runtime = (*slot).as_ptr();
        &*core::ptr::addr_of!((*runtime).tables)
    }
}

#[cfg(feature = "loaded-init-probe")]
fn run_loaded_init_probe(boot_argument: usize, console: &mut Console<EarlyPl011>) -> ! {
    let initramfs = Initramfs::from_bytes(INITRAMFS)
        .unwrap_or_else(|error| panic!("embedded initramfs is invalid: {error:?}"));
    let init_elf = initramfs
        .executable_file("init")
        .unwrap_or_else(|error| panic!("initramfs does not provide executable 'init': {error:?}"));
    let image = ElfImage::parse(init_elf)
        .unwrap_or_else(|error| panic!("embedded init ELF is invalid: {error:?}"));
    let stack_top = UserAddr::new(DEFAULT_USER_STACK_TOP)
        .unwrap_or_else(|error| panic!("default user stack top is invalid: {error:?}"));
    let stack = UserStackLayout::new(stack_top, 4, 1)
        .unwrap_or_else(|error| panic!("could not plan init stack: {error:?}"));
    let initial_stack = InitialStackImage::<INIT_STACK_BYTES>::new(
        stack,
        image.entry(),
        &["/init", "phoenix"],
        &["TERM=phoenix"],
    )
    .unwrap_or_else(|error| panic!("could not construct init stack: {error:?}"));
    let plan = ProcessImagePlan::<INIT_IMAGE_MAPPINGS, INIT_IMAGE_PAGES>::with_initial_stack(
        image,
        &initial_stack,
    )
    .unwrap_or_else(|error| panic!("could not plan init process image: {error:?}"));

    // SAFETY: this is the single-CPU bootstrap path while the documented
    // TTBR1 RAM alias is active. Every frame handed to this backend is owned
    // by the allocator-derived image or table type state below.
    let mut physical_memory = unsafe { BootstrapPhysicalMemory::assume_bootstrap_mapping() };
    let device_tree_start = PhysAddr::new(boot_argument);
    let (kernel_plan, map) = {
        let tree = physical_memory
            .device_tree(device_tree_start)
            .unwrap_or_else(|error| panic!("could not borrow boot DTB: {error:?}"));
        let info = tree
            .boot_info()
            .unwrap_or_else(|error| panic!("could not extract boot information: {error:?}"));
        let kernel_image = kernel_physical_range()
            .unwrap_or_else(|error| panic!("could not derive kernel image range: {error:?}"));
        let kernel_plan = final_kernel_page_table_plan(info)
            .unwrap_or_else(|error| panic!("could not plan final kernel page tables: {error:?}"));
        let map =
            memory_map_from_boot_info::<INIT_MEMORY_RANGES>(info, kernel_image, device_tree_start)
                .unwrap_or_else(|error| panic!("could not construct boot memory map: {error:?}"));
        (kernel_plan, map)
    };
    let mut allocator = map.into_allocator();
    let allocated = plan
        .allocate(&mut allocator)
        .unwrap_or_else(|error| panic!("could not allocate init image: {error:?}"));
    let populated = allocated
        .populate(&mut physical_memory)
        .unwrap_or_else(|error| {
            panic!(
                "could not populate init page {} at {:?}: {:?}",
                error.page_index(),
                error.stage(),
                error.error()
            )
        });

    let mut table_plan = UserPageTablePlan::<INIT_TABLE_PAGES, INIT_TABLE_LEAVES>::new()
        .unwrap_or_else(|error| panic!("could not start init table plan: {error:?}"));
    table_plan
        .map_process_image(&populated)
        .unwrap_or_else(|error| panic!("could not map init image: {error:?}"));
    let allocated_tables = table_plan
        .allocate(&mut allocator)
        .unwrap_or_else(|error| panic!("could not allocate init page tables: {error:?}"));
    let materialized = allocated_tables
        .materialize(&mut physical_memory)
        .unwrap_or_else(|error| {
            panic!(
                "could not materialize init table {:?} entry {:?} at {:?}: {:?}",
                error.table(),
                error.index(),
                error.stage(),
                error.error()
            )
        });
    let prepared = PreparedUserAddressSpace::new(populated, materialized).unwrap_or_else(|error| {
        panic!(
            "could not combine init address-space ownership: {:?}",
            error.reason()
        )
    });
    let kernel_mapping_count = kernel_plan.mappings().len();
    let kernel_table_count = kernel_plan.required_table_frames();
    let allocated_kernel_tables = kernel_plan
        .allocate(&mut allocator)
        .unwrap_or_else(|error| panic!("could not allocate final kernel tables: {error:?}"));
    let kernel_tables = allocated_kernel_tables
        .materialize(&mut physical_memory)
        .unwrap_or_else(|error| {
            panic!(
                "could not materialize final kernel table {:?} entry {:?} at {:?}: {:?}",
                error.table(),
                error.index(),
                error.stage(),
                error.error()
            )
        });
    drop(physical_memory);
    let kernel_root = kernel_tables.root_frame().start_address().as_usize();
    // SAFETY: this single-CPU boot path initializes the slot exactly once
    // before EL0 can issue an exception. The slot is never replaced or freed.
    let (prepared, kernel_tables) = unsafe {
        install_init_runtime(InitRuntime {
            address_space: prepared,
            kernel_tables,
            files: ReadOnlyFileTable::new(initramfs),
            _allocator: allocator,
        })
    };

    let _ = writeln!(
        console,
        "init-kernel-map: root={kernel_root:#018x} tables={kernel_table_count} leaves={kernel_mapping_count}"
    );
    let _ = writeln!(console, "{INIT_KERNEL_MAP_ENTER_SENTINEL}");
    console.sink_mut().flush();
    // SAFETY: the retained final hierarchy covers the current image, stack,
    // vectors, direct-mapped RAM, and PL011. The bootstrap handle was dropped,
    // and this remains a masked, single-core boot path.
    unsafe {
        kernel_tables.activate();
    }
    let _ = writeln!(console, "{INIT_KERNEL_MAP_SENTINEL}");

    let _ = writeln!(
        console,
        "init: entry={:#018x} stack={:#018x} pages={} root={:#018x}",
        prepared.entry().as_usize(),
        prepared.stack_pointer().as_usize(),
        prepared.resident_page_count(),
        prepared.root_frame().start_address().as_usize()
    );
    let _ = writeln!(console, "{INIT_ENTER_SENTINEL}");
    console.sink_mut().flush();

    // SAFETY: the runtime permanently owns every fully initialized leaf and
    // table frame. The bootstrap keeps caches disabled, vectors and the EL1
    // stack are active, and this single-core probe is the only user of ASID
    // zero.
    unsafe {
        prepared.activate_and_enter();
    }
}

#[cfg(feature = "loaded-init-probe")]
unsafe fn install_init_runtime(
    runtime: InitRuntime,
) -> (&'static InitAddressSpace, &'static FinalKernelTables) {
    let slot = &raw mut INIT_RUNTIME;
    // SAFETY: the caller guarantees unique one-time initialization and eternal
    // retention; raw pointers avoid manufacturing a reference before init.
    unsafe {
        (*slot).write(runtime);
        let runtime = (*slot).as_ptr();
        (
            &*core::ptr::addr_of!((*runtime).address_space),
            &*core::ptr::addr_of!((*runtime).kernel_tables),
        )
    }
}

#[cfg(feature = "loaded-init-probe")]
unsafe fn active_init_address_space() -> &'static InitAddressSpace {
    let slot = &raw const INIT_RUNTIME;
    // SAFETY: only lower-EL SVC dispatch calls this function, and EL0 entry is
    // possible only after `install_init_runtime` completed. The address-space
    // field is never subsequently mutated or freed.
    unsafe {
        let runtime = (*slot).as_ptr();
        &*core::ptr::addr_of!((*runtime).address_space)
    }
}

#[cfg(feature = "loaded-init-probe")]
unsafe fn active_init_files() -> &'static mut ReadOnlyFileTable<'static, INIT_OPEN_FILES> {
    let slot = &raw mut INIT_RUNTIME;
    // SAFETY: lower-EL syscall dispatch is single-core and non-reentrant in
    // this probe. Initialization precedes EL0 entry. This returns only the
    // file-table field, disjoint from the permanently shared address space,
    // and each mutable borrow ends before another syscall can begin.
    unsafe {
        let runtime = (*slot).as_mut_ptr();
        &mut *core::ptr::addr_of_mut!((*runtime).files)
    }
}

/// Common target of the assembly exception vectors.
///
/// # Safety
///
/// `frame` must point to the uniquely borrowed, fully initialized frame at the
/// current exception stack pointer, and `slot_index` must identify its vector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phoenix_exception_dispatch(frame: *mut ExceptionFrame, slot_index: usize) {
    // SAFETY: `vectors.S` allocates one aligned `ExceptionFrame`, initializes
    // every field, and passes its unique mutable address for this call.
    let frame = unsafe { &mut *frame };
    let slot = VectorSlot::ALL.get(slot_index).copied();

    #[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
    if handle_probe_syscall(frame, slot) {
        return;
    }

    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{EXCEPTION_SENTINEL}");
    let _ = writeln!(console, "vector: {slot:?}");
    let _ = writeln!(console, "pc: {:#018x}", frame.program_counter());
    let _ = writeln!(console, "spsr: {:#018x}", frame.saved_program_status());
    let _ = writeln!(console, "esr: {:#018x}", frame.syndrome().raw());
    let _ = writeln!(console, "far: {:#018x}", frame.fault_address());
    console.sink_mut().flush();
    halt()
}

#[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
fn handle_probe_syscall(frame: &mut ExceptionFrame, slot: Option<VectorSlot>) -> bool {
    let expected_slot = VectorSlot::new(VectorOrigin::LowerAArch64, ExceptionKind::Synchronous);
    let syndrome = frame.syndrome();
    if slot != Some(expected_slot)
        || syndrome.exception_class() != ExceptionClass::SupervisorCallAArch64
        || syndrome.supervisor_call_immediate() != Some(NATIVE_SVC_IMMEDIATE)
    {
        return false;
    }

    let request = syscall::request_from_frame(frame);
    match request.operation() {
        NativeSyscall::Exit => {
            let status = request.argument(0).expect("exit status is in x0");
            let mut console = Console::new(EarlyPl011::new());
            #[cfg(feature = "loaded-init-probe")]
            let _ = if status == 42 && INIT_WROTE_OUTPUT.load(Ordering::Acquire) {
                writeln!(console, "{INIT_SUCCESS_SENTINEL}")
            } else {
                writeln!(
                    console,
                    "{INIT_FAILURE_SENTINEL}: status={status} wrote-output={}",
                    INIT_WROTE_OUTPUT.load(Ordering::Relaxed)
                )
            };
            #[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
            let _ = if status == 42 {
                writeln!(console, "{EL0_SUCCESS_SENTINEL}")
            } else {
                writeln!(console, "{EL0_FAILURE_SENTINEL}: status={status}")
            };
            console.sink_mut().flush();
            halt()
        }
        NativeSyscall::Write => {
            #[cfg(feature = "loaded-init-probe")]
            handle_init_write(frame, request);
            #[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
            syscall::set_return(frame, SyscallReturn::error(Errno::NotImplemented));
            true
        }
        NativeSyscall::Read => {
            #[cfg(feature = "loaded-init-probe")]
            handle_init_read(frame, request);
            #[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
            syscall::set_return(frame, SyscallReturn::error(Errno::NotImplemented));
            true
        }
        NativeSyscall::Open => {
            #[cfg(feature = "loaded-init-probe")]
            handle_init_open(frame, request);
            #[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
            syscall::set_return(frame, SyscallReturn::error(Errno::NotImplemented));
            true
        }
        NativeSyscall::Close => {
            #[cfg(feature = "loaded-init-probe")]
            handle_init_close(frame, request);
            #[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
            syscall::set_return(frame, SyscallReturn::error(Errno::NotImplemented));
            true
        }
        NativeSyscall::Unknown(_) => {
            syscall::set_return(frame, SyscallReturn::error(Errno::NotImplemented));
            true
        }
    }
}

#[cfg(feature = "loaded-init-probe")]
fn handle_init_write(frame: &mut ExceptionFrame, request: phoenix_kernel::abi::SyscallRequest) {
    let write = match ConsoleWriteRequest::from_syscall(request, INIT_WRITE_LIMIT) {
        Ok(write) => write,
        Err(error) => {
            syscall::set_return(frame, SyscallReturn::error(error));
            return;
        }
    };
    let mut buffer = [0_u8; INIT_WRITE_LIMIT];
    // SAFETY: lower-EL dispatch is reachable only after the one-time runtime
    // installation, which retains this immutable address-space field forever.
    let address_space = unsafe { active_init_address_space() };
    let mut memory = ActiveInitMemory::new(address_space);
    let result = address_space.copy_from_user(
        &mut memory,
        write.user_buffer(),
        &mut buffer[..write.length()],
    );
    if result.is_err() {
        syscall::set_return(frame, SyscallReturn::error(Errno::BadAddress));
        return;
    }

    let mut console = EarlyPl011::new();
    for byte in &buffer[..write.length()] {
        console.write_byte(*byte);
    }
    console.flush();
    if write.length() != 0 {
        INIT_WROTE_OUTPUT.store(true, Ordering::Release);
    }
    syscall::set_return(
        frame,
        SyscallReturn::success(write.length() as u64).expect("bounded write length is successful"),
    );
}

#[cfg(feature = "loaded-init-probe")]
fn handle_init_open(frame: &mut ExceptionFrame, request: phoenix_kernel::abi::SyscallRequest) {
    let open = match FileOpenRequest::from_syscall(request, INIT_PATH_LIMIT) {
        Ok(open) => open,
        Err(error) => {
            syscall::set_return(frame, SyscallReturn::error(error));
            return;
        }
    };
    let mut path_bytes = [0_u8; INIT_PATH_LIMIT];
    // SAFETY: syscall dispatch starts only after the one-time runtime owner is
    // installed and retained permanently.
    let address_space = unsafe { active_init_address_space() };
    let mut memory = ActiveInitMemory::new(address_space);
    let copied = address_space.copy_from_user(
        &mut memory,
        open.path_buffer(),
        &mut path_bytes[..open.path_length()],
    );
    if copied.is_err() {
        syscall::set_return(frame, SyscallReturn::error(Errno::BadAddress));
        return;
    }
    let Ok(path) = core::str::from_utf8(&path_bytes[..open.path_length()]) else {
        syscall::set_return(frame, SyscallReturn::error(Errno::InvalidArgument));
        return;
    };
    // SAFETY: this synchronous single-core dispatcher has the only live
    // mutable borrow of the process-local file table.
    let opened = unsafe { active_init_files().open(path) };
    match opened {
        Ok(descriptor) => syscall::set_return(
            frame,
            SyscallReturn::success(descriptor).expect("small descriptor is a success value"),
        ),
        Err(error) => syscall::set_return(frame, SyscallReturn::error(file_errno(error))),
    }
}

#[cfg(feature = "loaded-init-probe")]
fn handle_init_read(frame: &mut ExceptionFrame, request: phoenix_kernel::abi::SyscallRequest) {
    let read = match FileReadRequest::from_syscall(request, INIT_READ_LIMIT) {
        Ok(read) => read,
        Err(error) => {
            syscall::set_return(frame, SyscallReturn::error(error));
            return;
        }
    };
    // SAFETY: this synchronous single-core dispatcher has the only live
    // mutable borrow of the process-local file table. The returned window
    // borrows immutable initramfs storage rather than the table itself.
    let window = match unsafe { active_init_files().read_window(read.descriptor(), read.length()) }
    {
        Ok(window) => window,
        Err(error) => {
            syscall::set_return(frame, SyscallReturn::error(file_errno(error)));
            return;
        }
    };
    // SAFETY: runtime publication precedes dispatch and permanently retains
    // the active user-frame owner.
    let address_space = unsafe { active_init_address_space() };
    let mut memory = ActiveInitMemory::new(address_space);
    let copied = address_space.copy_to_user(&mut memory, read.user_buffer(), window.bytes());
    if copied.is_err() {
        syscall::set_return(frame, SyscallReturn::error(Errno::BadAddress));
        return;
    }
    // SAFETY: no other execution can mutate this single-core probe table
    // between obtaining and committing the window.
    if let Err(error) = unsafe { active_init_files().commit_read(window) } {
        syscall::set_return(frame, SyscallReturn::error(file_errno(error)));
        return;
    }
    syscall::set_return(
        frame,
        SyscallReturn::success(window.bytes().len() as u64)
            .expect("bounded read length is a success value"),
    );
}

#[cfg(feature = "loaded-init-probe")]
fn handle_init_close(frame: &mut ExceptionFrame, request: phoenix_kernel::abi::SyscallRequest) {
    let close = match FileCloseRequest::from_syscall(request) {
        Ok(close) => close,
        Err(error) => {
            syscall::set_return(frame, SyscallReturn::error(error));
            return;
        }
    };
    // SAFETY: this synchronous single-core dispatcher has exclusive access to
    // the process-local table for the duration of the close operation.
    match unsafe { active_init_files().close(close.descriptor()) } {
        Ok(()) => syscall::set_return(
            frame,
            SyscallReturn::success(0).expect("zero is a success value"),
        ),
        Err(error) => syscall::set_return(frame, SyscallReturn::error(file_errno(error))),
    }
}

#[cfg(feature = "loaded-init-probe")]
const fn file_errno(error: FileError) -> Errno {
    match error {
        FileError::InvalidPath(_) => Errno::InvalidArgument,
        FileError::NotFound => Errno::NoSuchFile,
        FileError::IsDirectory => Errno::IsDirectory,
        FileError::PermissionDenied => Errno::PermissionDenied,
        FileError::TooManyOpenFiles | FileError::GenerationExhausted => Errno::TooManyOpenFiles,
        FileError::BadDescriptor => Errno::BadFileDescriptor,
        FileError::StaleRead => Errno::InputOutput,
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{PANIC_SENTINEL}");
    let _ = writeln!(console, "{info}");
    console.sink_mut().flush();
    halt()
}
