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
/// Native file descriptor reserved for standard output at process creation.
pub const STANDARD_OUTPUT: u64 = 1;
/// Only open mode accepted by the initial read-only filesystem bridge.
pub const OPEN_READ_ONLY: u64 = 0;

/// Initially assigned native system-call operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSyscall {
    /// Terminate the current process (`number=0`).
    Exit,
    /// Write bytes to a file descriptor (`number=1`).
    Write,
    /// Read bytes from a file descriptor (`number=2`).
    Read,
    /// Open a canonical path (`number=3`).
    Open,
    /// Close a file descriptor (`number=4`).
    Close,
    /// Unassigned number retained for forward-compatible rejection.
    Unknown(u64),
}

impl NativeSyscall {
    /// Decode a raw system-call number without discarding unknown values.
    pub const fn decode(number: u64) -> Self {
        match number {
            0 => Self::Exit,
            1 => Self::Write,
            2 => Self::Read,
            3 => Self::Open,
            4 => Self::Close,
            number => Self::Unknown(number),
        }
    }

    /// Return the assigned number, including an unknown raw value.
    pub const fn number(self) -> u64 {
        match self {
            Self::Exit => 0,
            Self::Write => 1,
            Self::Read => 2,
            Self::Open => 3,
            Self::Close => 4,
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

/// Validated bounded standard-output request for the initial console bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConsoleWriteRequest {
    user_buffer: u64,
    length: usize,
}

/// Validated bounded read request for the initial file bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileReadRequest {
    descriptor: u64,
    user_buffer: u64,
    length: usize,
}

impl FileReadRequest {
    /// Validate the operation and configured byte bound.
    pub fn from_syscall(request: SyscallRequest, maximum: usize) -> Result<Self, Errno> {
        if request.operation() != NativeSyscall::Read {
            return Err(Errno::InvalidArgument);
        }
        let raw_length = request
            .argument(2)
            .expect("argument two is always retained");
        let length = usize::try_from(raw_length).map_err(|_| Errno::InvalidArgument)?;
        if length > maximum {
            return Err(Errno::InvalidArgument);
        }
        Ok(Self {
            descriptor: request
                .argument(0)
                .expect("argument zero is always retained"),
            user_buffer: request
                .argument(1)
                .expect("argument one is always retained"),
            length,
        })
    }

    /// Return the raw process-local descriptor.
    pub const fn descriptor(self) -> u64 {
        self.descriptor
    }

    /// Return the untrusted destination address.
    pub const fn user_buffer(self) -> u64 {
        self.user_buffer
    }

    /// Return the validated maximum byte count.
    pub const fn length(self) -> usize {
        self.length
    }
}

/// Validated bounded path request for the initial read-only open operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileOpenRequest {
    path_buffer: u64,
    path_length: usize,
}

impl FileOpenRequest {
    /// Validate operation, nonempty path bound, and read-only mode.
    pub fn from_syscall(request: SyscallRequest, maximum_path: usize) -> Result<Self, Errno> {
        if request.operation() != NativeSyscall::Open || request.argument(2) != Some(OPEN_READ_ONLY)
        {
            return Err(Errno::InvalidArgument);
        }
        let raw_length = request
            .argument(1)
            .expect("argument one is always retained");
        let path_length = usize::try_from(raw_length).map_err(|_| Errno::InvalidArgument)?;
        if path_length == 0 || path_length > maximum_path {
            return Err(Errno::InvalidArgument);
        }
        Ok(Self {
            path_buffer: request
                .argument(0)
                .expect("argument zero is always retained"),
            path_length,
        })
    }

    /// Return the untrusted path byte address.
    pub const fn path_buffer(self) -> u64 {
        self.path_buffer
    }

    /// Return the validated nonzero path byte count.
    pub const fn path_length(self) -> usize {
        self.path_length
    }
}

/// Validated close request for the initial file bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileCloseRequest {
    descriptor: u64,
}

impl FileCloseRequest {
    /// Validate the syscall operation and retain its descriptor.
    pub fn from_syscall(request: SyscallRequest) -> Result<Self, Errno> {
        if request.operation() != NativeSyscall::Close {
            return Err(Errno::InvalidArgument);
        }
        Ok(Self {
            descriptor: request
                .argument(0)
                .expect("argument zero is always retained"),
        })
    }

    /// Return the raw process-local descriptor.
    pub const fn descriptor(self) -> u64 {
        self.descriptor
    }
}

impl ConsoleWriteRequest {
    /// Validate the operation, standard-output descriptor, and configured byte bound.
    pub fn from_syscall(request: SyscallRequest, maximum: usize) -> Result<Self, Errno> {
        if request.operation() != NativeSyscall::Write {
            return Err(Errno::InvalidArgument);
        }
        if request.argument(0) != Some(STANDARD_OUTPUT) {
            return Err(Errno::BadFileDescriptor);
        }
        let raw_length = request
            .argument(2)
            .expect("argument two is always retained");
        let length = usize::try_from(raw_length).map_err(|_| Errno::InvalidArgument)?;
        if length > maximum {
            return Err(Errno::InvalidArgument);
        }
        Ok(Self {
            user_buffer: request
                .argument(1)
                .expect("argument one is always retained"),
            length,
        })
    }

    /// Return the untrusted raw user-buffer address for checked copying.
    pub const fn user_buffer(self) -> u64 {
        self.user_buffer
    }

    /// Return the validated bounded byte count.
    pub const fn length(self) -> usize {
        self.length
    }
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
    /// No filesystem object exists at the requested path.
    NoSuchFile = 2,
    /// An internal I/O transaction could not be completed consistently.
    InputOutput = 5,
    /// File descriptor is not open for the requested operation.
    BadFileDescriptor = 9,
    /// Memory allocation failed.
    NoMemory = 12,
    /// A filesystem object does not grant the requested access.
    PermissionDenied = 13,
    /// A user pointer cannot be accessed for the complete request.
    BadAddress = 14,
    /// An open operation selected a directory where a file was required.
    IsDirectory = 21,
    /// An argument value is outside the operation's contract.
    InvalidArgument = 22,
    /// The process has exhausted its open-file capacity.
    TooManyOpenFiles = 24,
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
        ConsoleWriteRequest, DecodedSyscallReturn, Errno, FileCloseRequest, FileOpenRequest,
        FileReadRequest, NativeSyscall, OPEN_READ_ONLY, STANDARD_OUTPUT, SyscallRequest,
        SyscallReturn, SyscallReturnError,
    };

    #[test]
    fn syscall_numbers_preserve_unknown_values() {
        assert_eq!(NativeSyscall::decode(0), NativeSyscall::Exit);
        assert_eq!(NativeSyscall::decode(1), NativeSyscall::Write);
        assert_eq!(NativeSyscall::decode(2), NativeSyscall::Read);
        assert_eq!(NativeSyscall::decode(3), NativeSyscall::Open);
        assert_eq!(NativeSyscall::decode(4), NativeSyscall::Close);
        assert_eq!(NativeSyscall::Close.number(), 4);
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

    #[test]
    fn console_write_request_checks_operation_descriptor_and_bound() {
        let request = SyscallRequest::new(1, [STANDARD_OUTPUT, 0x40_0000, 12, 0, 0, 0]);
        let write = ConsoleWriteRequest::from_syscall(request, 12).unwrap();
        assert_eq!(write.user_buffer(), 0x40_0000);
        assert_eq!(write.length(), 12);

        assert_eq!(
            ConsoleWriteRequest::from_syscall(
                SyscallRequest::new(0, [STANDARD_OUTPUT, 0, 0, 0, 0, 0]),
                12
            ),
            Err(Errno::InvalidArgument)
        );
        assert_eq!(
            ConsoleWriteRequest::from_syscall(
                SyscallRequest::new(1, [2, 0x40_0000, 12, 0, 0, 0]),
                12
            ),
            Err(Errno::BadFileDescriptor)
        );
        assert_eq!(
            ConsoleWriteRequest::from_syscall(
                SyscallRequest::new(1, [STANDARD_OUTPUT, 0x40_0000, 13, 0, 0, 0]),
                12
            ),
            Err(Errno::InvalidArgument)
        );
    }

    #[test]
    fn file_requests_validate_operations_modes_and_bounds() {
        let open = FileOpenRequest::from_syscall(
            SyscallRequest::new(3, [0x40_1000, 8, OPEN_READ_ONLY, 0, 0, 0]),
            8,
        )
        .unwrap();
        assert_eq!(open.path_buffer(), 0x40_1000);
        assert_eq!(open.path_length(), 8);
        assert_eq!(
            FileOpenRequest::from_syscall(
                SyscallRequest::new(3, [0, 0, OPEN_READ_ONLY, 0, 0, 0]),
                8
            ),
            Err(Errno::InvalidArgument)
        );
        assert_eq!(
            FileOpenRequest::from_syscall(SyscallRequest::new(3, [0, 4, 1, 0, 0, 0]), 8),
            Err(Errno::InvalidArgument)
        );

        let read =
            FileReadRequest::from_syscall(SyscallRequest::new(2, [3, 0x7f_f000, 64, 0, 0, 0]), 64)
                .unwrap();
        assert_eq!(read.descriptor(), 3);
        assert_eq!(read.user_buffer(), 0x7f_f000);
        assert_eq!(read.length(), 64);
        assert_eq!(
            FileReadRequest::from_syscall(SyscallRequest::new(2, [3, 0, 65, 0, 0, 0]), 64),
            Err(Errno::InvalidArgument)
        );

        let close =
            FileCloseRequest::from_syscall(SyscallRequest::new(4, [3, 0, 0, 0, 0, 0])).unwrap();
        assert_eq!(close.descriptor(), 3);
        assert_eq!(
            FileCloseRequest::from_syscall(SyscallRequest::new(2, [3, 0, 0, 0, 0, 0])),
            Err(Errno::InvalidArgument)
        );
    }
}
