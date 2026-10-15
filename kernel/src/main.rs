#![no_main]
#![no_std]

#[cfg(not(target_arch = "aarch64"))]
compile_error!("the Phoenix kernel binary currently supports only AArch64");

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
mod kernel_threads;
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
mod runtime_probe;

#[cfg(feature = "loaded-init-probe")]
mod system;

use core::fmt::Write;
use core::panic::PanicInfo;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use core::{convert::Infallible, mem::MaybeUninit, num::NonZeroU32, pin::Pin};

use phoenix_kernel::arch::aarch64::exception::{ExceptionFrame, VectorSlot};
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use phoenix_kernel::arch::aarch64::{
    LocalIrq,
    gicv3::{GicV3Configuration, GicV3MmioError, MappedGicV3Registers, prepare},
    interrupt_runtime::{
        InterruptDispatch, InterruptRuntime, InterruptRuntimeError, InterruptRuntimePhase,
        PublishedInterruptRuntime,
    },
    irqs_masked,
    timer::PhysicalTimer,
};
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
use phoenix_kernel::timer_service::{
    LockedTimerQueue, OneShotTimerConsumer, OneShotTimerConsumerError,
};

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
use phoenix_kernel::arch::aarch64::kernel_page_table::{
    Aarch64KernelLeafTlb, Aarch64KernelStackMapper,
};
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use phoenix_kernel::memory::FrameAllocator;
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
use phoenix_kernel::memory::{
    HeapAllocationError, KernelHeap, KernelStackKind, KernelStackMemory, PAGE_SIZE,
};
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
    platform::aarch64::qemu_virt::{
        BootGicV3Discovery, FINAL_KERNEL_TABLE_CAPACITY, FinalDirectMapLayout,
        FinalDirectMapMemory, QemuVirtKernelStackPool, discover_boot_gicv3,
        final_direct_map_layout, final_kernel_page_table_plan, runtime_kernel_stack_pool,
    },
};

#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::abi::{
    ConsoleWriteRequest, FileCloseRequest, FileOpenRequest, FileReadRequest,
};
#[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
use phoenix_kernel::abi::{Errno, NATIVE_SVC_IMMEDIATE, NativeSyscall, SyscallReturn};
#[cfg(feature = "loaded-init-probe")]
use phoenix_kernel::arch::aarch64::asid::{AddressSpaceId, AsidAllocator};
#[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
use phoenix_kernel::arch::aarch64::el0;
#[cfg(any(
    feature = "loaded-init-probe",
    feature = "el0-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
use phoenix_kernel::arch::aarch64::exception::ExceptionKind;
#[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
use phoenix_kernel::arch::aarch64::exception::{ExceptionClass, VectorOrigin, classify_user_fault};
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
use phoenix_kernel::process::{ExitStatus, ProcessControl, ProcessId, UserFault};
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
mod events;
#[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
const USER_FAULT_SENTINEL: &str = "PHOENIX_USER_FAULT";
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
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
const KERNEL_HEAP_SUCCESS_SENTINEL: &str = "PHOENIX_KERNEL_HEAP_OK";
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
const KERNEL_STACK_SUCCESS_SENTINEL: &str = "PHOENIX_KERNEL_STACK_OK";
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
const GIC_PREPARED_SENTINEL: &str = "PHOENIX_GIC_PREPARED";
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
const IRQ_RUNTIME_PUBLISHED_SENTINEL: &str = "PHOENIX_IRQ_RUNTIME_PUBLISHED";
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
    process: ProcessControl,
    _init_asid: AddressSpaceId,
    address_space: InitAddressSpace,
    kernel_tables: FinalKernelTables,
    direct_map_layout: FinalDirectMapLayout<INIT_MEMORY_RANGES>,
    gic: BootGicV3Discovery,
    _kernel_stack_pool: QemuVirtKernelStackPool,
    files: ReadOnlyFileTable<'static, INIT_OPEN_FILES>,
    _allocator: FrameAllocator<INIT_MEMORY_RANGES>,
    _asids: AsidAllocator,
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
    direct_map: FinalDirectMapMemory<'static, FINAL_KERNEL_TABLE_CAPACITY, INIT_MEMORY_RANGES>,
}
#[cfg(feature = "loaded-init-probe")]
impl ActiveInitMemory {
    fn new(
        address_space: &'static InitAddressSpace,
        direct_map: FinalDirectMapMemory<'static, FINAL_KERNEL_TABLE_CAPACITY, INIT_MEMORY_RANGES>,
    ) -> Self {
        Self {
            address_space,
            direct_map,
        }
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
        self.direct_map
            .read_frame(frame, offset, output)
            .map_err(|_error| ActiveInitMemoryError::Platform)
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
        UserMemoryWriter::write_frame(&mut self.direct_map, frame, offset, input)
            .map_err(|_error| ActiveInitMemoryError::Platform)
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
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
const KERNEL_HEAP_BACKING_FRAMES: usize = 4;
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
const KERNEL_HEAP_ALLOCATIONS: usize = 4;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
type FinalKernelTables = MaterializedKernelPageTables<FINAL_KERNEL_TABLE_CAPACITY>;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
type BootGicV3Registers = MappedGicV3Registers<'static, FINAL_KERNEL_TABLE_CAPACITY>;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
const GIC_PREPARATION_POLL_BUDGET: u32 = 100_000;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
const BOOT_TIMER_CAPACITY: usize = 1;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
type BootInterruptRuntime =
    InterruptRuntime<BootGicV3Registers, BOOT_TIMER_CAPACITY, PhysicalTimer>;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
type BootInterruptHandle =
    PublishedInterruptRuntime<'static, BootGicV3Registers, BOOT_TIMER_CAPACITY, PhysicalTimer>;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
type BootInterruptError = InterruptRuntimeError<GicV3MmioError, Infallible>;
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
static mut BOOT_INTERRUPT_RUNTIME: MaybeUninit<BootInterruptRuntime> = MaybeUninit::uninit();
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
static mut BOOT_INTERRUPT_HANDLE: MaybeUninit<BootInterruptHandle> = MaybeUninit::uninit();
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
static BOOT_INTERRUPT_PUBLISHED: AtomicBool = AtomicBool::new(false);
#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
static BOOT_TIMER_CONSUMER: OneShotTimerConsumer = OneShotTimerConsumer::new();

#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
enum BootIrqFailure {
    Unpublished,
    Runtime(BootInterruptError),
    Unexpected { intid: u32 },
    TimerConsumer(OneShotTimerConsumerError),
}
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
struct KernelMapRuntime {
    tables: FinalKernelTables,
    direct_map_layout: FinalDirectMapLayout<KERNEL_MAP_MEMORY_RANGES>,
    gic: BootGicV3Discovery,
    allocator: FrameAllocator<KERNEL_MAP_MEMORY_RANGES>,
    heap: KernelHeap<KERNEL_HEAP_ALLOCATIONS>,
    kernel_stack_pool: QemuVirtKernelStackPool,
}
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
struct KernelMapRuntimeAccess {
    tables: &'static FinalKernelTables,
    direct_map_layout: FinalDirectMapLayout<KERNEL_MAP_MEMORY_RANGES>,
    gic: BootGicV3Discovery,
    allocator: &'static mut FrameAllocator<KERNEL_MAP_MEMORY_RANGES>,
    heap: &'static mut KernelHeap<KERNEL_HEAP_ALLOCATIONS>,
    kernel_stack_pool: &'static mut QemuVirtKernelStackPool,
}
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
static mut KERNEL_MAP_RUNTIME: MaybeUninit<KernelMapRuntime> = MaybeUninit::uninit();

/// Rust entry point reached by the AArch64 bootstrap.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_argument: usize, entry_exception_level: usize) -> ! {
    // SAFETY: This is the first Rust operation after assembly zeroed BSS; the
    // sole CPU has IRQs masked and no BSS singleton has been accessed yet.
    unsafe { phoenix_kernel::arch::aarch64::diagnostics::validate_boot_bss() };
    // SAFETY: bootstrap maps the complete linked image as executable normal
    // memory, provides an aligned EL1 stack, and keeps exceptions masked.
    unsafe {
        install_exception_vectors();
    }
    events::record(1, entry_exception_level as u64 >> 2, boot_argument as u64);
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{}", BuildInfo::CURRENT);
    let _ = writeln!(
        console,
        "entry exception level: EL{}",
        entry_exception_level >> 2
    );
    let _ = writeln!(console, "boot argument: {boot_argument:#018x}");
    let _ = writeln!(console, "{BOOT_SUCCESS_SENTINEL}");
    console.sink_mut().flush();

    #[cfg(all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    ))]
    runtime_probe::select(boot_argument);

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

    // Own every reservation-filtered range without touching reserved pages.
    // Probe both endpoints, observe explicit exhaustion, then restore all ranges.
    let mut held = [None; MEMORY_MAP_CAPACITY];
    let mut ranges = 0;
    while let Some(range) = allocator.free_ranges().first().copied() {
        let owned = allocator
            .allocate_contiguous(range.len(), 1)
            .unwrap()
            .unwrap();
        for number in [owned.start_frame_number(), owned.end_frame_number() - 1] {
            let endpoint =
                phoenix_kernel::memory::PageFrame::from_start(PhysAddr::new(number * 4096))
                    .unwrap();
            ProcessImageMemory::clear_frame(&mut physical_memory, endpoint).unwrap();
            ProcessImageMemory::write_frame(
                &mut physical_memory,
                endpoint,
                4096 - PROBE_BYTES.len(),
                PROBE_BYTES,
            )
            .unwrap();
            physical_memory
                .read_frame(endpoint, 4096 - PROBE_BYTES.len(), &mut observed)
                .unwrap();
            assert_eq!(observed, PROBE_BYTES);
            ProcessImageMemory::clear_frame(&mut physical_memory, endpoint).unwrap();
        }
        held[ranges] = Some(owned);
        ranges += 1;
    }
    assert!(allocator.allocate().unwrap().is_none());
    for owned in held.into_iter().flatten() {
        allocator.deallocate_contiguous(owned).unwrap();
    }
    assert_eq!(allocator.total_free_frames(), expected_free_frames);
    for address in [0x3fff_f000, 0x8000_0000] {
        let outside =
            phoenix_kernel::memory::PageFrame::from_start(PhysAddr::new(address)).unwrap();
        assert!(
            physical_memory
                .read_frame(outside, 0, &mut observed)
                .is_err()
        );
    }
    let _ = writeln!(
        console,
        "memory-boundaries: ranges={ranges} endpoints=ok exhaustion=ok restored=ok outside=rejected"
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
    let (plan, map, gic) = {
        let tree = physical_memory
            .device_tree(device_tree_start)
            .unwrap_or_else(|error| panic!("could not borrow boot DTB: {error:?}"));
        let info = tree
            .boot_info()
            .unwrap_or_else(|error| panic!("could not extract boot information: {error:?}"));
        // SAFETY: the single boot CPU still owns the bootstrap low device
        // identity map, IRQs remain masked, and no GIC consumer exists yet.
        let gic = unsafe { discover_boot_gicv3(tree) }
            .unwrap_or_else(|error| panic!("could not discover boot GICv3: {error:?}"));
        let kernel_image = kernel_physical_range()
            .unwrap_or_else(|error| panic!("could not derive kernel image range: {error:?}"));
        let plan = final_kernel_page_table_plan(info, gic.layout())
            .unwrap_or_else(|error| panic!("could not plan final kernel page tables: {error:?}"));
        let map = memory_map_from_boot_info::<KERNEL_MAP_MEMORY_RANGES>(
            info,
            kernel_image,
            device_tree_start,
        )
        .unwrap_or_else(|error| panic!("could not construct boot memory map: {error:?}"));
        (plan, map, gic)
    };

    let mapping_count = plan.mappings().len();
    let table_count = plan.required_table_frames();
    let mut allocator = map.into_allocator();
    let direct_map_layout = final_direct_map_layout(&plan, allocator.managed_frames())
        .unwrap_or_else(|error| panic!("could not prove final RAM direct map: {error:?}"));
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
    let runtime = unsafe {
        install_kernel_map_runtime(KernelMapRuntime {
            tables: materialized,
            direct_map_layout,
            gic,
            allocator,
            heap: KernelHeap::new(),
            kernel_stack_pool: runtime_kernel_stack_pool()
                .unwrap_or_else(|error| panic!("could not reserve kernel-stack arena: {error:?}")),
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
    // table, maps all pinned QEMU RAM, and provides exact PL011, GICD, and
    // selected boot-GICR device pages. The static runtime permanently retains
    // the hierarchy and GIC layout on the single boot CPU.
    unsafe {
        runtime.tables.activate();
    }

    // SAFETY: the one-time runtime permanently owns the active table hierarchy
    // and copied GIC discovery evidence. IRQs remain masked on the only CPU,
    // and no other GIC register owner can access the closed delivery state.
    let gic_configuration =
        unsafe { prepare_and_publish_boot_interrupt_runtime(runtime.tables, runtime.gic) };
    let _ = writeln!(
        console,
        "gicv3: timer-intid={} poll-budget={} delivery=closed",
        gic_configuration.timer().intid(),
        gic_configuration.poll_budget(),
    );
    let _ = writeln!(console, "{GIC_PREPARED_SENTINEL}");
    let _ = writeln!(console, "{IRQ_RUNTIME_PUBLISHED_SENTINEL}");
    console.sink_mut().flush();

    initialize_and_exercise_kernel_heap(
        runtime.tables,
        runtime.direct_map_layout,
        runtime.allocator,
        runtime.heap,
        console,
    );
    exercise_runtime_kernel_stack(
        runtime.tables,
        runtime.direct_map_layout,
        runtime.allocator,
        runtime.kernel_stack_pool,
        console,
    );

    if runtime_probe::threads_selected() {
        kernel_threads::run(
            runtime.tables,
            runtime.direct_map_layout,
            runtime.allocator,
            runtime.kernel_stack_pool,
            console,
        );
    }
    runtime_probe::after_map(console);

    let _ = writeln!(console, "{KERNEL_MAP_SUCCESS_SENTINEL}");
    console.sink_mut().flush();
    halt()
}

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
unsafe fn install_kernel_map_runtime(runtime: KernelMapRuntime) -> KernelMapRuntimeAccess {
    let slot = &raw mut KERNEL_MAP_RUNTIME;
    // SAFETY: the caller guarantees one-time initialization and permanent
    // retention without any competing reference. The returned shared table
    // reference and mutable allocator/heap/pool references point to disjoint
    // fields, and the copied layout does not borrow the owner. Raw pointers
    // avoid a reference to uninitialized storage.
    unsafe {
        (*slot).write(runtime);
        let runtime = (*slot).as_mut_ptr();
        KernelMapRuntimeAccess {
            tables: &*core::ptr::addr_of!((*runtime).tables),
            direct_map_layout: *core::ptr::addr_of!((*runtime).direct_map_layout),
            gic: *core::ptr::addr_of!((*runtime).gic),
            allocator: &mut *core::ptr::addr_of_mut!((*runtime).allocator),
            heap: &mut *core::ptr::addr_of_mut!((*runtime).heap),
            kernel_stack_pool: &mut *core::ptr::addr_of_mut!((*runtime).kernel_stack_pool),
        }
    }
}

#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
unsafe fn prepare_and_publish_boot_interrupt_runtime(
    tables: &'static FinalKernelTables,
    discovery: BootGicV3Discovery,
) -> GicV3Configuration {
    assert!(
        irqs_masked(),
        "boot GICv3 preparation requires DAIF.I to remain masked"
    );
    let poll_budget = NonZeroU32::new(GIC_PREPARATION_POLL_BUDGET)
        .expect("the fixed GICv3 poll budget is nonzero");
    let configuration = GicV3Configuration::new(
        discovery.topology().timers().non_secure_physical(),
        poll_budget,
    );
    // SAFETY: `tables` is the permanently retained active TTBR1 owner, its
    // validated plan maps both exact ranges as Device-nGnRnE, the copied
    // discovery selected this CPU's GICR, IRQs are checked masked above, and
    // the one-time boot path grants this backend unique GIC register access.
    let backend =
        unsafe { MappedGicV3Registers::assume_active(tables, discovery.layout().registers()) };
    // SAFETY: the preceding construction establishes unique mapped register
    // ownership for the single boot CPU, DAIF.I is masked, and no interrupt
    // handler or competing CPU can access the controller during preparation.
    let prepared = match unsafe { prepare(backend, configuration) } {
        Ok(prepared) => prepared,
        Err(failure) => panic!(
            "could not prepare boot GICv3: primary={:?} recovery={:?}",
            failure.error(),
            failure.recovery_error()
        ),
    };

    // SAFETY: this one-time single-CPU boot path creates the only physical
    // timer owner after EL1 counter/timer access was enabled by bootstrap or
    // supplied by the direct-EL1 firmware contract.
    let mut physical_timer = unsafe { PhysicalTimer::claim() };
    physical_timer.disable();
    let initial_counter = physical_timer.counter();
    // SAFETY: the unique owner was just explicitly disabled and masked, and
    // `initial_counter` was read from that owner's fixed physical domain.
    let timer = unsafe { LockedTimerQueue::assume_disarmed(initial_counter, physical_timer) };
    let runtime = InterruptRuntime::new(prepared, timer, configuration.timer());

    let runtime_slot = &raw mut BOOT_INTERRUPT_RUNTIME;
    // SAFETY: each final-map probe reaches this path at most once. The static
    // slot never moves or deinitializes, so its non-Unpin runtime can be pinned
    // at this address for the remainder of boot.
    let pinned = unsafe {
        (*runtime_slot).write(runtime);
        Pin::new_unchecked(&*(*runtime_slot).as_ptr())
    };
    let local_irq = LocalIrq::new();
    // SAFETY: VBAR_EL1 and the common Rust dispatcher were installed before
    // final-map construction. The handler below uses the separate global
    // handle slot, which is initialized before any activation; DAIF.I and both
    // GIC delivery gates remain closed throughout this publication.
    let handle = match unsafe { pinned.publish(&local_irq) } {
        Ok(handle) => handle,
        Err(phase) => panic!("could not publish boot interrupt runtime from phase {phase:?}"),
    };
    assert_eq!(
        handle.phase(&local_irq),
        InterruptRuntimePhase::Prepared,
        "published boot interrupt runtime must keep delivery closed"
    );

    let handle_slot = &raw mut BOOT_INTERRUPT_HANDLE;
    // SAFETY: the same one-time path initializes the lookup exactly once. The
    // handle borrows the permanent runtime slot and is never moved afterward.
    unsafe {
        (*handle_slot).write(handle);
    }
    BOOT_INTERRUPT_PUBLISHED.store(true, Ordering::Release);
    configuration
}

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
struct GuardedHeapMemory<'a>(
    &'a mut FinalDirectMapMemory<'static, FINAL_KERNEL_TABLE_CAPACITY, KERNEL_MAP_MEMORY_RANGES>,
);
#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
impl phoenix_kernel::memory::HeapBackingMemory for GuardedHeapMemory<'_> {
    type Error = phoenix_kernel::platform::aarch64::qemu_virt::FinalDirectMapAccessError;
    fn virtual_start(
        &mut self,
        frames: phoenix_kernel::memory::FrameRange,
    ) -> Result<phoenix_kernel::memory::VirtAddr, Self::Error> {
        // Validate complete managed backing without publishing a hardware alias.
        phoenix_kernel::memory::HeapBackingMemory::virtual_start(self.0, frames)?;
        assert_eq!(frames.len(), KERNEL_HEAP_BACKING_FRAMES);
        Ok(phoenix_kernel::memory::VirtAddr::new(
            phoenix_kernel::platform::aarch64::qemu_virt::KERNEL_HEAP_START,
        ))
    }
    fn clear_frame(&mut self, frame: phoenix_kernel::memory::PageFrame) -> Result<(), Self::Error> {
        phoenix_kernel::memory::HeapBackingMemory::clear_frame(self.0, frame)
    }
}

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
fn initialize_and_exercise_kernel_heap(
    tables: &'static FinalKernelTables,
    direct_map_layout: FinalDirectMapLayout<KERNEL_MAP_MEMORY_RANGES>,
    allocator: &mut FrameAllocator<KERNEL_MAP_MEMORY_RANGES>,
    heap: &mut KernelHeap<KERNEL_HEAP_ALLOCATIONS>,
    console: &mut Console<EarlyPl011>,
) {
    let free_before = allocator.total_free_frames();
    // SAFETY: `tables` is the retained hierarchy materialized from
    // `direct_map_layout`'s proved plan and was activated immediately before
    // this call. The boot CPU is single-threaded with exceptions masked; the
    // allocator grants the heap unique ownership of every mutated frame.
    let mut direct_map = unsafe { FinalDirectMapMemory::assume_active(tables, direct_map_layout) };
    let mut guarded = GuardedHeapMemory(&mut direct_map);
    heap.initialize(allocator, &mut guarded, KERNEL_HEAP_BACKING_FRAMES, 1)
        .unwrap_or_else(|error| panic!("could not initialize kernel heap: {error:?}"));
    assert_eq!(
        allocator.total_free_frames(),
        free_before - KERNEL_HEAP_BACKING_FRAMES,
        "kernel heap did not retain its exact backing frames"
    );

    // SAFETY: The permanent KernelMapRuntime owns this initialized heap and
    // retains every backing frame for boot lifetime. No allocation escaped yet.
    unsafe {
        phoenix_kernel::arch::aarch64::kernel_page_table::publish_permanent_heap(
            tables,
            &mut direct_map,
            &mut Aarch64KernelLeafTlb,
            heap.backing_frames().unwrap(),
            phoenix_kernel::memory::VirtAddr::new(
                phoenix_kernel::platform::aarch64::qemu_virt::KERNEL_HEAP_START,
            ),
        )
    }
    .expect("guarded heap publication");
    let first = heap
        .allocate(64, 64)
        .unwrap_or_else(|error| panic!("could not allocate aligned heap probe: {error:?}"));
    assert_eq!(first.address().as_usize() % 64, 0);
    // SAFETY: The heap token uniquely owns 64 mapped bytes; guards are outside
    // this allocation, and no other CPU or IRQ accesses the heap.
    unsafe {
        let pointer = first.address().as_usize() as *mut u64;
        pointer.write_volatile(0x50484f454e4958);
        assert_eq!(pointer.read_volatile(), 0x50484f454e4958);
    }
    heap.deallocate(first)
        .unwrap_or_else(|error| panic!("could not release aligned heap probe: {error:?}"));
    let reused = heap
        .allocate(64, 64)
        .unwrap_or_else(|error| panic!("could not reuse released heap bytes: {error:?}"));
    assert_eq!(reused.address(), first.address());
    heap.deallocate(reused)
        .unwrap_or_else(|error| panic!("could not release reused heap probe: {error:?}"));

    assert_eq!(
        heap.allocate(heap.capacity() + 1, 1),
        Err(HeapAllocationError::OutOfMemory),
        "oversized heap request did not report byte exhaustion"
    );
    let mut metadata_probes = [None; KERNEL_HEAP_ALLOCATIONS];
    for allocation in &mut metadata_probes {
        *allocation = Some(
            heap.allocate(1, 1)
                .unwrap_or_else(|error| panic!("could not fill heap metadata: {error:?}")),
        );
    }
    assert_eq!(
        heap.allocate(1, 1),
        Err(HeapAllocationError::MetadataExhausted),
        "full heap metadata did not report exhaustion"
    );
    for allocation in metadata_probes.into_iter().flatten() {
        heap.deallocate(allocation)
            .unwrap_or_else(|error| panic!("could not release metadata probe: {error:?}"));
    }
    assert_eq!(heap.free_bytes(), heap.capacity());

    let backing = heap.backing_frames().expect("initialized heap backing");
    let backing_start = backing
        .start_frame_number()
        .checked_mul(PAGE_SIZE)
        .expect("managed heap frame address");
    let _ = writeln!(
        console,
        "kernel-heap: start={:#018x} frames={} bytes={} records={}",
        backing_start,
        backing.len(),
        heap.capacity(),
        KERNEL_HEAP_ALLOCATIONS
    );
    let _ = writeln!(console, "{KERNEL_HEAP_SUCCESS_SENTINEL}");
    console.sink_mut().flush();
}

#[cfg(all(
    feature = "kernel-map-probe",
    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
))]
fn exercise_runtime_kernel_stack(
    tables: &'static FinalKernelTables,
    direct_map_layout: FinalDirectMapLayout<KERNEL_MAP_MEMORY_RANGES>,
    allocator: &mut FrameAllocator<KERNEL_MAP_MEMORY_RANGES>,
    pool: &mut QemuVirtKernelStackPool,
    console: &mut Console<EarlyPl011>,
) {
    const PROBE: &[u8] = b"Phoenix mapped kernel stack";

    let free_before = allocator.total_free_frames();
    // SAFETY: `tables` is the retained active hierarchy that produced this
    // direct-map proof. The single boot CPU has exceptions masked and owns the
    // allocator, pool, and every table descriptor touched below.
    let mut direct_map = unsafe { FinalDirectMapMemory::assume_active(tables, direct_map_layout) };
    let stack = pool
        .allocate(allocator, &mut direct_map, KernelStackKind::Thread)
        .unwrap_or_else(|error| panic!("could not allocate kernel-stack probe: {error:?}"));
    let first_mapping = stack
        .mappings()
        .next()
        .expect("runtime stack has usable pages");
    let layout = stack.layout();
    assert_eq!(
        allocator.total_free_frames(),
        free_before - phoenix_kernel::platform::aarch64::qemu_virt::KERNEL_STACK_USABLE_PAGES,
        "kernel stack did not retain its exact frames"
    );

    let stack = {
        let mut maintenance = Aarch64KernelLeafTlb;
        // SAFETY: the final hierarchy is active and permanently retained;
        // `direct_map` uniquely accesses its table frames, while this boot
        // path is single-CPU with exceptions unable to mutate descriptors.
        let mut mapper =
            unsafe { Aarch64KernelStackMapper::new(tables, &mut direct_map, &mut maintenance) };
        let mapped = stack
            .publish(&mut mapper)
            .unwrap_or_else(|(_stack, error)| panic!("could not publish kernel stack: {error:?}"));
        assert_eq!(mapped.initial_stack_pointer(), layout.usable().end());
        runtime_probe::stack_guards(
            layout.lower_guard().start().as_usize(),
            layout.upper_guard().start().as_usize(),
        );

        let virtual_page = first_mapping.0.as_usize() as *mut u8;
        // SAFETY: `mapped` owns the exact newly published RW/NX L3 leaves;
        // `PROBE` fits inside the first usable page and no other code can
        // access this unentered stack slot.
        unsafe {
            for (offset, byte) in PROBE.iter().copied().enumerate() {
                core::ptr::write_volatile(virtual_page.add(offset), byte);
            }
            for (offset, expected) in PROBE.iter().copied().enumerate() {
                assert_eq!(core::ptr::read_volatile(virtual_page.add(offset)), expected);
            }
        }

        match mapped.unpublish(&mut mapper) {
            Ok(stack) => stack,
            Err((_mapped, error)) => panic!("could not retire kernel stack: {error:?}"),
        }
    };

    let mut observed = [0_u8; PROBE.len()];
    assert!(
        direct_map
            .read_frame(first_mapping.1, 4095, &mut observed)
            .is_err()
    );
    assert!(
        direct_map
            .read_frame(
                phoenix_kernel::memory::PageFrame::from_start(PhysAddr::new(0)).unwrap(),
                0,
                &mut observed
            )
            .is_err()
    );
    direct_map
        .read_frame(first_mapping.1, 0, &mut observed)
        .unwrap_or_else(|error| panic!("could not verify kernel-stack frame: {error:?}"));
    assert_eq!(observed, PROBE, "kernel-stack virtual alias mismatch");
    for frame in stack.frames() {
        KernelStackMemory::clear_frame(&mut direct_map, frame)
            .unwrap_or_else(|error| panic!("could not scrub kernel-stack frame: {error:?}"));
    }
    stack
        .release_unpublished(pool, allocator)
        .unwrap_or_else(|(_stack, error)| panic!("could not release kernel stack: {error:?}"));
    assert_eq!(
        allocator.total_free_frames(),
        free_before,
        "kernel-stack probe did not restore its frames"
    );
    assert_eq!(pool.allocated_slots(), 0);

    let _ = writeln!(
        console,
        "kernel-stack: start={:#018x} pages={} sp={:#018x}",
        layout.usable().start().as_usize(),
        phoenix_kernel::platform::aarch64::qemu_virt::KERNEL_STACK_USABLE_PAGES,
        layout.initial_stack_pointer().as_usize(),
    );
    let _ = writeln!(console, "{KERNEL_STACK_SUCCESS_SENTINEL}");
    console.sink_mut().flush();
}

#[cfg(feature = "loaded-init-probe")]
fn run_loaded_init_probe(boot_argument: usize, console: &mut Console<EarlyPl011>) -> ! {
    if cfg!(feature = "system") {
        system::boot(boot_argument, console);
    }
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
    let (kernel_plan, map, gic) = {
        let tree = physical_memory
            .device_tree(device_tree_start)
            .unwrap_or_else(|error| panic!("could not borrow boot DTB: {error:?}"));
        let info = tree
            .boot_info()
            .unwrap_or_else(|error| panic!("could not extract boot information: {error:?}"));
        // SAFETY: the single boot CPU still owns the bootstrap low device
        // identity map, IRQs remain masked, and no GIC consumer exists yet.
        let gic = unsafe { discover_boot_gicv3(tree) }
            .unwrap_or_else(|error| panic!("could not discover boot GICv3: {error:?}"));
        let kernel_image = kernel_physical_range()
            .unwrap_or_else(|error| panic!("could not derive kernel image range: {error:?}"));
        let kernel_plan = final_kernel_page_table_plan(info, gic.layout())
            .unwrap_or_else(|error| panic!("could not plan final kernel page tables: {error:?}"));
        let map =
            memory_map_from_boot_info::<INIT_MEMORY_RANGES>(info, kernel_image, device_tree_start)
                .unwrap_or_else(|error| panic!("could not construct boot memory map: {error:?}"));
        (kernel_plan, map, gic)
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
    let direct_map_layout = final_direct_map_layout(&kernel_plan, allocator.managed_frames())
        .unwrap_or_else(|error| panic!("could not prove final RAM direct map: {error:?}"));
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
    let init_id = ProcessId::new(0, 1).expect("the bootstrap init generation is nonzero");
    let mut asids = AsidAllocator::new();
    let init_asid = asids
        .allocate()
        .unwrap_or_else(|error| panic!("could not allocate init ASID: {error:?}"));
    let mut process = ProcessControl::new(init_id);
    process
        .make_ready()
        .unwrap_or_else(|error| panic!("could not make init ready: {error:?}"));
    // SAFETY: this single-CPU boot path initializes the slot exactly once
    // before EL0 can issue an exception. The slot is never replaced or freed.
    let (prepared, kernel_tables, gic) = unsafe {
        install_init_runtime(InitRuntime {
            process,
            _init_asid: init_asid,
            address_space: prepared,
            kernel_tables,
            direct_map_layout,
            gic,
            _kernel_stack_pool: runtime_kernel_stack_pool()
                .unwrap_or_else(|error| panic!("could not reserve kernel-stack arena: {error:?}")),
            files: ReadOnlyFileTable::new(initramfs),
            _allocator: allocator,
            _asids: asids,
        })
    };

    let _ = writeln!(
        console,
        "init-kernel-map: root={kernel_root:#018x} tables={kernel_table_count} leaves={kernel_mapping_count}"
    );
    let _ = writeln!(console, "{INIT_KERNEL_MAP_ENTER_SENTINEL}");
    console.sink_mut().flush();
    // SAFETY: the retained final hierarchy covers the current image, stack,
    // vectors, direct-mapped RAM, PL011, GICD, and selected boot GICR. The
    // bootstrap handle was dropped, and this remains a masked, single-core
    // boot path.
    unsafe {
        kernel_tables.activate();
    }

    // SAFETY: the installed runtime permanently owns the active final tables
    // and copied boot-CPU GIC evidence. IRQs remain masked, the boot CPU is the
    // sole observer, and no interrupt can enter before lookup publication.
    let gic_configuration =
        unsafe { prepare_and_publish_boot_interrupt_runtime(kernel_tables, gic) };
    let _ = writeln!(
        console,
        "gicv3: timer-intid={} poll-budget={} delivery=closed",
        gic_configuration.timer().intid(),
        gic_configuration.poll_budget(),
    );
    let _ = writeln!(console, "{GIC_PREPARED_SENTINEL}");
    let _ = writeln!(console, "{IRQ_RUNTIME_PUBLISHED_SENTINEL}");
    let _ = writeln!(console, "{INIT_KERNEL_MAP_SENTINEL}");
    console.sink_mut().flush();

    // SAFETY: the one-time runtime is installed in the ready state and no
    // execution context can race this single-core transition before `eret`.
    let init_id = unsafe { start_active_init_process() }
        .unwrap_or_else(|error| panic!("could not start init: {error:?}"));

    let _ = writeln!(
        console,
        "init: pid={}.{} asid={} entry={:#018x} stack={:#018x} pages={} root={:#018x}",
        init_id.slot(),
        init_id.generation(),
        init_asid.raw(),
        prepared.entry().as_usize(),
        prepared.stack_pointer().as_usize(),
        prepared.resident_page_count(),
        prepared.root_frame().start_address().as_usize()
    );
    let _ = writeln!(console, "{INIT_ENTER_SENTINEL}");
    console.sink_mut().flush();

    // SAFETY: the runtime permanently owns every fully initialized leaf and
    // table frame. The bootstrap keeps caches disabled, vectors and the EL1
    // stack are active, and this single-core probe is the only user of the
    // retained nonzero ASID.
    unsafe {
        prepared.activate_and_enter(init_asid);
    }
}

#[cfg(feature = "loaded-init-probe")]
unsafe fn install_init_runtime(
    runtime: InitRuntime,
) -> (
    &'static InitAddressSpace,
    &'static FinalKernelTables,
    BootGicV3Discovery,
) {
    let slot = &raw mut INIT_RUNTIME;
    // SAFETY: the caller guarantees unique one-time initialization and eternal
    // retention; raw pointers avoid manufacturing a reference before init.
    unsafe {
        (*slot).write(runtime);
        let runtime = (*slot).as_ptr();
        (
            &*core::ptr::addr_of!((*runtime).address_space),
            &*core::ptr::addr_of!((*runtime).kernel_tables),
            *core::ptr::addr_of!((*runtime).gic),
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
unsafe fn active_init_direct_map()
-> FinalDirectMapMemory<'static, FINAL_KERNEL_TABLE_CAPACITY, INIT_MEMORY_RANGES> {
    let slot = &raw const INIT_RUNTIME;
    // SAFETY: EL0 and syscall dispatch are reachable only after the runtime
    // was installed and its exact kernel tables were activated. The retained
    // proof was derived from the plan consumed to materialize those tables;
    // both fields remain alive forever in this single-core probe.
    unsafe {
        let runtime = (*slot).as_ptr();
        let tables = &*core::ptr::addr_of!((*runtime).kernel_tables);
        let layout = *core::ptr::addr_of!((*runtime).direct_map_layout);
        FinalDirectMapMemory::assume_active(tables, layout)
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

#[cfg(feature = "loaded-init-probe")]
unsafe fn start_active_init_process()
-> Result<ProcessId, phoenix_kernel::process::ProcessTransitionError> {
    let slot = &raw mut INIT_RUNTIME;
    // SAFETY: the runtime is installed before this function is reachable.
    // Bootstrap calls this once before EL0 entry and no process-field reference
    // escapes the operation.
    unsafe {
        let runtime = (*slot).as_mut_ptr();
        let process = &mut *core::ptr::addr_of_mut!((*runtime).process);
        process.start()?;
        Ok(process.id())
    }
}

#[cfg(feature = "loaded-init-probe")]
unsafe fn exit_active_init_process(
    status: ExitStatus,
) -> Result<(), phoenix_kernel::process::ProcessTransitionError> {
    let slot = &raw mut INIT_RUNTIME;
    // SAFETY: the runtime is installed and running before SVC dispatch. The
    // dispatcher is non-reentrant on one CPU and no mutable reference escapes.
    unsafe {
        let runtime = (*slot).as_mut_ptr();
        let process = &mut *core::ptr::addr_of_mut!((*runtime).process);
        process.exit(status)
    }
}

#[cfg(feature = "loaded-init-probe")]
unsafe fn fault_active_init_process(
    fault: UserFault,
) -> Result<ProcessId, phoenix_kernel::process::ProcessTransitionError> {
    let slot = &raw mut INIT_RUNTIME;
    // SAFETY: the runtime is installed and running before a lower-EL fault is
    // dispatched. Dispatch is non-reentrant on one CPU and no borrow escapes.
    unsafe {
        let runtime = (*slot).as_mut_ptr();
        let process = &mut *core::ptr::addr_of_mut!((*runtime).process);
        process.fault(fault)?;
        Ok(process.id())
    }
}

#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
fn boot_interrupt_handle() -> Option<&'static BootInterruptHandle> {
    if !BOOT_INTERRUPT_PUBLISHED.load(Ordering::Acquire) {
        return None;
    }
    let slot = &raw const BOOT_INTERRUPT_HANDLE;
    // SAFETY: Acquire pairs with one-time initialization; both permanently
    // pinned owners remain valid, accessed only by the boot CPU and its IRQs.
    Some(unsafe { &*(*slot).as_ptr() })
}

#[cfg(any(
    feature = "loaded-init-probe",
    all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    )
))]
fn dispatch_boot_irq(slot: Option<VectorSlot>) -> Option<Result<(), BootIrqFailure>> {
    let slot = slot?;
    if slot.kind() != ExceptionKind::Irq {
        return None;
    }
    let Some(handle) = boot_interrupt_handle() else {
        return Some(Err(BootIrqFailure::Unpublished));
    };
    let local_irq = LocalIrq::new();
    Some(match handle.dispatch_one(&local_irq) {
        Ok(InterruptDispatch::Spurious { .. }) => Ok(()),
        Ok(InterruptDispatch::Timer(batch)) if batch.is_empty() => Ok(()),
        #[cfg(all(
            feature = "kernel-map-probe",
            not(any(feature = "el0-probe", feature = "loaded-init-probe"))
        ))]
        Ok(InterruptDispatch::Timer(batch)) if kernel_threads::active() => {
            kernel_threads::timer_batch(&batch);
            Ok(())
        }
        Ok(InterruptDispatch::Timer(batch)) => BOOT_TIMER_CONSUMER
            .consume(&local_irq, &batch)
            .map(|_| {
                #[cfg(all(
                    feature = "kernel-map-probe",
                    not(any(feature = "el0-probe", feature = "loaded-init-probe"))
                ))]
                runtime_probe::irq_completed();
            })
            .map_err(BootIrqFailure::TimerConsumer),
        Ok(InterruptDispatch::Unexpected { intid }) => Err(BootIrqFailure::Unexpected { intid }),
        Err(error) => Err(BootIrqFailure::Runtime(error)),
    })
}

/// Common target of the assembly exception vectors.
///
/// # Safety
///
/// `frame` must point to the uniquely borrowed, fully initialized frame at the
/// current exception stack pointer, and `slot_index` must identify its vector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn phoenix_exception_dispatch(
    frame: *mut ExceptionFrame,
    slot_index: usize,
) -> *mut ExceptionFrame {
    // SAFETY: `vectors.S` allocates one aligned `ExceptionFrame`, initializes
    // every field, and passes its unique mutable address for this call.
    let frame = unsafe { &mut *frame };
    let slot = VectorSlot::ALL.get(slot_index).copied();
    #[cfg(feature = "loaded-init-probe")]
    if system::active() {
        system::dispatch(frame, slot);
        return frame;
    }
    #[cfg(all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    ))]
    if let Some(next) = kernel_threads::dispatch(frame, slot_index, false) {
        return next;
    }

    #[cfg(all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    ))]
    if runtime_probe::exception(frame, slot_index) {
        return frame;
    }

    #[cfg(any(
        feature = "loaded-init-probe",
        all(
            feature = "kernel-map-probe",
            not(any(feature = "el0-probe", feature = "loaded-init-probe"))
        )
    ))]
    let irq_failure = match dispatch_boot_irq(slot) {
        Some(Ok(())) => {
            #[cfg(all(
                feature = "kernel-map-probe",
                not(any(feature = "el0-probe", feature = "loaded-init-probe"))
            ))]
            if let Some(next) = kernel_threads::dispatch(frame, slot_index, true) {
                return next;
            }
            return frame;
        }
        Some(Err(failure)) => Some(failure),
        None => None,
    };

    #[cfg(any(feature = "el0-probe", feature = "loaded-init-probe"))]
    if handle_probe_syscall(frame, slot) {
        return frame;
    }

    #[cfg(feature = "loaded-init-probe")]
    if handle_loaded_init_fault(frame, slot) {
        return frame;
    }
    #[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
    if handle_static_el0_fault(frame, slot) {
        return frame;
    }

    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{EXCEPTION_SENTINEL}");
    let _ = writeln!(console, "vector: {slot:?}");
    let _ = writeln!(console, "pc: {:#018x}", frame.program_counter());
    let _ = writeln!(console, "spsr: {:#018x}", frame.saved_program_status());
    let _ = writeln!(console, "esr: {:#018x}", frame.syndrome().raw());
    let _ = writeln!(console, "far: {:#018x}", frame.fault_address());
    for index in 0..31 {
        let _ = writeln!(console, "x{index}: {:#018x}", frame.general(index).unwrap());
    }
    print_stack_trace(&mut console);
    events::dump(&mut console);
    #[cfg(any(
        feature = "loaded-init-probe",
        all(
            feature = "kernel-map-probe",
            not(any(feature = "el0-probe", feature = "loaded-init-probe"))
        )
    ))]
    if let Some(failure) = irq_failure {
        match failure {
            BootIrqFailure::Unpublished => {
                let _ = writeln!(console, "irq: runtime unpublished");
            }
            BootIrqFailure::Runtime(error) => {
                let _ = writeln!(console, "irq: runtime error {error:?}");
            }
            BootIrqFailure::Unexpected { intid } => {
                let _ = writeln!(console, "irq: unexpected intid={intid}");
            }
            BootIrqFailure::TimerConsumer(error) => {
                let _ = writeln!(console, "irq: timer consumer error {error:?}");
            }
        }
    }
    console.sink_mut().flush();
    #[cfg(all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    ))]
    runtime_probe::fatal_exit(frame, slot_index);
    halt()
}

#[cfg(feature = "loaded-init-probe")]
fn handle_loaded_init_fault(frame: &ExceptionFrame, slot: Option<VectorSlot>) -> bool {
    let Some(slot) = slot else {
        return false;
    };
    let Some(fault) = classify_user_fault(slot, frame) else {
        return false;
    };
    // SAFETY: the only reachable lower-EL context is the installed running
    // init process, and exception dispatch is synchronous and non-reentrant.
    let Ok(process_id) = (unsafe { fault_active_init_process(fault) }) else {
        return false;
    };
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{USER_FAULT_SENTINEL}");
    let _ = writeln!(
        console,
        "fault: process={process_id:?} kind={:?} access={:?} pc={:#018x} address={:?} syndrome={:#018x}",
        fault.kind(),
        fault.access(),
        fault.instruction_pointer(),
        fault.fault_address(),
        fault.architecture_code(),
    );
    let _ = writeln!(console, "{INIT_FAILURE_SENTINEL}: user-fault");
    console.sink_mut().flush();
    halt()
}

#[cfg(all(feature = "el0-probe", not(feature = "loaded-init-probe")))]
fn handle_static_el0_fault(frame: &ExceptionFrame, slot: Option<VectorSlot>) -> bool {
    let Some(slot) = slot else {
        return false;
    };
    let Some(fault) = classify_user_fault(slot, frame) else {
        return false;
    };
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{USER_FAULT_SENTINEL}");
    let _ = writeln!(
        console,
        "fault: kind={:?} access={:?} pc={:#018x} address={:?} syndrome={:#018x}",
        fault.kind(),
        fault.access(),
        fault.instruction_pointer(),
        fault.fault_address(),
        fault.architecture_code(),
    );
    let _ = writeln!(console, "{EL0_FAILURE_SENTINEL}: user-fault");
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
            // SAFETY: this is the first terminal transition of the installed
            // single-core init process, and dispatch cannot be re-entered.
            let lifecycle = unsafe { exit_active_init_process(ExitStatus::new(status)) };
            #[cfg(feature = "loaded-init-probe")]
            let _ = if status == 42
                && INIT_WROTE_OUTPUT.load(Ordering::Acquire)
                && lifecycle.is_ok()
            {
                writeln!(console, "{INIT_SUCCESS_SENTINEL}")
            } else {
                writeln!(
                    console,
                    "{INIT_FAILURE_SENTINEL}: status={status} wrote-output={} lifecycle={lifecycle:?}",
                    INIT_WROTE_OUTPUT.load(Ordering::Relaxed),
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
        NativeSyscall::Run
        | NativeSyscall::Spawn
        | NativeSyscall::Wait
        | NativeSyscall::Yield
        | NativeSyscall::Unknown(_) => {
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
    // SAFETY: the runtime and active final-map owner precede every EL0 syscall.
    let direct_map = unsafe { active_init_direct_map() };
    let mut memory = ActiveInitMemory::new(address_space, direct_map);
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
    // SAFETY: the runtime and active final-map owner precede every EL0 syscall.
    let direct_map = unsafe { active_init_direct_map() };
    let mut memory = ActiveInitMemory::new(address_space, direct_map);
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
    // SAFETY: the runtime and active final-map owner precede every EL0 syscall.
    let direct_map = unsafe { active_init_direct_map() };
    let mut memory = ActiveInitMemory::new(address_space, direct_map);
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
    phoenix_kernel::arch::aarch64::diagnostics::disable_irqs();
    let mut console = Console::new(EarlyPl011::new());
    let _ = writeln!(console, "{PANIC_SENTINEL}");
    let _ = writeln!(console, "{info}");
    print_stack_trace(&mut console);
    events::dump(&mut console);
    console.sink_mut().flush();
    #[cfg(all(
        feature = "kernel-map-probe",
        not(any(feature = "el0-probe", feature = "loaded-init-probe"))
    ))]
    runtime_probe::panic_exit();
    halt()
}

fn print_stack_trace(console: &mut Console<EarlyPl011>) {
    use phoenix_kernel::arch::aarch64::diagnostics as cpu;
    use phoenix_kernel::platform::aarch64::qemu_virt::{
        KERNEL_STACK_ARENA_SLOTS, KERNEL_STACK_ARENA_START, KERNEL_STACK_USABLE_PAGES,
    };
    unsafe extern "C" {
        static __boot_stack_bottom: u8;
        static __boot_stack_top: u8;
        static __text_start: u8;
        static __text_end: u8;
    }
    let (_, sp, _) = cpu::boot_state();
    let mut bounds =
        (&raw const __boot_stack_bottom) as usize..(&raw const __boot_stack_top) as usize;
    if !bounds.contains(&sp) {
        let stride = (KERNEL_STACK_USABLE_PAGES + 2) * 4096;
        let arena =
            KERNEL_STACK_ARENA_START..KERNEL_STACK_ARENA_START + KERNEL_STACK_ARENA_SLOTS * stride;
        if !arena.contains(&sp) {
            return;
        }
        let bottom =
            KERNEL_STACK_ARENA_START + ((sp - KERNEL_STACK_ARENA_START) / stride) * stride + 4096;
        bounds = bottom..bottom + KERNEL_STACK_USABLE_PAGES * 4096;
        if !bounds.contains(&sp) {
            return;
        }
    }
    let text = (&raw const __text_start) as usize..(&raw const __text_end) as usize;
    let count = phoenix_kernel::stack_trace::walk::<16>(
        cpu::frame_pointer(),
        bounds,
        text,
        |address| {
            // SAFETY: The walker checks alignment and the complete 16-byte record
            // inside this CPU's active owned stack. IRQs are masked; the stack's
            // mapping/owner cannot be retired until this context is switched out.
            unsafe {
                (
                    (address as *const usize).read_volatile(),
                    ((address + 8) as *const usize).read_volatile(),
                )
            }
        },
        |address| {
            let _ = writeln!(console, "trace: return={address:#018x}");
        },
    );
    let _ = writeln!(console, "trace: frames={count} limit=16");
}
