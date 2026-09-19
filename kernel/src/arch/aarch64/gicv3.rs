//! Host-testable GICv3 private-interrupt preparation state machine.

use core::num::NonZeroU32;

const GICD_CTLR: usize = 0x0000;
const GICD_PIDR2: usize = 0xffe8;
const GICD_CTLR_ENABLE_G1: u32 = 1 << 0;
const GICD_CTLR_ENABLE_G1A: u32 = 1 << 1;
const GICD_CTLR_ARE_NS: u32 = 1 << 4;
const GICD_CTLR_RWP: u32 = 1 << 31;

const GICR_CTLR: usize = 0x0000;
const GICR_WAKER: usize = 0x0014;
const GICR_PIDR2: usize = 0xffe8;
const GICR_SGI_BASE: usize = 0x1_0000;
const GICR_IGROUPR0: usize = GICR_SGI_BASE + 0x0080;
const GICR_ISENABLER0: usize = GICR_SGI_BASE + 0x0100;
const GICR_ICENABLER0: usize = GICR_SGI_BASE + 0x0180;
const GICR_ICPENDR0: usize = GICR_SGI_BASE + 0x0280;
const GICR_ICACTIVER0: usize = GICR_SGI_BASE + 0x0380;
const GICR_IPRIORITYR: usize = GICR_SGI_BASE + 0x0400;
const GICR_ICFGR1: usize = GICR_SGI_BASE + 0x0c04;
const GICR_IGRPMODR0: usize = GICR_SGI_BASE + 0x0d00;
const GICR_CTLR_RWP: u32 = 1 << 3;
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

const GIC_PIDR2_ARCHITECTURE_SHIFT: u32 = 4;
const GIC_PIDR2_ARCHITECTURE_MASK: u32 = 0x0f;
const GIC_ARCHITECTURE_V3: u8 = 3;
const GIC_ARCHITECTURE_V4: u8 = 4;

const ICC_SRE_EL1_SRE: u64 = 1 << 0;
const ICC_SRE_EL1_DFB: u64 = 1 << 1;
const ICC_SRE_EL1_DIB: u64 = 1 << 2;
const ICC_CTLR_EL1_EOI_MODE: u64 = 1 << 1;
const DEFAULT_PRIORITY: u8 = 0xa0;
const PRIORITY_MASK_ALLOW_ALL: u64 = 0xff;
const PRIVATE_INTERRUPT_MASK: u32 = u32::MAX;

/// Lowest architectural private peripheral interrupt ID.
pub const PPI_INTID_MIN: u16 = 16;
/// Highest architectural private peripheral interrupt ID.
pub const PPI_INTID_MAX: u16 = 31;

/// Trigger mode supported for a standard private peripheral interrupt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptTrigger {
    /// Rising-edge triggered interrupt (`0b10` in `GICR_ICFGR1`).
    EdgeRising,
    /// Active-high level-triggered interrupt (`0b00` in `GICR_ICFGR1`).
    LevelHigh,
}

/// Invalid architectural PPI identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateInterruptError {
    /// The supplied INTID is outside `16..=31`.
    OutOfRange {
        /// Rejected architectural INTID.
        intid: u16,
    },
}

/// One standard GIC private peripheral interrupt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivatePeripheralInterrupt {
    intid: u16,
    trigger: InterruptTrigger,
}

impl PrivatePeripheralInterrupt {
    /// Validate one architectural PPI INTID and trigger.
    pub const fn new(intid: u16, trigger: InterruptTrigger) -> Result<Self, PrivateInterruptError> {
        if intid < PPI_INTID_MIN || intid > PPI_INTID_MAX {
            return Err(PrivateInterruptError::OutOfRange { intid });
        }
        Ok(Self { intid, trigger })
    }

    /// Return the architectural interrupt identifier.
    pub const fn intid(self) -> u16 {
        self.intid
    }

    /// Return the configured trigger mode.
    pub const fn trigger(self) -> InterruptTrigger {
        self.trigger
    }
}

/// CPU-interface system register used by the preparation sequence.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CpuInterfaceRegister {
    /// System-register interface enable and bypass controls.
    Sre,
    /// Priority mask.
    PriorityMask,
    /// Group-1 binary point.
    BinaryPoint1,
    /// Group-1 enable for the current security state.
    GroupEnable1,
    /// CPU-interface control, including EOI mode.
    Control,
}

/// Exclusive access to one GIC distributor, one selected redistributor, and
/// the current CPU's GICv3 system-register interface.
///
/// # Safety
///
/// Implementations must perform ordered accesses to the exact GICD/GICR
/// frames and current-CPU `ICC_*_EL1` registers they uniquely own. MMIO reads
/// and writes must be volatile, correctly sized, and unable to alias ordinary
/// memory. `synchronize` must complete preceding device/system-register writes
/// before later polling or state publication. No interrupt handler or other
/// CPU may access the same mutable controller state concurrently.
pub unsafe trait GicV3RegisterAccess {
    /// Backend-specific access failure.
    type Error;

    /// Read one 32-bit distributor register.
    fn read_distributor(&mut self, offset: usize) -> Result<u32, Self::Error>;

    /// Write one 32-bit distributor register.
    fn write_distributor(&mut self, offset: usize, value: u32) -> Result<(), Self::Error>;

    /// Read one 32-bit register relative to the selected GICR RD frame.
    fn read_redistributor(&mut self, offset: usize) -> Result<u32, Self::Error>;

    /// Write one 32-bit register relative to the selected GICR RD frame.
    fn write_redistributor(&mut self, offset: usize, value: u32) -> Result<(), Self::Error>;

    /// Read one current-CPU system register.
    fn read_cpu_interface(&mut self, register: CpuInterfaceRegister) -> Result<u64, Self::Error>;

    /// Write one current-CPU system register.
    fn write_cpu_interface(
        &mut self,
        register: CpuInterfaceRegister,
        value: u64,
    ) -> Result<(), Self::Error>;

    /// Complete earlier register writes before subsequent observations.
    fn synchronize(&mut self);
}

/// Configuration for preparing one boot-CPU timer PPI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicV3Configuration {
    timer: PrivatePeripheralInterrupt,
    priority: u8,
    poll_budget: NonZeroU32,
}

/// Invalid GICv3 preparation configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GicV3ConfigurationError {
    /// Priority `0xff` is blocked by the prepared `ICC_PMR_EL1` value.
    PermanentlyMaskedPriority,
}

impl GicV3Configuration {
    /// Create a configuration with Phoenix's initial interrupt priority.
    pub const fn new(timer: PrivatePeripheralInterrupt, poll_budget: NonZeroU32) -> Self {
        Self {
            timer,
            priority: DEFAULT_PRIORITY,
            poll_budget,
        }
    }

    /// Override the timer priority byte.
    pub const fn with_priority(mut self, priority: u8) -> Result<Self, GicV3ConfigurationError> {
        if priority == u8::MAX {
            return Err(GicV3ConfigurationError::PermanentlyMaskedPriority);
        }
        self.priority = priority;
        Ok(self)
    }

    /// Return the timer PPI.
    pub const fn timer(self) -> PrivatePeripheralInterrupt {
        self.timer
    }

    /// Return the exact priority byte written to `GICR_IPRIORITYR`.
    pub const fn priority(self) -> u8 {
        self.priority
    }

    /// Return the maximum number of reads in each hardware-state poll.
    pub const fn poll_budget(self) -> NonZeroU32 {
        self.poll_budget
    }
}

/// Exact preparation phase associated with a failure or timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GicV3PreparationStage {
    /// Verify GICD PIDR2.
    VerifyDistributor,
    /// Verify the selected GICR PIDR2.
    VerifyRedistributor,
    /// Enable and validate the current CPU's system-register interface.
    EnableSystemRegisters,
    /// Disable CPU-interface Group-1 delivery.
    QuiesceCpuInterface,
    /// Disable distributor groups and enable affinity routing.
    ConfigureDistributor,
    /// Wake the selected redistributor.
    WakeRedistributor,
    /// Disable, clear, group, prioritize, configure, and enable the timer PPI.
    ConfigurePrivateInterrupts,
    /// Configure priority masking, binary point, and combined EOI/deactivation.
    ConfigureCpuInterface,
    /// Best-effort failure recovery.
    Recovery,
}

/// Failure while preparing a GICv3 with delivery kept disabled.
#[derive(Debug, Eq, PartialEq)]
pub enum GicV3PreparationError<E> {
    /// One backend register operation failed.
    Access {
        /// Operation phase.
        stage: GicV3PreparationStage,
        /// Backend-specific failure.
        error: E,
    },
    /// A write-pending or wake transition exceeded its finite poll budget.
    Timeout {
        /// Timed-out phase.
        stage: GicV3PreparationStage,
    },
    /// PIDR2 does not report a GICv3/v4 programming model.
    UnsupportedArchitecture {
        /// `GICD` or `GICR`.
        component: &'static str,
        /// Observed four-bit architecture revision.
        revision: u8,
    },
    /// `ICC_SRE_EL1.SRE` remained clear after the enabling write.
    SystemRegisterInterfaceUnavailable {
        /// Value read back from `ICC_SRE_EL1`.
        observed: u64,
    },
    /// `GICD_CTLR.ARE_NS` remained clear after the enabling write.
    AffinityRoutingUnavailable {
        /// Value read back from `GICD_CTLR`.
        observed: u32,
    },
}

/// Failed preparation retaining unique backend ownership and recovery evidence.
#[derive(Debug)]
pub struct GicV3PreparationFailure<B: GicV3RegisterAccess> {
    backend: B,
    error: GicV3PreparationError<B::Error>,
    recovery_error: Option<GicV3PreparationError<B::Error>>,
}

/// Backend ownership plus primary and recovery failures from preparation.
pub type GicV3PreparationFailureParts<B> = (
    B,
    GicV3PreparationError<<B as GicV3RegisterAccess>::Error>,
    Option<GicV3PreparationError<<B as GicV3RegisterAccess>::Error>>,
);

impl<B: GicV3RegisterAccess> GicV3PreparationFailure<B> {
    /// Return the failure that stopped preparation.
    pub const fn error(&self) -> &GicV3PreparationError<B::Error> {
        &self.error
    }

    /// Return the first failure from best-effort delivery shutdown, if any.
    pub const fn recovery_error(&self) -> Option<&GicV3PreparationError<B::Error>> {
        self.recovery_error.as_ref()
    }

    /// Recover backend ownership plus both pieces of failure evidence.
    ///
    /// The backend may describe partially programmed hardware, especially when
    /// `recovery_error` is present. IRQs must remain externally masked.
    pub fn into_parts(self) -> GicV3PreparationFailureParts<B> {
        (self.backend, self.error, self.recovery_error)
    }
}

/// GICv3 private-interrupt state prepared with all delivery gates disabled.
#[derive(Debug)]
pub struct PreparedGicV3<B> {
    backend: B,
    configuration: GicV3Configuration,
}

impl<B> PreparedGicV3<B> {
    /// Return the retained timer configuration.
    pub const fn configuration(&self) -> GicV3Configuration {
        self.configuration
    }

    /// Borrow the backend for read-only diagnostics.
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// Consume the prepared state and return the uniquely owned backend.
    ///
    /// Delivery remains disabled, but private interrupt registers have been
    /// reconfigured and affinity routing remains enabled.
    pub fn into_backend(self) -> B {
        self.backend
    }
}

/// Prepare one selected redistributor and timer PPI without enabling delivery.
///
/// # Safety
///
/// The caller must keep `PSTATE.I` masked, uniquely own `backend`, and ensure
/// that no interrupt handler can access the controller throughout the call.
/// The backend's GICR frame must belong to the current boot CPU. Higher
/// exception levels must permit `ICC_SRE_EL1` access. Success deliberately
/// leaves both `GICD_CTLR` group enables and `ICC_IGRPEN1_EL1` clear.
pub unsafe fn prepare<B: GicV3RegisterAccess>(
    backend: B,
    configuration: GicV3Configuration,
) -> Result<PreparedGicV3<B>, GicV3PreparationFailure<B>> {
    let mut owner = Preparation {
        backend,
        configuration,
    };
    match owner.run() {
        Ok(()) => Ok(PreparedGicV3 {
            backend: owner.backend,
            configuration,
        }),
        Err(error) => {
            let recovery_error = owner.recover();
            Err(GicV3PreparationFailure {
                backend: owner.backend,
                error,
                recovery_error,
            })
        }
    }
}

struct Preparation<B> {
    backend: B,
    configuration: GicV3Configuration,
}

impl<B: GicV3RegisterAccess> Preparation<B> {
    fn run(&mut self) -> Result<(), GicV3PreparationError<B::Error>> {
        self.verify_architecture("GICD", GicV3PreparationStage::VerifyDistributor, true)?;
        self.verify_architecture("GICR", GicV3PreparationStage::VerifyRedistributor, false)?;
        self.enable_system_registers()?;
        self.write_cpu(
            GicV3PreparationStage::QuiesceCpuInterface,
            CpuInterfaceRegister::GroupEnable1,
            0,
        )?;
        self.write_cpu(
            GicV3PreparationStage::QuiesceCpuInterface,
            CpuInterfaceRegister::PriorityMask,
            0,
        )?;
        self.backend.synchronize();
        self.configure_distributor()?;
        self.wake_redistributor()?;
        self.configure_private_interrupts()?;
        self.configure_cpu_interface()?;
        Ok(())
    }

    fn verify_architecture(
        &mut self,
        component: &'static str,
        stage: GicV3PreparationStage,
        distributor: bool,
    ) -> Result<(), GicV3PreparationError<B::Error>> {
        let raw = if distributor {
            self.read_distributor(stage, GICD_PIDR2)?
        } else {
            self.read_redistributor(stage, GICR_PIDR2)?
        };
        let revision = ((raw >> GIC_PIDR2_ARCHITECTURE_SHIFT) & GIC_PIDR2_ARCHITECTURE_MASK) as u8;
        if matches!(revision, GIC_ARCHITECTURE_V3 | GIC_ARCHITECTURE_V4) {
            Ok(())
        } else {
            Err(GicV3PreparationError::UnsupportedArchitecture {
                component,
                revision,
            })
        }
    }

    fn enable_system_registers(&mut self) -> Result<(), GicV3PreparationError<B::Error>> {
        let stage = GicV3PreparationStage::EnableSystemRegisters;
        let current = self.read_cpu(stage, CpuInterfaceRegister::Sre)?;
        self.write_cpu(
            stage,
            CpuInterfaceRegister::Sre,
            current | ICC_SRE_EL1_SRE | ICC_SRE_EL1_DFB | ICC_SRE_EL1_DIB,
        )?;
        self.backend.synchronize();
        let observed = self.read_cpu(stage, CpuInterfaceRegister::Sre)?;
        if observed & ICC_SRE_EL1_SRE == 0 {
            return Err(GicV3PreparationError::SystemRegisterInterfaceUnavailable { observed });
        }
        Ok(())
    }

    fn configure_distributor(&mut self) -> Result<(), GicV3PreparationError<B::Error>> {
        let stage = GicV3PreparationStage::ConfigureDistributor;
        let current = self.read_distributor(stage, GICD_CTLR)?;
        let affinity_routing = current & GICD_CTLR_ARE_NS;
        self.write_distributor(stage, GICD_CTLR, affinity_routing)?;
        self.backend.synchronize();
        self.poll_distributor_clear(stage, GICD_CTLR_RWP)?;
        if affinity_routing == 0 {
            self.write_distributor(stage, GICD_CTLR, GICD_CTLR_ARE_NS)?;
            self.backend.synchronize();
            self.poll_distributor_clear(stage, GICD_CTLR_RWP)?;
        }
        let observed = self.read_distributor(stage, GICD_CTLR)?;
        if observed & GICD_CTLR_ARE_NS == 0 {
            return Err(GicV3PreparationError::AffinityRoutingUnavailable { observed });
        }
        debug_assert_eq!(observed & (GICD_CTLR_ENABLE_G1 | GICD_CTLR_ENABLE_G1A), 0);
        Ok(())
    }

    fn wake_redistributor(&mut self) -> Result<(), GicV3PreparationError<B::Error>> {
        let stage = GicV3PreparationStage::WakeRedistributor;
        let waker = self.read_redistributor(stage, GICR_WAKER)?;
        self.write_redistributor(stage, GICR_WAKER, waker & !GICR_WAKER_PROCESSOR_SLEEP)?;
        self.backend.synchronize();
        self.poll_redistributor_clear(stage, GICR_WAKER, GICR_WAKER_CHILDREN_ASLEEP)
    }

    fn configure_private_interrupts(&mut self) -> Result<(), GicV3PreparationError<B::Error>> {
        let stage = GicV3PreparationStage::ConfigurePrivateInterrupts;
        self.write_redistributor(stage, GICR_ICENABLER0, PRIVATE_INTERRUPT_MASK)?;
        self.backend.synchronize();
        self.poll_redistributor_clear(stage, GICR_CTLR, GICR_CTLR_RWP)?;
        self.write_redistributor(stage, GICR_ICPENDR0, PRIVATE_INTERRUPT_MASK)?;
        self.write_redistributor(stage, GICR_ICACTIVER0, PRIVATE_INTERRUPT_MASK)?;
        self.write_redistributor(stage, GICR_IGROUPR0, PRIVATE_INTERRUPT_MASK)?;
        self.write_redistributor(stage, GICR_IGRPMODR0, 0)?;

        let timer = self.configuration.timer;
        let ppi_index = u32::from(timer.intid() - PPI_INTID_MIN);
        let shift = ppi_index * 2;
        let mut configuration = self.read_redistributor(stage, GICR_ICFGR1)?;
        configuration &= !(0b11 << shift);
        if timer.trigger() == InterruptTrigger::EdgeRising {
            configuration |= 0b10 << shift;
        }
        self.write_redistributor(stage, GICR_ICFGR1, configuration)?;

        let priority_offset = GICR_IPRIORITYR + usize::from(timer.intid() / 4) * 4;
        let priority_shift = u32::from(timer.intid() % 4) * 8;
        let mut priorities = self.read_redistributor(stage, priority_offset)?;
        priorities &= !(0xff << priority_shift);
        priorities |= u32::from(self.configuration.priority) << priority_shift;
        self.write_redistributor(stage, priority_offset, priorities)?;

        self.write_redistributor(stage, GICR_ISENABLER0, 1_u32 << timer.intid())?;
        self.backend.synchronize();
        self.poll_redistributor_clear(stage, GICR_CTLR, GICR_CTLR_RWP)
    }

    fn configure_cpu_interface(&mut self) -> Result<(), GicV3PreparationError<B::Error>> {
        let stage = GicV3PreparationStage::ConfigureCpuInterface;
        let control = self.read_cpu(stage, CpuInterfaceRegister::Control)?;
        self.write_cpu(
            stage,
            CpuInterfaceRegister::Control,
            control & !ICC_CTLR_EL1_EOI_MODE,
        )?;
        self.write_cpu(stage, CpuInterfaceRegister::BinaryPoint1, 0)?;
        self.write_cpu(
            stage,
            CpuInterfaceRegister::PriorityMask,
            PRIORITY_MASK_ALLOW_ALL,
        )?;
        self.write_cpu(stage, CpuInterfaceRegister::GroupEnable1, 0)?;
        self.backend.synchronize();
        Ok(())
    }

    fn recover(&mut self) -> Option<GicV3PreparationError<B::Error>> {
        let stage = GicV3PreparationStage::Recovery;
        let mut first = None;
        if let Err(error) = self
            .backend
            .write_cpu_interface(CpuInterfaceRegister::GroupEnable1, 0)
        {
            first = Some(GicV3PreparationError::Access { stage, error });
        }
        if let Err(error) = self
            .backend
            .write_redistributor(GICR_ICENABLER0, PRIVATE_INTERRUPT_MASK)
            && first.is_none()
        {
            first = Some(GicV3PreparationError::Access { stage, error });
        }
        match self.backend.read_distributor(GICD_CTLR) {
            Ok(control) => {
                if let Err(error) = self
                    .backend
                    .write_distributor(GICD_CTLR, control & GICD_CTLR_ARE_NS)
                    && first.is_none()
                {
                    first = Some(GicV3PreparationError::Access { stage, error });
                }
            }
            Err(error) if first.is_none() => {
                first = Some(GicV3PreparationError::Access { stage, error });
            }
            Err(_) => {}
        }
        self.backend.synchronize();
        first
    }

    fn poll_distributor_clear(
        &mut self,
        stage: GicV3PreparationStage,
        mask: u32,
    ) -> Result<(), GicV3PreparationError<B::Error>> {
        for _ in 0..self.configuration.poll_budget.get() {
            if self.read_distributor(stage, GICD_CTLR)? & mask == 0 {
                return Ok(());
            }
        }
        Err(GicV3PreparationError::Timeout { stage })
    }

    fn poll_redistributor_clear(
        &mut self,
        stage: GicV3PreparationStage,
        offset: usize,
        mask: u32,
    ) -> Result<(), GicV3PreparationError<B::Error>> {
        for _ in 0..self.configuration.poll_budget.get() {
            if self.read_redistributor(stage, offset)? & mask == 0 {
                return Ok(());
            }
        }
        Err(GicV3PreparationError::Timeout { stage })
    }

    fn read_distributor(
        &mut self,
        stage: GicV3PreparationStage,
        offset: usize,
    ) -> Result<u32, GicV3PreparationError<B::Error>> {
        self.backend
            .read_distributor(offset)
            .map_err(|error| GicV3PreparationError::Access { stage, error })
    }

    fn write_distributor(
        &mut self,
        stage: GicV3PreparationStage,
        offset: usize,
        value: u32,
    ) -> Result<(), GicV3PreparationError<B::Error>> {
        self.backend
            .write_distributor(offset, value)
            .map_err(|error| GicV3PreparationError::Access { stage, error })
    }

    fn read_redistributor(
        &mut self,
        stage: GicV3PreparationStage,
        offset: usize,
    ) -> Result<u32, GicV3PreparationError<B::Error>> {
        self.backend
            .read_redistributor(offset)
            .map_err(|error| GicV3PreparationError::Access { stage, error })
    }

    fn write_redistributor(
        &mut self,
        stage: GicV3PreparationStage,
        offset: usize,
        value: u32,
    ) -> Result<(), GicV3PreparationError<B::Error>> {
        self.backend
            .write_redistributor(offset, value)
            .map_err(|error| GicV3PreparationError::Access { stage, error })
    }

    fn read_cpu(
        &mut self,
        stage: GicV3PreparationStage,
        register: CpuInterfaceRegister,
    ) -> Result<u64, GicV3PreparationError<B::Error>> {
        self.backend
            .read_cpu_interface(register)
            .map_err(|error| GicV3PreparationError::Access { stage, error })
    }

    fn write_cpu(
        &mut self,
        stage: GicV3PreparationStage,
        register: CpuInterfaceRegister,
        value: u64,
    ) -> Result<(), GicV3PreparationError<B::Error>> {
        self.backend
            .write_cpu_interface(register, value)
            .map_err(|error| GicV3PreparationError::Access { stage, error })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::num::NonZeroU32;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    use super::{
        CpuInterfaceRegister, GICD_CTLR, GICD_CTLR_ARE_NS, GICD_CTLR_ENABLE_G1,
        GICD_CTLR_ENABLE_G1A, GICD_PIDR2, GICR_ICACTIVER0, GICR_ICENABLER0, GICR_ICFGR1,
        GICR_ICPENDR0, GICR_IGROUPR0, GICR_IGRPMODR0, GICR_IPRIORITYR, GICR_ISENABLER0, GICR_PIDR2,
        GICR_WAKER, GICR_WAKER_CHILDREN_ASLEEP, GICR_WAKER_PROCESSOR_SLEEP, GicV3Configuration,
        GicV3ConfigurationError, GicV3PreparationError, GicV3PreparationStage, GicV3RegisterAccess,
        ICC_SRE_EL1_SRE, InterruptTrigger, PrivateInterruptError, PrivatePeripheralInterrupt,
        prepare,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum MockError {
        Injected,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ReadDistributor(usize),
        WriteDistributor(usize, u32),
        ReadRedistributor(usize),
        WriteRedistributor(usize, u32),
        ReadCpu(CpuInterfaceRegister),
        WriteCpu(CpuInterfaceRegister, u64),
        Synchronize,
    }

    #[derive(Debug)]
    struct MockRegisters {
        distributor: BTreeMap<usize, u32>,
        redistributor: BTreeMap<usize, u32>,
        cpu: BTreeMap<CpuInterfaceRegister, u64>,
        operations: Vec<Operation>,
        operation_count: usize,
        fail_at: Option<usize>,
        sre_sticks: bool,
        children_asleep_reads: usize,
        children_asleep_forever: bool,
    }

    impl MockRegisters {
        fn valid() -> Self {
            let mut distributor = BTreeMap::new();
            distributor.insert(GICD_PIDR2, 3 << 4);
            distributor.insert(GICD_CTLR, 0);
            let mut redistributor = BTreeMap::new();
            redistributor.insert(GICR_PIDR2, 3 << 4);
            redistributor.insert(
                GICR_WAKER,
                GICR_WAKER_PROCESSOR_SLEEP | GICR_WAKER_CHILDREN_ASLEEP,
            );
            redistributor.insert(GICR_ICFGR1, u32::MAX);
            redistributor.insert(GICR_IPRIORITYR + 7 * 4, 0x1122_3344);
            let mut cpu = BTreeMap::new();
            cpu.insert(CpuInterfaceRegister::Sre, 0);
            cpu.insert(CpuInterfaceRegister::Control, u64::MAX);
            Self {
                distributor,
                redistributor,
                cpu,
                operations: Vec::new(),
                operation_count: 0,
                fail_at: None,
                sre_sticks: true,
                children_asleep_reads: 1,
                children_asleep_forever: false,
            }
        }

        fn before_operation(&mut self) -> Result<(), MockError> {
            self.operation_count += 1;
            if self.fail_at == Some(self.operation_count) {
                Err(MockError::Injected)
            } else {
                Ok(())
            }
        }

        fn distributor(&self, offset: usize) -> u32 {
            self.distributor.get(&offset).copied().unwrap_or(0)
        }

        fn redistributor(&self, offset: usize) -> u32 {
            self.redistributor.get(&offset).copied().unwrap_or(0)
        }

        fn cpu(&self, register: CpuInterfaceRegister) -> u64 {
            self.cpu.get(&register).copied().unwrap_or(0)
        }
    }

    // SAFETY: this deterministic mock owns all modeled register state, uses no
    // raw memory, and records every ordered access for the test's lifetime.
    unsafe impl GicV3RegisterAccess for MockRegisters {
        type Error = MockError;

        fn read_distributor(&mut self, offset: usize) -> Result<u32, Self::Error> {
            self.before_operation()?;
            self.operations.push(Operation::ReadDistributor(offset));
            Ok(self.distributor(offset))
        }

        fn write_distributor(&mut self, offset: usize, value: u32) -> Result<(), Self::Error> {
            self.before_operation()?;
            self.operations
                .push(Operation::WriteDistributor(offset, value));
            self.distributor.insert(offset, value);
            Ok(())
        }

        fn read_redistributor(&mut self, offset: usize) -> Result<u32, Self::Error> {
            self.before_operation()?;
            self.operations.push(Operation::ReadRedistributor(offset));
            if offset == GICR_WAKER {
                let mut value = self.redistributor(offset);
                if value & GICR_WAKER_PROCESSOR_SLEEP == 0 && !self.children_asleep_forever {
                    if self.children_asleep_reads == 0 {
                        value &= !GICR_WAKER_CHILDREN_ASLEEP;
                        self.redistributor.insert(offset, value);
                    } else {
                        self.children_asleep_reads -= 1;
                    }
                }
                return Ok(value);
            }
            Ok(self.redistributor(offset))
        }

        fn write_redistributor(&mut self, offset: usize, value: u32) -> Result<(), Self::Error> {
            self.before_operation()?;
            self.operations
                .push(Operation::WriteRedistributor(offset, value));
            self.redistributor.insert(offset, value);
            Ok(())
        }

        fn read_cpu_interface(
            &mut self,
            register: CpuInterfaceRegister,
        ) -> Result<u64, Self::Error> {
            self.before_operation()?;
            self.operations.push(Operation::ReadCpu(register));
            Ok(self.cpu(register))
        }

        fn write_cpu_interface(
            &mut self,
            register: CpuInterfaceRegister,
            value: u64,
        ) -> Result<(), Self::Error> {
            self.before_operation()?;
            self.operations.push(Operation::WriteCpu(register, value));
            if register != CpuInterfaceRegister::Sre || self.sre_sticks {
                self.cpu.insert(register, value);
            }
            Ok(())
        }

        fn synchronize(&mut self) {
            self.operations.push(Operation::Synchronize);
        }
    }

    fn timer(trigger: InterruptTrigger) -> PrivatePeripheralInterrupt {
        PrivatePeripheralInterrupt::new(30, trigger).unwrap()
    }

    fn configuration(trigger: InterruptTrigger) -> GicV3Configuration {
        GicV3Configuration::new(timer(trigger), NonZeroU32::new(3).unwrap())
    }

    #[test]
    fn ppi_identity_is_checked_at_the_architecture_boundary() {
        assert_eq!(
            PrivatePeripheralInterrupt::new(15, InterruptTrigger::LevelHigh),
            Err(PrivateInterruptError::OutOfRange { intid: 15 })
        );
        assert_eq!(
            PrivatePeripheralInterrupt::new(32, InterruptTrigger::EdgeRising),
            Err(PrivateInterruptError::OutOfRange { intid: 32 })
        );
        assert_eq!(timer(InterruptTrigger::LevelHigh).intid(), 30);
        assert_eq!(
            configuration(InterruptTrigger::LevelHigh).with_priority(u8::MAX),
            Err(GicV3ConfigurationError::PermanentlyMaskedPriority)
        );
    }

    #[test]
    fn preparation_configures_one_timer_ppi_but_keeps_delivery_disabled() {
        let registers = MockRegisters::valid();
        // SAFETY: the mock is uniquely owned and models externally masked IRQs.
        let prepared =
            unsafe { prepare(registers, configuration(InterruptTrigger::LevelHigh)) }.unwrap();
        let registers = prepared.backend();

        assert_eq!(registers.distributor(GICD_CTLR), GICD_CTLR_ARE_NS);
        assert_eq!(
            registers.cpu(CpuInterfaceRegister::GroupEnable1),
            0,
            "CPU-interface delivery must remain disabled"
        );
        assert_eq!(registers.cpu(CpuInterfaceRegister::PriorityMask), 0xff);
        assert_ne!(
            registers.cpu(CpuInterfaceRegister::Sre) & ICC_SRE_EL1_SRE,
            0
        );
        assert_eq!(registers.redistributor(GICR_ICENABLER0), u32::MAX);
        assert_eq!(registers.redistributor(GICR_ICPENDR0), u32::MAX);
        assert_eq!(registers.redistributor(GICR_ICACTIVER0), u32::MAX);
        assert_eq!(registers.redistributor(GICR_IGROUPR0), u32::MAX);
        assert_eq!(registers.redistributor(GICR_IGRPMODR0), 0);
        assert_eq!(registers.redistributor(GICR_ICFGR1) & (0b11 << 28), 0);
        assert_eq!(
            registers.redistributor(GICR_IPRIORITYR + 7 * 4),
            0x11a0_3344
        );
        assert_eq!(registers.redistributor(GICR_ISENABLER0), 1 << 30);
        assert_eq!(
            registers.redistributor(GICR_WAKER) & GICR_WAKER_PROCESSOR_SLEEP,
            0
        );
    }

    #[test]
    fn edge_configuration_changes_only_the_selected_ppi_field_and_priority_byte() {
        let registers = MockRegisters::valid();
        let config = configuration(InterruptTrigger::EdgeRising)
            .with_priority(0x70)
            .unwrap();
        // SAFETY: the mock is uniquely owned and models externally masked IRQs.
        let prepared = unsafe { prepare(registers, config) }.unwrap();
        let registers = prepared.backend();
        assert_eq!(
            registers.redistributor(GICR_ICFGR1),
            !(0b11 << 28) | (0b10 << 28)
        );
        assert_eq!(
            registers.redistributor(GICR_IPRIORITYR + 7 * 4),
            0x1170_3344
        );
    }

    #[test]
    fn repeated_preparation_preserves_affinity_routing_while_closing_group_gates() {
        let mut registers = MockRegisters::valid();
        registers.distributor.insert(
            GICD_CTLR,
            GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_G1 | GICD_CTLR_ENABLE_G1A,
        );
        // SAFETY: the mock is uniquely owned and models externally masked IRQs.
        let prepared =
            unsafe { prepare(registers, configuration(InterruptTrigger::LevelHigh)) }.unwrap();
        let registers = prepared.backend();
        assert_eq!(registers.distributor(GICD_CTLR), GICD_CTLR_ARE_NS);
        assert!(
            !registers
                .operations
                .contains(&Operation::WriteDistributor(GICD_CTLR, 0))
        );
    }

    #[test]
    fn unsupported_interfaces_and_bounded_wake_timeout_recover_delivery_gates() {
        let mut unsupported = MockRegisters::valid();
        unsupported.distributor.insert(GICD_PIDR2, 2 << 4);
        // SAFETY: the mock is uniquely owned and models externally masked IRQs.
        let failure = unsafe { prepare(unsupported, configuration(InterruptTrigger::LevelHigh)) }
            .unwrap_err();
        assert_eq!(
            failure.error(),
            &GicV3PreparationError::UnsupportedArchitecture {
                component: "GICD",
                revision: 2,
            }
        );
        assert_eq!(failure.recovery_error(), None);
        let (registers, _, _) = failure.into_parts();
        assert_eq!(registers.distributor(GICD_CTLR), 0);
        assert_eq!(registers.cpu(CpuInterfaceRegister::GroupEnable1), 0);

        let mut no_sre = MockRegisters::valid();
        no_sre.sre_sticks = false;
        // SAFETY: the mock is uniquely owned and models externally masked IRQs.
        let failure =
            unsafe { prepare(no_sre, configuration(InterruptTrigger::LevelHigh)) }.unwrap_err();
        assert_eq!(
            failure.error(),
            &GicV3PreparationError::SystemRegisterInterfaceUnavailable { observed: 0 }
        );

        let mut sleeping = MockRegisters::valid();
        sleeping.children_asleep_forever = true;
        // SAFETY: the mock is uniquely owned and models externally masked IRQs.
        let failure =
            unsafe { prepare(sleeping, configuration(InterruptTrigger::LevelHigh)) }.unwrap_err();
        assert_eq!(
            failure.error(),
            &GicV3PreparationError::Timeout {
                stage: GicV3PreparationStage::WakeRedistributor,
            }
        );
        let (registers, _, _) = failure.into_parts();
        assert_eq!(registers.distributor(GICD_CTLR), GICD_CTLR_ARE_NS);
        assert_eq!(registers.redistributor(GICR_ICENABLER0), u32::MAX);
    }

    #[test]
    fn injected_access_failure_is_attributed_and_recovery_is_attempted() {
        let mut registers = MockRegisters::valid();
        registers.fail_at = Some(6);
        // SAFETY: the mock is uniquely owned and models externally masked IRQs.
        let failure =
            unsafe { prepare(registers, configuration(InterruptTrigger::LevelHigh)) }.unwrap_err();
        assert_eq!(
            failure.error(),
            &GicV3PreparationError::Access {
                stage: GicV3PreparationStage::QuiesceCpuInterface,
                error: MockError::Injected,
            }
        );
        assert_eq!(failure.recovery_error(), None);
        let (registers, _, _) = failure.into_parts();
        assert_eq!(registers.distributor(GICD_CTLR), 0);
        assert_eq!(registers.cpu(CpuInterfaceRegister::GroupEnable1), 0);
        assert!(
            registers
                .operations
                .contains(&Operation::WriteRedistributor(GICR_ICENABLER0, u32::MAX))
        );
    }
}
