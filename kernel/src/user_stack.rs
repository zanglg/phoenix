//! Allocation-free construction of the initial native user stack image.

use crate::memory::PAGE_SIZE;
use crate::user::{UserAddr, UserStackLayout};

/// Auxiliary-vector key terminating the vector.
pub const AUX_NULL: u64 = 0;
/// Auxiliary-vector key carrying the base page size.
pub const AUX_PAGE_SIZE: u64 = 6;
/// Auxiliary-vector key carrying the executable entry address.
pub const AUX_ENTRY: u64 = 9;
/// Phoenix-private auxiliary-vector key carrying the native ABI revision.
pub const AUX_PHOENIX_ABI_REVISION: u64 = 0x5000;
/// Native ABI revision encoded in the initial user stack.
pub const NATIVE_ABI_REVISION: u64 = 0;

const STACK_ALIGNMENT: usize = 16;
const WORD_SIZE: usize = size_of::<u64>();
const AUXILIARY_ENTRY_COUNT: usize = 4;

/// Kind of string rejected while constructing an initial stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialStackStringKind {
    /// One entry in `argv`.
    Argument,
    /// One entry in `envp`.
    Environment,
}

/// Failure while constructing an initial user stack image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialStackError {
    /// Native process startup requires at least `argv[0]`.
    MissingProgramName,
    /// A string contains an interior zero byte and cannot be C-terminated.
    InteriorNul {
        /// Whether the rejected value was an argument or environment entry.
        kind: InitialStackStringKind,
        /// Zero-based index within its input list.
        index: usize,
    },
    /// Size or address arithmetic overflowed.
    ArithmeticOverflow,
    /// The fixed output buffer cannot hold the complete initialized stack.
    CapacityExceeded {
        /// Bytes required after final stack-pointer alignment.
        required: usize,
        /// Bytes available in the fixed output buffer.
        capacity: usize,
    },
    /// The guarded stack mapping is smaller than the complete initialized stack.
    StackTooSmall {
        /// Bytes required after final stack-pointer alignment.
        required: usize,
        /// Bytes mapped as usable stack.
        available: usize,
    },
}

/// Owned bytes and virtual addresses for one initial native process stack.
///
/// Byte zero corresponds to `stack_pointer`; `bytes()` extends upward to the
/// original aligned top of the supplied `UserStackLayout`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitialStackImage<const CAPACITY: usize> {
    layout: UserStackLayout,
    stack_pointer: UserAddr,
    bytes: [u8; CAPACITY],
    len: usize,
    argument_count: usize,
    environment_count: usize,
}

impl<const CAPACITY: usize> InitialStackImage<CAPACITY> {
    /// Build `argc`, `argv`, `envp`, the native auxiliary vector, and strings.
    pub fn new(
        layout: UserStackLayout,
        entry: UserAddr,
        arguments: &[&str],
        environment: &[&str],
    ) -> Result<Self, InitialStackError> {
        if arguments.is_empty() {
            return Err(InitialStackError::MissingProgramName);
        }
        let argument_bytes = validate_and_measure(arguments, InitialStackStringKind::Argument)?;
        let environment_bytes =
            validate_and_measure(environment, InitialStackStringKind::Environment)?;
        let string_bytes = argument_bytes
            .checked_add(environment_bytes)
            .ok_or(InitialStackError::ArithmeticOverflow)?;
        let pointer_words = arguments
            .len()
            .checked_add(environment.len())
            .and_then(|count| count.checked_add(3))
            .ok_or(InitialStackError::ArithmeticOverflow)?;
        let auxiliary_words = AUXILIARY_ENTRY_COUNT
            .checked_mul(2)
            .ok_or(InitialStackError::ArithmeticOverflow)?;
        let metadata_bytes = pointer_words
            .checked_add(auxiliary_words)
            .and_then(|words| words.checked_mul(WORD_SIZE))
            .ok_or(InitialStackError::ArithmeticOverflow)?;
        let unaligned_size = metadata_bytes
            .checked_add(string_bytes)
            .ok_or(InitialStackError::ArithmeticOverflow)?;
        let required = align_up(unaligned_size, STACK_ALIGNMENT)?;

        if required > CAPACITY {
            return Err(InitialStackError::CapacityExceeded {
                required,
                capacity: CAPACITY,
            });
        }
        let available = layout.usable_range().byte_len();
        if required > available {
            return Err(InitialStackError::StackTooSmall {
                required,
                available,
            });
        }
        let top = layout.initial_stack_pointer().as_usize();
        let stack_pointer_raw = top
            .checked_sub(required)
            .ok_or(InitialStackError::ArithmeticOverflow)?;
        let stack_pointer =
            UserAddr::new(stack_pointer_raw).map_err(|_| InitialStackError::ArithmeticOverflow)?;

        let mut result = Self {
            layout,
            stack_pointer,
            bytes: [0; CAPACITY],
            len: required,
            argument_count: arguments.len(),
            environment_count: environment.len(),
        };
        let string_offset = required - string_bytes;
        let string_virtual_start = stack_pointer_raw
            .checked_add(string_offset)
            .ok_or(InitialStackError::ArithmeticOverflow)?;
        let mut metadata_offset = 0;
        let mut next_string_offset = string_offset;
        let mut next_string_virtual = string_virtual_start;

        result.write_word(&mut metadata_offset, arguments.len() as u64);
        for argument in arguments {
            result.write_word(&mut metadata_offset, next_string_virtual as u64);
            result.write_string(&mut next_string_offset, &mut next_string_virtual, argument);
        }
        result.write_word(&mut metadata_offset, 0);
        for value in environment {
            result.write_word(&mut metadata_offset, next_string_virtual as u64);
            result.write_string(&mut next_string_offset, &mut next_string_virtual, value);
        }
        result.write_word(&mut metadata_offset, 0);
        for (key, value) in [
            (AUX_PAGE_SIZE, PAGE_SIZE as u64),
            (AUX_ENTRY, entry.as_usize() as u64),
            (AUX_PHOENIX_ABI_REVISION, NATIVE_ABI_REVISION),
            (AUX_NULL, 0),
        ] {
            result.write_word(&mut metadata_offset, key);
            result.write_word(&mut metadata_offset, value);
        }

        debug_assert_eq!(metadata_offset, metadata_bytes);
        debug_assert_eq!(next_string_offset, required);
        debug_assert_eq!(next_string_virtual, top);
        Ok(result)
    }

    /// Return the guarded stack mapping this image was built for.
    pub const fn layout(&self) -> UserStackLayout {
        self.layout
    }

    /// Return the 16-byte-aligned stack pointer passed to the program entry.
    pub const fn stack_pointer(&self) -> UserAddr {
        self.stack_pointer
    }

    /// Return initialized bytes corresponding to `[stack_pointer, stack_top)`.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// Return the value encoded as `argc`.
    pub const fn argument_count(&self) -> usize {
        self.argument_count
    }

    /// Return the number of pointers encoded before the `envp` terminator.
    pub const fn environment_count(&self) -> usize {
        self.environment_count
    }

    fn write_word(&mut self, offset: &mut usize, value: u64) {
        let end = *offset + WORD_SIZE;
        self.bytes[*offset..end].copy_from_slice(&value.to_le_bytes());
        *offset = end;
    }

    fn write_string(&mut self, offset: &mut usize, virtual_address: &mut usize, value: &str) {
        let end = *offset + value.len();
        self.bytes[*offset..end].copy_from_slice(value.as_bytes());
        self.bytes[end] = 0;
        *offset = end + 1;
        *virtual_address += value.len() + 1;
    }
}

fn validate_and_measure(
    values: &[&str],
    kind: InitialStackStringKind,
) -> Result<usize, InitialStackError> {
    let mut total = 0_usize;
    for (index, value) in values.iter().enumerate() {
        if value.as_bytes().contains(&0) {
            return Err(InitialStackError::InteriorNul { kind, index });
        }
        total = total
            .checked_add(value.len())
            .and_then(|size| size.checked_add(1))
            .ok_or(InitialStackError::ArithmeticOverflow)?;
    }
    Ok(total)
}

fn align_up(value: usize, alignment: usize) -> Result<usize, InitialStackError> {
    value
        .checked_add(alignment - 1)
        .map(|rounded| rounded & !(alignment - 1))
        .ok_or(InitialStackError::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
    use super::{
        AUX_ENTRY, AUX_NULL, AUX_PAGE_SIZE, AUX_PHOENIX_ABI_REVISION, InitialStackError,
        InitialStackImage, InitialStackStringKind, NATIVE_ABI_REVISION,
    };
    use crate::memory::PAGE_SIZE;
    use crate::user::{UserAddr, UserStackLayout};

    #[test]
    fn builds_aligned_native_stack_with_exact_pointers_and_auxiliary_vector() {
        let layout = stack(0x80_0000, 2);
        let entry = user(0x40_0000);
        let image =
            InitialStackImage::<512>::new(layout, entry, &["/init", "hello"], &["TERM=phoenix"])
                .unwrap();

        assert_eq!(image.stack_pointer().as_usize() & 0xf, 0);
        assert_eq!(image.argument_count(), 2);
        assert_eq!(image.environment_count(), 1);
        let bytes = image.bytes();
        assert_eq!(word(bytes, 0), 2);
        let argv0 = word(bytes, 1) as usize;
        let argv1 = word(bytes, 2) as usize;
        assert_eq!(word(bytes, 3), 0);
        let env0 = word(bytes, 4) as usize;
        assert_eq!(word(bytes, 5), 0);
        assert_eq!((word(bytes, 6), word(bytes, 7)), (AUX_PAGE_SIZE, 4096));
        assert_eq!(
            (word(bytes, 8), word(bytes, 9)),
            (AUX_ENTRY, entry.as_usize() as u64)
        );
        assert_eq!(
            (word(bytes, 10), word(bytes, 11)),
            (AUX_PHOENIX_ABI_REVISION, NATIVE_ABI_REVISION)
        );
        assert_eq!((word(bytes, 12), word(bytes, 13)), (AUX_NULL, 0));
        assert_eq!(cstring(image.stack_pointer(), bytes, argv0), b"/init");
        assert_eq!(cstring(image.stack_pointer(), bytes, argv1), b"hello");
        assert_eq!(cstring(image.stack_pointer(), bytes, env0), b"TERM=phoenix");
    }

    #[test]
    fn rejects_missing_program_name_nul_capacity_and_stack_exhaustion() {
        let layout = stack(0x80_0000, 1);
        assert_eq!(
            InitialStackImage::<128>::new(layout, user(0x40_0000), &[], &[]),
            Err(InitialStackError::MissingProgramName)
        );
        assert_eq!(
            InitialStackImage::<128>::new(layout, user(0x40_0000), &["bad\0arg"], &[]),
            Err(InitialStackError::InteriorNul {
                kind: InitialStackStringKind::Argument,
                index: 0,
            })
        );
        assert!(matches!(
            InitialStackImage::<16>::new(layout, user(0x40_0000), &["/init"], &[]),
            Err(InitialStackError::CapacityExceeded { .. })
        ));
        let huge = "x".repeat(PAGE_SIZE);
        assert!(matches!(
            InitialStackImage::<8192>::new(layout, user(0x40_0000), &[&huge], &[]),
            Err(InitialStackError::StackTooSmall { .. })
        ));
    }

    fn word(bytes: &[u8], index: usize) -> u64 {
        let start = index * 8;
        u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap())
    }

    fn cstring(stack_pointer: UserAddr, bytes: &[u8], address: usize) -> &[u8] {
        let start = address - stack_pointer.as_usize();
        let end = bytes[start..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|length| start + length)
            .unwrap();
        &bytes[start..end]
    }

    fn stack(top: usize, pages: usize) -> UserStackLayout {
        UserStackLayout::new(user(top), pages, 1).unwrap()
    }

    fn user(address: usize) -> UserAddr {
        UserAddr::new(address).unwrap()
    }
}
