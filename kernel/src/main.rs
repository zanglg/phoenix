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
