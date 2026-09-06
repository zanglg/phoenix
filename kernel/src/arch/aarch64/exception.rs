//! Host-testable AArch64 exception layout and syndrome decoding.

use core::mem::{align_of, offset_of, size_of};

/// Architectural alignment of `VBAR_EL1`.
pub const VECTOR_TABLE_ALIGNMENT: usize = 2048;
/// Total size of an AArch64 exception vector table.
pub const VECTOR_TABLE_SIZE: usize = 2048;
/// Size reserved for one vector entry.
pub const VECTOR_ENTRY_SIZE: usize = 128;

/// Source execution state and stack selection represented by a vector group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum VectorOrigin {
    /// Exception taken from the current EL while using `SP_EL0`.
    CurrentSp0 = 0,
    /// Exception taken from the current EL while using `SP_ELx`.
    CurrentSpX = 1,
    /// Exception taken from a lower EL executing AArch64.
    LowerAArch64 = 2,
    /// Exception taken from a lower EL executing AArch32.
    LowerAArch32 = 3,
}

/// Architectural exception kind represented within every vector group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ExceptionKind {
    /// Synchronous exception.
    Synchronous = 0,
    /// IRQ interrupt.
    Irq = 1,
    /// FIQ interrupt.
    Fiq = 2,
    /// System error.
    SError = 3,
}

/// One of the 16 architecturally defined vector-table entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VectorSlot {
    origin: VectorOrigin,
    kind: ExceptionKind,
}

impl VectorSlot {
    /// All vector slots in increasing table-offset order.
    pub const ALL: [Self; 16] = [
        Self::new(VectorOrigin::CurrentSp0, ExceptionKind::Synchronous),
        Self::new(VectorOrigin::CurrentSp0, ExceptionKind::Irq),
        Self::new(VectorOrigin::CurrentSp0, ExceptionKind::Fiq),
        Self::new(VectorOrigin::CurrentSp0, ExceptionKind::SError),
        Self::new(VectorOrigin::CurrentSpX, ExceptionKind::Synchronous),
        Self::new(VectorOrigin::CurrentSpX, ExceptionKind::Irq),
        Self::new(VectorOrigin::CurrentSpX, ExceptionKind::Fiq),
        Self::new(VectorOrigin::CurrentSpX, ExceptionKind::SError),
        Self::new(VectorOrigin::LowerAArch64, ExceptionKind::Synchronous),
        Self::new(VectorOrigin::LowerAArch64, ExceptionKind::Irq),
        Self::new(VectorOrigin::LowerAArch64, ExceptionKind::Fiq),
        Self::new(VectorOrigin::LowerAArch64, ExceptionKind::SError),
        Self::new(VectorOrigin::LowerAArch32, ExceptionKind::Synchronous),
        Self::new(VectorOrigin::LowerAArch32, ExceptionKind::Irq),
        Self::new(VectorOrigin::LowerAArch32, ExceptionKind::Fiq),
        Self::new(VectorOrigin::LowerAArch32, ExceptionKind::SError),
    ];

    /// Construct a vector slot from its two architectural dimensions.
    pub const fn new(origin: VectorOrigin, kind: ExceptionKind) -> Self {
        Self { origin, kind }
    }

    /// Return the source execution state and stack selection.
    pub const fn origin(self) -> VectorOrigin {
        self.origin
    }

    /// Return the exception kind.
    pub const fn kind(self) -> ExceptionKind {
        self.kind
    }

    /// Return this slot's byte offset from `VBAR_EL1`.
    pub const fn offset(self) -> usize {
        self.origin as usize * 4 * VECTOR_ENTRY_SIZE + self.kind as usize * VECTOR_ENTRY_SIZE
    }

    /// Decode an aligned table offset.
    pub const fn from_offset(offset: usize) -> Option<Self> {
        if offset >= VECTOR_TABLE_SIZE || !offset.is_multiple_of(VECTOR_ENTRY_SIZE) {
            return None;
        }
        Some(Self::ALL[offset / VECTOR_ENTRY_SIZE])
    }
}

/// Register state saved by the planned common exception entry path.
///
/// The layout is a contract with future assembly. Changing it requires updating
/// the static assertions in this module and the exception design document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C, align(16))]
pub struct ExceptionFrame {
    general: [u64; 31],
    sp_el0: u64,
    elr_el1: u64,
    spsr_el1: u64,
    esr_el1: u64,
    far_el1: u64,
}

impl ExceptionFrame {
    /// Number of general-purpose registers saved in the frame.
    pub const GENERAL_REGISTER_COUNT: usize = 31;
    /// Byte offset of `x0`.
    pub const X0_OFFSET: usize = offset_of!(Self, general);
    /// Byte offset of `x30`.
    pub const X30_OFFSET: usize = Self::X0_OFFSET + 30 * size_of::<u64>();
    /// Byte offset of `SP_EL0`.
    pub const SP_EL0_OFFSET: usize = offset_of!(Self, sp_el0);
    /// Byte offset of `ELR_EL1`.
    pub const ELR_EL1_OFFSET: usize = offset_of!(Self, elr_el1);
    /// Byte offset of `SPSR_EL1`.
    pub const SPSR_EL1_OFFSET: usize = offset_of!(Self, spsr_el1);
    /// Byte offset of `ESR_EL1`.
    pub const ESR_EL1_OFFSET: usize = offset_of!(Self, esr_el1);
    /// Byte offset of `FAR_EL1`.
    pub const FAR_EL1_OFFSET: usize = offset_of!(Self, far_el1);
    /// Total frame size consumed on the exception stack.
    pub const SIZE: usize = size_of::<Self>();
    /// Required stack alignment.
    pub const ALIGNMENT: usize = align_of::<Self>();

    /// Construct a zeroed frame for initialization and tests.
    pub const fn zeroed() -> Self {
        Self {
            general: [0; 31],
            sp_el0: 0,
            elr_el1: 0,
            spsr_el1: 0,
            esr_el1: 0,
            far_el1: 0,
        }
    }

    /// Read a general-purpose register by architectural number.
    pub const fn general(&self, register: usize) -> Option<u64> {
        if register < Self::GENERAL_REGISTER_COUNT {
            Some(self.general[register])
        } else {
            None
        }
    }

    /// Update a general-purpose register by architectural number.
    pub fn set_general(&mut self, register: usize, value: u64) -> bool {
        if register >= Self::GENERAL_REGISTER_COUNT {
            return false;
        }
        self.general[register] = value;
        true
    }

    /// Return the interrupted `SP_EL0` value.
    pub const fn sp_el0(&self) -> u64 {
        self.sp_el0
    }

    /// Return the interrupted program counter.
    pub const fn program_counter(&self) -> u64 {
        self.elr_el1
    }

    /// Set the program counter restored by a future `eret`.
    pub fn set_program_counter(&mut self, address: u64) {
        self.elr_el1 = address;
    }

    /// Return the saved process state.
    pub const fn saved_program_status(&self) -> u64 {
        self.spsr_el1
    }

    /// Decode the saved exception syndrome.
    pub const fn syndrome(&self) -> Syndrome {
        Syndrome::from_raw(self.esr_el1)
    }

    /// Return the saved fault address.
    pub const fn fault_address(&self) -> u64 {
        self.far_el1
    }
}

/// Length reported for the trapped instruction by `ESR_EL1.IL`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstructionLength {
    /// 16-bit instruction.
    Bits16,
    /// 32-bit instruction.
    Bits32,
}

/// Decoded `ESR_EL1.EC` exception class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionClass {
    /// Unknown reason (`EC=0x00`).
    Unknown,
    /// Trapped `WFI` or `WFE`.
    TrappedWfiWfe,
    /// Trapped SIMD or floating-point instruction.
    TrappedSimdFloatingPoint,
    /// Illegal execution state.
    IllegalExecutionState,
    /// AArch64 supervisor call.
    SupervisorCallAArch64,
    /// Trapped AArch64 system-register access.
    TrappedSystemRegister,
    /// Instruction abort from a lower EL.
    InstructionAbortLower,
    /// Instruction abort from the current EL.
    InstructionAbortCurrent,
    /// Program-counter alignment fault.
    ProgramCounterAlignment,
    /// Data abort from a lower EL.
    DataAbortLower,
    /// Data abort from the current EL.
    DataAbortCurrent,
    /// Stack-pointer alignment fault.
    StackPointerAlignment,
    /// System error interrupt.
    SError,
    /// Hardware breakpoint from a lower EL.
    BreakpointLower,
    /// Hardware breakpoint from the current EL.
    BreakpointCurrent,
    /// Software step from a lower EL.
    SoftwareStepLower,
    /// Software step from the current EL.
    SoftwareStepCurrent,
    /// Watchpoint from a lower EL.
    WatchpointLower,
    /// Watchpoint from the current EL.
    WatchpointCurrent,
    /// AArch32 `BKPT` instruction.
    BreakpointInstructionAArch32,
    /// AArch64 `BRK` instruction.
    BreakpointInstructionAArch64,
    /// Class not yet interpreted by Phoenix; the raw six-bit value is retained.
    Other(u8),
}

impl ExceptionClass {
    const fn decode(value: u8) -> Self {
        match value {
            0x00 => Self::Unknown,
            0x01 => Self::TrappedWfiWfe,
            0x07 => Self::TrappedSimdFloatingPoint,
            0x0e => Self::IllegalExecutionState,
            0x15 => Self::SupervisorCallAArch64,
            0x18 => Self::TrappedSystemRegister,
            0x20 => Self::InstructionAbortLower,
            0x21 => Self::InstructionAbortCurrent,
            0x22 => Self::ProgramCounterAlignment,
            0x24 => Self::DataAbortLower,
            0x25 => Self::DataAbortCurrent,
            0x26 => Self::StackPointerAlignment,
            0x2f => Self::SError,
            0x30 => Self::BreakpointLower,
            0x31 => Self::BreakpointCurrent,
            0x32 => Self::SoftwareStepLower,
            0x33 => Self::SoftwareStepCurrent,
            0x34 => Self::WatchpointLower,
            0x35 => Self::WatchpointCurrent,
            0x38 => Self::BreakpointInstructionAArch32,
            0x3c => Self::BreakpointInstructionAArch64,
            value => Self::Other(value),
        }
    }
}

/// Raw and decoded view of `ESR_EL1`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Syndrome(u64);

impl Syndrome {
    const EC_SHIFT: u32 = 26;
    const EC_MASK: u64 = 0x3f;
    const IL_BIT: u64 = 1 << 25;
    const ISS_MASK: u64 = 0x01ff_ffff;

    /// Preserve a raw `ESR_EL1` value.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the unmodified register value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Return the raw six-bit exception class code.
    pub const fn exception_class_code(self) -> u8 {
        ((self.0 >> Self::EC_SHIFT) & Self::EC_MASK) as u8
    }

    /// Decode the exception class without discarding unknown values.
    pub const fn exception_class(self) -> ExceptionClass {
        ExceptionClass::decode(self.exception_class_code())
    }

    /// Decode the trapped instruction length.
    pub const fn instruction_length(self) -> InstructionLength {
        if self.0 & Self::IL_BIT == 0 {
            InstructionLength::Bits16
        } else {
            InstructionLength::Bits32
        }
    }

    /// Return the common 25-bit instruction-specific syndrome.
    pub const fn iss(self) -> u32 {
        (self.0 & Self::ISS_MASK) as u32
    }

    /// Return the immediate supplied by an AArch64 `SVC` instruction.
    pub const fn supervisor_call_immediate(self) -> Option<u16> {
        if matches!(
            self.exception_class(),
            ExceptionClass::SupervisorCallAArch64
        ) {
            Some((self.iss() & 0xffff) as u16)
        } else {
            None
        }
    }

    /// Decode a data-abort ISS or report the actual exception class.
    pub const fn data_abort(self) -> Result<DataAbortSyndrome, SyndromeDecodeError> {
        let class = self.exception_class();
        if !matches!(
            class,
            ExceptionClass::DataAbortLower | ExceptionClass::DataAbortCurrent
        ) {
            return Err(SyndromeDecodeError::WrongExceptionClass(class));
        }
        Ok(DataAbortSyndrome::decode(self.iss()))
    }
}

/// Failure to apply a class-specific syndrome decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyndromeDecodeError {
    /// The syndrome belongs to a different exception class.
    WrongExceptionClass(ExceptionClass),
}

/// Translation-table level named by an abort fault status code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FaultLevel {
    /// Level zero.
    Level0 = 0,
    /// Level one.
    Level1 = 1,
    /// Level two.
    Level2 = 2,
    /// Level three.
    Level3 = 3,
}

impl FaultLevel {
    const fn decode(value: u8) -> Self {
        match value & 0b11 {
            0 => Self::Level0,
            1 => Self::Level1,
            2 => Self::Level2,
            _ => Self::Level3,
        }
    }
}

/// Classified six-bit data fault status code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataFaultStatus {
    /// Address-size fault at the named level.
    AddressSize(FaultLevel),
    /// Translation fault at the named level.
    Translation(FaultLevel),
    /// Access-flag fault at the named level.
    AccessFlag(FaultLevel),
    /// Permission fault at the named level.
    Permission(FaultLevel),
    /// Alignment fault.
    Alignment,
    /// Status not yet classified; the raw six-bit code is retained.
    Other(u8),
}

impl DataFaultStatus {
    const fn decode(value: u8) -> Self {
        match value {
            0b100001 => Self::Alignment,
            value if value & 0b111100 == 0b000000 => Self::AddressSize(FaultLevel::decode(value)),
            value if value & 0b111100 == 0b000100 => Self::Translation(FaultLevel::decode(value)),
            value if value & 0b111100 == 0b001000 => Self::AccessFlag(FaultLevel::decode(value)),
            value if value & 0b111100 == 0b001100 => Self::Permission(FaultLevel::decode(value)),
            value => Self::Other(value),
        }
    }
}

/// Transfer width described by a valid data-abort access syndrome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessSize {
    /// One byte.
    Byte,
    /// Two bytes.
    HalfWord,
    /// Four bytes.
    Word,
    /// Eight bytes.
    DoubleWord,
}

impl AccessSize {
    const fn decode(value: u32) -> Self {
        match value & 0b11 {
            0 => Self::Byte,
            1 => Self::HalfWord,
            2 => Self::Word,
            _ => Self::DoubleWord,
        }
    }
}

/// Register-transfer information present when `ISS.ISV` is set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataAbortAccess {
    /// Size of the faulting transfer.
    pub size: AccessSize,
    /// Whether a load result would be sign-extended.
    pub sign_extend: bool,
    /// Architectural transfer register number.
    pub register: u8,
    /// Whether the transfer uses a 64-bit register.
    pub register_is_64_bit: bool,
    /// Whether acquire/release semantics were requested.
    pub acquire_release: bool,
}

/// Decoded common fields of a data-abort instruction-specific syndrome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataAbortSyndrome {
    raw_iss: u32,
    fault_status: DataFaultStatus,
    write: bool,
    stage_one_page_table_walk: bool,
    cache_maintenance: bool,
    external_abort: bool,
    far_not_valid: bool,
    access: Option<DataAbortAccess>,
}

impl DataAbortSyndrome {
    const fn decode(iss: u32) -> Self {
        let access = if iss & (1 << 24) != 0 {
            Some(DataAbortAccess {
                size: AccessSize::decode(iss >> 22),
                sign_extend: iss & (1 << 21) != 0,
                register: ((iss >> 16) & 0x1f) as u8,
                register_is_64_bit: iss & (1 << 15) != 0,
                acquire_release: iss & (1 << 14) != 0,
            })
        } else {
            None
        };
        Self {
            raw_iss: iss,
            fault_status: DataFaultStatus::decode((iss & 0x3f) as u8),
            write: iss & (1 << 6) != 0,
            stage_one_page_table_walk: iss & (1 << 7) != 0,
            cache_maintenance: iss & (1 << 8) != 0,
            external_abort: iss & (1 << 9) != 0,
            far_not_valid: iss & (1 << 10) != 0,
            access,
        }
    }

    /// Return the original 25-bit ISS.
    pub const fn raw_iss(self) -> u32 {
        self.raw_iss
    }

    /// Return the classified data fault status.
    pub const fn fault_status(self) -> DataFaultStatus {
        self.fault_status
    }

    /// Return whether the faulting access was a write.
    pub const fn is_write(self) -> bool {
        self.write
    }

    /// Return whether the fault occurred during a stage-1 table walk.
    pub const fn during_stage_one_page_table_walk(self) -> bool {
        self.stage_one_page_table_walk
    }

    /// Return whether the fault arose from cache maintenance.
    pub const fn during_cache_maintenance(self) -> bool {
        self.cache_maintenance
    }

    /// Return the external-abort indicator.
    pub const fn is_external_abort(self) -> bool {
        self.external_abort
    }

    /// Return whether `FAR_EL1` is invalid for this abort.
    pub const fn far_not_valid(self) -> bool {
        self.far_not_valid
    }

    /// Return transfer details when the architectural syndrome marks them valid.
    pub const fn access(self) -> Option<DataAbortAccess> {
        self.access
    }
}

const _: () = {
    assert!(VECTOR_TABLE_SIZE == 16 * VECTOR_ENTRY_SIZE);
    assert!(ExceptionFrame::X0_OFFSET == 0);
    assert!(ExceptionFrame::X30_OFFSET == 240);
    assert!(ExceptionFrame::SP_EL0_OFFSET == 248);
    assert!(ExceptionFrame::ELR_EL1_OFFSET == 256);
    assert!(ExceptionFrame::SPSR_EL1_OFFSET == 264);
    assert!(ExceptionFrame::ESR_EL1_OFFSET == 272);
    assert!(ExceptionFrame::FAR_EL1_OFFSET == 280);
    assert!(ExceptionFrame::SIZE == 288);
    assert!(ExceptionFrame::ALIGNMENT == 16);
};

#[cfg(test)]
mod tests {
    use super::{
        AccessSize, DataAbortAccess, DataFaultStatus, ExceptionClass, ExceptionFrame,
        ExceptionKind, FaultLevel, InstructionLength, Syndrome, SyndromeDecodeError,
        VECTOR_ENTRY_SIZE, VECTOR_TABLE_ALIGNMENT, VECTOR_TABLE_SIZE, VectorOrigin, VectorSlot,
    };

    #[test]
    fn all_vector_slots_cover_the_table_in_architectural_order() {
        assert_eq!(VECTOR_TABLE_ALIGNMENT, 2048);
        assert_eq!(VECTOR_TABLE_SIZE, 2048);
        for (index, slot) in VectorSlot::ALL.into_iter().enumerate() {
            assert_eq!(slot.offset(), index * VECTOR_ENTRY_SIZE);
            assert_eq!(VectorSlot::from_offset(slot.offset()), Some(slot));
        }
        assert_eq!(VectorSlot::from_offset(1), None);
        assert_eq!(VectorSlot::from_offset(VECTOR_TABLE_SIZE), None);
        assert_eq!(
            VectorSlot::from_offset(0x480),
            Some(VectorSlot::new(
                VectorOrigin::LowerAArch64,
                ExceptionKind::Irq
            ))
        );
    }

    #[test]
    fn exception_frame_layout_is_stable_and_mutation_is_checked() {
        assert_eq!(ExceptionFrame::SIZE, 288);
        assert_eq!(ExceptionFrame::ALIGNMENT, 16);
        assert_eq!(ExceptionFrame::X30_OFFSET, 240);
        assert_eq!(ExceptionFrame::FAR_EL1_OFFSET, 280);

        let mut frame = ExceptionFrame::zeroed();
        assert!(frame.set_general(30, 0xfeed));
        assert!(!frame.set_general(31, 1));
        frame.set_program_counter(0x4008_1234);
        assert_eq!(frame.general(30), Some(0xfeed));
        assert_eq!(frame.general(31), None);
        assert_eq!(frame.program_counter(), 0x4008_1234);
    }

    #[test]
    fn syndrome_preserves_unknown_classes_and_common_fields() {
        let raw = (0x2a_u64 << 26) | (1 << 25) | 0x12_3456;
        let syndrome = Syndrome::from_raw(raw);

        assert_eq!(syndrome.raw(), raw);
        assert_eq!(syndrome.exception_class_code(), 0x2a);
        assert_eq!(syndrome.exception_class(), ExceptionClass::Other(0x2a));
        assert_eq!(syndrome.instruction_length(), InstructionLength::Bits32);
        assert_eq!(syndrome.iss(), 0x12_3456);
    }

    #[test]
    fn decodes_supervisor_call_immediate_only_for_aarch64_svc() {
        let svc = Syndrome::from_raw((0x15_u64 << 26) | (1 << 25) | 0xbeef);
        let other = Syndrome::from_raw((0x3c_u64 << 26) | 0xbeef);

        assert_eq!(svc.exception_class(), ExceptionClass::SupervisorCallAArch64);
        assert_eq!(svc.supervisor_call_immediate(), Some(0xbeef));
        assert_eq!(other.supervisor_call_immediate(), None);
    }

    #[test]
    fn decodes_data_abort_status_and_access_information() {
        let iss = (1_u64 << 24)
            | (0b11 << 22)
            | (1 << 21)
            | (7 << 16)
            | (1 << 15)
            | (1 << 14)
            | (1 << 10)
            | (1 << 9)
            | (1 << 8)
            | (1 << 7)
            | (1 << 6)
            | 0b001110;
        let abort = Syndrome::from_raw((0x24_u64 << 26) | (1 << 25) | iss)
            .data_abort()
            .expect("lower-EL data abort");

        assert_eq!(
            abort.fault_status(),
            DataFaultStatus::Permission(FaultLevel::Level2)
        );
        assert!(abort.is_write());
        assert!(abort.during_stage_one_page_table_walk());
        assert!(abort.during_cache_maintenance());
        assert!(abort.is_external_abort());
        assert!(abort.far_not_valid());
        assert_eq!(
            abort.access(),
            Some(DataAbortAccess {
                size: AccessSize::DoubleWord,
                sign_extend: true,
                register: 7,
                register_is_64_bit: true,
                acquire_release: true,
            })
        );
    }

    #[test]
    fn data_abort_decoder_rejects_another_exception_class() {
        let syndrome = Syndrome::from_raw(0x20_u64 << 26);
        assert_eq!(
            syndrome.data_abort(),
            Err(SyndromeDecodeError::WrongExceptionClass(
                ExceptionClass::InstructionAbortLower
            ))
        );
    }

    #[test]
    fn fault_status_decoder_retains_unclassified_codes() {
        let cases = [
            (0b000000, DataFaultStatus::AddressSize(FaultLevel::Level0)),
            (0b000111, DataFaultStatus::Translation(FaultLevel::Level3)),
            (0b001001, DataFaultStatus::AccessFlag(FaultLevel::Level1)),
            (0b001100, DataFaultStatus::Permission(FaultLevel::Level0)),
            (0b100001, DataFaultStatus::Alignment),
            (0b110000, DataFaultStatus::Other(0b110000)),
        ];
        for (status, expected) in cases {
            let syndrome = Syndrome::from_raw((0x25_u64 << 26) | status);
            assert_eq!(syndrome.data_abort().unwrap().fault_status(), expected);
        }
    }
}
