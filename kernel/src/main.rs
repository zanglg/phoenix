#![no_main]
#![no_std]

#[cfg(not(target_arch = "aarch64"))]
compile_error!("the Phoenix kernel binary currently supports only AArch64");

use core::fmt::Write;
use core::panic::PanicInfo;

use phoenix_kernel::arch::aarch64::exception::{ExceptionFrame, VectorSlot};
use phoenix_kernel::arch::aarch64::{halt, install_exception_vectors};
use phoenix_kernel::build_info::BuildInfo;
use phoenix_kernel::console::Console;
use phoenix_kernel::platform::aarch64::qemu_virt::EarlyPl011;

#[cfg(feature = "boot-memory-probe")]
use phoenix_kernel::memory::{PhysAddr, memory_map_from_boot_info};
#[cfg(feature = "boot-memory-probe")]
use phoenix_kernel::platform::aarch64::qemu_virt::{
    BootstrapPhysicalMemory, kernel_physical_range,
};
#[cfg(feature = "boot-memory-probe")]
use phoenix_kernel::process_image::ProcessImageMemory;

#[cfg(feature = "el0-probe")]
use phoenix_kernel::abi::{Errno, NATIVE_SVC_IMMEDIATE, NativeSyscall, SyscallReturn};
#[cfg(feature = "el0-probe")]
use phoenix_kernel::arch::aarch64::el0;
#[cfg(feature = "el0-probe")]
use phoenix_kernel::arch::aarch64::exception::{ExceptionClass, ExceptionKind, VectorOrigin};
#[cfg(feature = "el0-probe")]
use phoenix_kernel::arch::aarch64::syscall;

core::arch::global_asm!(include_str!("arch/aarch64/boot.S"));
core::arch::global_asm!(include_str!("arch/aarch64/vectors.S"));
#[cfg(feature = "el0-probe")]
core::arch::global_asm!(include_str!("arch/aarch64/user_probe.S"));

const BOOT_SUCCESS_SENTINEL: &str = "PHOENIX_BOOT_OK";
const PANIC_SENTINEL: &str = "PHOENIX_PANIC";
const EXCEPTION_SENTINEL: &str = "PHOENIX_EXCEPTION";
#[cfg(feature = "boot-memory-probe")]
const MEMORY_SUCCESS_SENTINEL: &str = "PHOENIX_MEMORY_OK";
#[cfg(feature = "el0-probe")]
const EL0_ENTER_SENTINEL: &str = "PHOENIX_EL0_ENTER";
#[cfg(feature = "el0-probe")]
const EL0_SUCCESS_SENTINEL: &str = "PHOENIX_EL0_OK";
#[cfg(feature = "el0-probe")]
const EL0_FAILURE_SENTINEL: &str = "PHOENIX_EL0_FAIL";

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

    #[cfg(feature = "el0-probe")]
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

    #[cfg(not(feature = "el0-probe"))]
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

    #[cfg(feature = "el0-probe")]
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

#[cfg(feature = "el0-probe")]
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
            if status == 42 {
                let _ = writeln!(console, "{EL0_SUCCESS_SENTINEL}");
            } else {
                let _ = writeln!(console, "{EL0_FAILURE_SENTINEL}: status={status}");
            }
            console.sink_mut().flush();
            halt()
        }
        NativeSyscall::Write | NativeSyscall::Unknown(_) => {
            syscall::set_return(frame, SyscallReturn::error(Errno::NotImplemented));
            true
        }
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
