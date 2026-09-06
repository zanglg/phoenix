//! AArch64 register adaptation for the native Phoenix system-call ABI.

use crate::abi::{SYSCALL_ARGUMENT_COUNT, SyscallRequest, SyscallReturn};

use super::exception::ExceptionFrame;

/// Register holding the native system-call number on AArch64.
pub const NUMBER_REGISTER: usize = 8;
/// First register holding a system-call argument and the return value.
pub const RETURN_REGISTER: usize = 0;

/// Decode `x8` and `x0..x5` from an exception frame.
pub fn request_from_frame(frame: &ExceptionFrame) -> SyscallRequest {
    let mut arguments = [0; SYSCALL_ARGUMENT_COUNT];
    for (index, argument) in arguments.iter_mut().enumerate() {
        *argument = frame
            .general(index)
            .expect("x0..x5 always exist in an ExceptionFrame");
    }
    let number = frame
        .general(NUMBER_REGISTER)
        .expect("x8 always exists in an ExceptionFrame");
    SyscallRequest::new(number, arguments)
}

/// Store a native return value in `x0` for the exception epilogue.
pub fn set_return(frame: &mut ExceptionFrame, value: SyscallReturn) {
    let stored = frame.set_general(RETURN_REGISTER, value.raw());
    debug_assert!(stored, "x0 must exist in an ExceptionFrame");
}

#[cfg(test)]
mod tests {
    use super::{request_from_frame, set_return};
    use crate::abi::{Errno, NativeSyscall, SyscallReturn};
    use crate::arch::aarch64::exception::ExceptionFrame;

    #[test]
    fn adapts_aarch64_registers_without_changing_the_saved_pc() {
        let mut frame = ExceptionFrame::zeroed();
        for register in 0..=8 {
            assert!(frame.set_general(register, 100 + register as u64));
        }
        frame.set_general(8, NativeSyscall::Write.number());
        frame.set_program_counter(0x40_1004);

        let request = request_from_frame(&frame);
        assert_eq!(request.operation(), NativeSyscall::Write);
        assert_eq!(request.arguments(), [100, 101, 102, 103, 104, 105]);

        set_return(&mut frame, SyscallReturn::error(Errno::BadFileDescriptor));
        assert_eq!(frame.general(0), Some((-9_i64) as u64));
        assert_eq!(frame.program_counter(), 0x40_1004);
    }
}
