//! Initial native Phoenix system-call ABI value types.

/// Current experimental native ABI revision.
///
/// Revision zero carries no stability promise and changes with the 0.0.0 tree.
pub const NATIVE_ABI_REVISION: u16 = 0;
/// Immediate reserved for native Phoenix calls made with AArch64 `SVC`.
pub const NATIVE_SVC_IMMEDIATE: u16 = 0;
/// Maximum number of system-call arguments carried in registers.
pub const SYSCALL_ARGUMENT_COUNT: usize = 6;
/// Largest error number recognized in the signed return-value encoding.
pub const MAX_ERROR_NUMBER: u16 = 4095;

/// Initially assigned native system-call operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSyscall {
    /// Terminate the current process (`number=0`).
    Exit,
    /// Write bytes to a file descriptor (`number=1`).
    Write,
    /// Unassigned number retained for forward-compatible rejection.
    Unknown(u64),
}

impl NativeSyscall {
    /// Decode a raw system-call number without discarding unknown values.
    pub const fn decode(number: u64) -> Self {
        match number {
            0 => Self::Exit,
            1 => Self::Write,
            number => Self::Unknown(number),
        }
    }

    /// Return the assigned number, including an unknown raw value.
    pub const fn number(self) -> u64 {
        match self {
            Self::Exit => 0,
            Self::Write => 1,
            Self::Unknown(number) => number,
        }
    }
}

/// System-call number and six raw register arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallRequest {
    operation: NativeSyscall,
    arguments: [u64; SYSCALL_ARGUMENT_COUNT],
}

impl SyscallRequest {
    /// Decode a raw number and preserve all argument registers.
    pub const fn new(number: u64, arguments: [u64; SYSCALL_ARGUMENT_COUNT]) -> Self {
        Self {
            operation: NativeSyscall::decode(number),
            arguments,
        }
    }

    /// Return the decoded operation.
    pub const fn operation(self) -> NativeSyscall {
        self.operation
    }

    /// Return all six raw arguments.
    pub const fn arguments(self) -> [u64; SYSCALL_ARGUMENT_COUNT] {
        self.arguments
    }

    /// Return one raw argument by zero-based index.
    pub const fn argument(self, index: usize) -> Option<u64> {
        if index < SYSCALL_ARGUMENT_COUNT {
            Some(self.arguments[index])
        } else {
            None
        }
    }
}

/// Initially assigned ABI error numbers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum Errno {
    /// File descriptor is not open for the requested operation.
    BadFileDescriptor = 9,
    /// A user pointer cannot be accessed for the complete request.
    BadAddress = 14,
    /// Memory allocation failed.
    NoMemory = 12,
    /// An argument value is outside the operation's contract.
    InvalidArgument = 22,
    /// The system call is unknown or not implemented.
    NotImplemented = 38,
}

/// Failure while constructing a raw system-call return value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallReturnError {
    /// A successful value would collide with the signed error range.
    SuccessValueTooLarge(u64),
}

/// Encoded native system-call return register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct SyscallReturn(u64);

impl SyscallReturn {
    /// Encode a non-negative success value.
    pub const fn success(value: u64) -> Result<Self, SyscallReturnError> {
        if value <= i64::MAX as u64 {
            Ok(Self(value))
        } else {
            Err(SyscallReturnError::SuccessValueTooLarge(value))
        }
    }

    /// Encode a negative ABI error number.
    pub const fn error(error: Errno) -> Self {
        Self((-(error as i64)) as u64)
    }

    /// Preserve a raw return register for decoding and tests.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the register representation.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Decode the signed error convention while retaining unknown errors.
    pub const fn decode(self) -> DecodedSyscallReturn {
        let signed = self.0 as i64;
        if signed < 0 && signed >= -(MAX_ERROR_NUMBER as i64) {
            DecodedSyscallReturn::Error((-signed) as u16)
        } else {
            DecodedSyscallReturn::Success(self.0)
        }
    }
}

/// Interpreted native return value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodedSyscallReturn {
    /// Successful non-negative value.
    Success(u64),
    /// Positive error number decoded from a negative register value.
    Error(u16),
}

#[cfg(test)]
mod tests {
    use super::{
        DecodedSyscallReturn, Errno, NativeSyscall, SyscallRequest, SyscallReturn,
        SyscallReturnError,
    };

    #[test]
    fn syscall_numbers_preserve_unknown_values() {
        assert_eq!(NativeSyscall::decode(0), NativeSyscall::Exit);
        assert_eq!(NativeSyscall::decode(1), NativeSyscall::Write);
        assert_eq!(NativeSyscall::decode(99), NativeSyscall::Unknown(99));
        assert_eq!(NativeSyscall::Unknown(u64::MAX).number(), u64::MAX);
    }

    #[test]
    fn request_preserves_six_arguments_and_checks_indices() {
        let request = SyscallRequest::new(1, [10, 11, 12, 13, 14, 15]);
        assert_eq!(request.operation(), NativeSyscall::Write);
        assert_eq!(request.arguments(), [10, 11, 12, 13, 14, 15]);
        assert_eq!(request.argument(5), Some(15));
        assert_eq!(request.argument(6), None);
    }

    #[test]
    fn return_encoding_separates_success_from_negative_errors() {
        let success = SyscallReturn::success(i64::MAX as u64).unwrap();
        assert_eq!(
            success.decode(),
            DecodedSyscallReturn::Success(i64::MAX as u64)
        );
        assert_eq!(
            SyscallReturn::success(i64::MAX as u64 + 1),
            Err(SyscallReturnError::SuccessValueTooLarge(
                i64::MAX as u64 + 1
            ))
        );

        let error = SyscallReturn::error(Errno::BadAddress);
        assert_eq!(error.raw() as i64, -14);
        assert_eq!(error.decode(), DecodedSyscallReturn::Error(14));
        assert_eq!(
            SyscallReturn::from_raw((-4096_i64) as u64).decode(),
            DecodedSyscallReturn::Success((-4096_i64) as u64)
        );
    }
}
