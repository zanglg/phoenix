//! EL1h thread contexts restored through the ordinary exception epilogue.
use super::exception::ExceptionFrame;
use core::arch::asm;

/// Build a first-entry frame on a private, mapped kernel stack.
///
/// # Safety
/// `top` must be 16-byte aligned and have at least ExceptionFrame::SIZE writable
/// bytes below it. The stack must remain mapped and uniquely owned until the
/// thread exits and is no longer current. `entry` must never return.
pub unsafe fn initialize(
    top: usize,
    entry: extern "C" fn(usize) -> !,
    argument: usize,
) -> *mut ExceptionFrame {
    assert_eq!(top & 15, 0);
    let pointer = (top - ExceptionFrame::SIZE) as *mut ExceptionFrame;
    let frame = ExceptionFrame::kernel_thread(entry as usize as u64, argument as u64);
    // SAFETY: The caller grants the complete aligned, exclusive frame storage.
    unsafe {
        pointer.write(frame);
    }
    pointer
}

/// Reschedule a kernel thread at a lock-free boundary.
pub fn yield_now() {
    // SAFETY: This SVC enters the installed kernel scheduler; caller registers
    // are preserved by the complete exception frame and same-CPU return.
    unsafe {
        asm!("svc #0x100", options(nostack));
    }
}

/// Sleep for a bounded number of nanoseconds at a lock-free boundary.
pub fn sleep(nanoseconds: u64) {
    // SAFETY: The handler consumes x0 as a duration, saves every register, and
    // restores the context only after the absolute deadline has elapsed.
    unsafe {
        asm!("svc #0x101", in("x0") nanoseconds, options(nostack));
    }
}

/// Permanently exit the current kernel thread. Its stack is reaped elsewhere.
pub fn exit() -> ! {
    // SAFETY: The installed scheduler removes this thread from the run queue;
    // no Rust values or borrowed guards may require cleanup after this call.
    unsafe {
        asm!("svc #0x102", options(noreturn));
    }
}

/// Wait for a timer tick in the permanently retained idle/boot context.
pub fn idle() {
    // SAFETY: WFI merely waits; the caller has enabled a live IRQ wake source.
    unsafe {
        asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

/// Enter a constructed process frame using the common register-restore path.
///
/// # Safety
/// The selected process root and its lifetime must already be established.
/// The frame must reside on the active mapped kernel stack. No borrowed runtime
/// state or lock guard may survive the non-returning transition to EL0.
pub unsafe fn enter_user(frame: &ExceptionFrame) -> ! {
    // SAFETY: Caller supplies a complete frame on the retained kernel stack;
    // the assembly restores all integer/control state and enters EL0 by ERET.
    unsafe {
        asm!("mov sp, {frame}", "b phoenix_restore_frame", frame = in(reg) frame, options(noreturn));
    }
}
