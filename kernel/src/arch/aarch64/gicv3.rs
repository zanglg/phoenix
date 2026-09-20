//! Host-testable GICv3 discovery, preparation, and delivery type state.

use core::marker::PhantomData;
use core::num::NonZeroU32;
#[cfg(target_arch = "aarch64")]
use core::{arch::asm, ptr};

#[cfg(target_arch = "aarch64")]
use crate::arch::aarch64::kernel_page_table::MaterializedKernelPageTables;
use crate::memory::{AddressRange, PAGE_SIZE, PhysAddr, VirtAddr};

const GICD_CTLR: usize = 0x0000;
const GICD_PIDR2: usize = 0xffe8;
const GICD_CTLR_ENABLE_G1: u32 = 1 << 0;
const GICD_CTLR_ENABLE_G1A: u32 = 1 << 1;
// Bit 1 is the Non-secure Group-1 enable in the Non-secure register view and
// the Group-1 enable when the GIC implements a single Security state.
const GICD_CTLR_ENABLE_NONSECURE_GROUP1: u32 = GICD_CTLR_ENABLE_G1A;
const GICD_CTLR_ARE_NS: u32 = 1 << 4;
const GICD_CTLR_RWP: u32 = 1 << 31;

const GICR_CTLR: usize = 0x0000;
const GICR_TYPER: usize = 0x0008;
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
const GICR_TYPER_VLPIS: u64 = 1 << 1;
const GICR_TYPER_LAST: u64 = 1 << 4;
const GICR_TYPER_AFFINITY_SHIFT: u32 = 32;

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
const ICC_IAR1_INTID_MASK: u32 = 0x00ff_ffff;
const SPECIAL_INTID_MIN: u32 = 1020;
const SPECIAL_INTID_MAX: u32 = 1023;

/// Lowest architectural private peripheral interrupt ID.
pub const PPI_INTID_MIN: u16 = 16;
/// Highest architectural private peripheral interrupt ID.
pub const PPI_INTID_MAX: u16 = 31;
/// Size of one GICv3 redistributor RD plus SGI frame pair.
pub const GIC_REDISTRIBUTOR_FRAME_SIZE: usize = 128 * 1024;
/// Minimum distributor range needed by the GICv3 programming model.
pub const GIC_DISTRIBUTOR_MIN_SIZE: usize = 64 * 1024;

/// Packed `Aff3:Aff2:Aff1:Aff0` identity used by `GICR_TYPER`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CpuAffinity(u32);

impl CpuAffinity {
    /// Extract the four affinity levels from an `MPIDR_EL1` value.
    pub const fn from_mpidr(mpidr: u64) -> Self {
        let aff0 = mpidr & 0xff;
        let aff1 = (mpidr >> 8) & 0xff;
        let aff2 = (mpidr >> 16) & 0xff;
        let aff3 = (mpidr >> 32) & 0xff;
        Self(((aff3 << 24) | (aff2 << 16) | (aff1 << 8) | aff0) as u32)
    }

    /// Return the packed affinity value compared with `GICR_TYPER[63:32]`.
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Read the affinity of the current AArch64 processing element.
    #[cfg(target_arch = "aarch64")]
    pub fn current() -> Self {
        let mpidr: u64;
        // SAFETY: reading `MPIDR_EL1` has no memory side effects and is
        // available at EL1 in the AArch64 execution state used by Phoenix.
        unsafe {
            asm!(
                "mrs {mpidr}, MPIDR_EL1",
                mpidr = out(reg) mpidr,
                options(nomem, nostack, preserves_flags)
            );
        }
        Self::from_mpidr(mpidr)
    }
}

/// Fallible read access used while selecting the current CPU redistributor.
pub trait GicRedistributorDiscoveryAccess {
    /// Backend-specific read failure.
    type Error;

    /// Read one aligned 32-bit physical MMIO register.
    fn read_u32(&mut self, address: PhysAddr) -> Result<u32, Self::Error>;

    /// Read one aligned 64-bit physical MMIO register.
    fn read_u64(&mut self, address: PhysAddr) -> Result<u64, Self::Error>;
}

/// Exact 128 KiB redistributor RD/SGI pair selected for one CPU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedRedistributor {
    range: AddressRange<PhysAddr>,
    typer: u64,
}

impl SelectedRedistributor {
    /// Return the exact RD/SGI frame pair; optional VLPI pages are excluded.
    pub const fn range(self) -> AddressRange<PhysAddr> {
        self.range
    }

    /// Return the observed `GICR_TYPER` value.
    pub const fn typer(self) -> u64 {
        self.typer
    }
}

/// Failure while selecting one CPU's redistributor from a discovered region.
#[derive(Debug, Eq, PartialEq)]
pub enum RedistributorSelectionError<E> {
    /// A physical register address overflowed.
    AddressOverflow,
    /// A candidate does not contain a complete RD/SGI frame pair.
    TruncatedFrame {
        /// Candidate physical base.
        base: usize,
    },
    /// A candidate PIDR2 does not report GICv3 or GICv4.
    UnsupportedArchitecture {
        /// Candidate physical base.
        base: usize,
        /// Observed four-bit architecture revision.
        revision: u8,
    },
    /// A physical MMIO read failed.
    Access {
        /// Register address that could not be read.
        address: usize,
        /// Backend-specific failure.
        error: E,
    },
    /// More than one redistributor advertises the requested affinity.
    DuplicateAffinity {
        /// Duplicated packed affinity.
        affinity: u32,
    },
    /// No redistributor advertises the requested affinity before `Last`.
    AffinityNotFound {
        /// Missing packed affinity.
        affinity: u32,
    },
    /// The discovered range ended without a `GICR_TYPER.Last` frame.
    MissingLast,
}

/// Select one CPU's exact redistributor RD/SGI pair without retaining MMIO.
pub fn select_redistributor<A: GicRedistributorDiscoveryAccess>(
    region: AddressRange<PhysAddr>,
    affinity: CpuAffinity,
    access: &mut A,
) -> Result<SelectedRedistributor, RedistributorSelectionError<A::Error>> {
    let end = region.end().as_usize();
    let mut base = region.start().as_usize();
    let mut selected = None;
    while base < end {
        let frame_end = base
            .checked_add(GIC_REDISTRIBUTOR_FRAME_SIZE)
            .ok_or(RedistributorSelectionError::AddressOverflow)?;
        if frame_end > end {
            return Err(RedistributorSelectionError::TruncatedFrame { base });
        }

        let pidr2_address = base
            .checked_add(GICR_PIDR2)
            .ok_or(RedistributorSelectionError::AddressOverflow)?;
        let pidr2 = access
            .read_u32(PhysAddr::new(pidr2_address))
            .map_err(|error| RedistributorSelectionError::Access {
                address: pidr2_address,
                error,
            })?;
        let revision =
            ((pidr2 >> GIC_PIDR2_ARCHITECTURE_SHIFT) & GIC_PIDR2_ARCHITECTURE_MASK) as u8;
        if !matches!(revision, GIC_ARCHITECTURE_V3 | GIC_ARCHITECTURE_V4) {
            return Err(RedistributorSelectionError::UnsupportedArchitecture { base, revision });
        }

        let typer_address = base
            .checked_add(GICR_TYPER)
            .ok_or(RedistributorSelectionError::AddressOverflow)?;
        let typer = access
            .read_u64(PhysAddr::new(typer_address))
            .map_err(|error| RedistributorSelectionError::Access {
                address: typer_address,
                error,
            })?;
        let candidate_affinity = (typer >> GICR_TYPER_AFFINITY_SHIFT) as u32;
        if candidate_affinity == affinity.raw() {
            if selected.is_some() {
                return Err(RedistributorSelectionError::DuplicateAffinity {
                    affinity: affinity.raw(),
                });
            }
            let range = AddressRange::new(PhysAddr::new(base), PhysAddr::new(frame_end))
                .map_err(|_| RedistributorSelectionError::AddressOverflow)?;
            selected = Some(SelectedRedistributor { range, typer });
        }
        if typer & GICR_TYPER_LAST != 0 {
            return selected.ok_or(RedistributorSelectionError::AffinityNotFound {
                affinity: affinity.raw(),
            });
        }

        let stride = if typer & GICR_TYPER_VLPIS != 0 {
            GIC_REDISTRIBUTOR_FRAME_SIZE
                .checked_mul(2)
                .ok_or(RedistributorSelectionError::AddressOverflow)?
        } else {
            GIC_REDISTRIBUTOR_FRAME_SIZE
        };
        base = base
            .checked_add(stride)
            .ok_or(RedistributorSelectionError::AddressOverflow)?;
    }
    Err(RedistributorSelectionError::MissingLast)
}

/// Checked virtual ranges used by the concrete GICv3 register backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicV3MmioLayout {
    distributor: AddressRange<VirtAddr>,
    redistributor: AddressRange<VirtAddr>,
}

/// Invalid mapped GIC register ownership or access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GicV3MmioError {
    /// One range is misaligned or too short for required registers.
    InvalidRange {
        /// `GICD` or `GICR`.
        component: &'static str,
    },
    /// Distributor and redistributor virtual ranges overlap.
    OverlappingRanges,
    /// A register offset is not aligned for a 32-bit access.
    MisalignedOffset {
        /// `GICD` or `GICR`.
        component: &'static str,
        /// Rejected byte offset.
        offset: usize,
    },
    /// A register lies outside its exact mapped range.
    RegisterOutOfRange {
        /// `GICD` or `GICR`.
        component: &'static str,
        /// Rejected byte offset.
        offset: usize,
    },
    /// Virtual address arithmetic overflowed.
    AddressOverflow,
}

impl GicV3MmioLayout {
    /// Validate exact mapped distributor and selected-redistributor ranges.
    pub fn new(
        distributor: AddressRange<VirtAddr>,
        redistributor: AddressRange<VirtAddr>,
    ) -> Result<Self, GicV3MmioError> {
        if !valid_mmio_range(distributor, GIC_DISTRIBUTOR_MIN_SIZE, false) {
            return Err(GicV3MmioError::InvalidRange { component: "GICD" });
        }
        if !valid_mmio_range(redistributor, GIC_REDISTRIBUTOR_FRAME_SIZE, true) {
            return Err(GicV3MmioError::InvalidRange { component: "GICR" });
        }
        if distributor.overlaps(redistributor) {
            return Err(GicV3MmioError::OverlappingRanges);
        }
        Ok(Self {
            distributor,
            redistributor,
        })
    }

    /// Return the complete mapped distributor range.
    pub const fn distributor(self) -> AddressRange<VirtAddr> {
        self.distributor
    }

    /// Return the selected mapped redistributor RD/SGI pair.
    pub const fn redistributor(self) -> AddressRange<VirtAddr> {
        self.redistributor
    }

    #[cfg(any(target_arch = "aarch64", test))]
    fn distributor_register(self, offset: usize) -> Result<usize, GicV3MmioError> {
        register_address("GICD", self.distributor, offset)
    }

    #[cfg(any(target_arch = "aarch64", test))]
    fn redistributor_register(self, offset: usize) -> Result<usize, GicV3MmioError> {
        register_address("GICR", self.redistributor, offset)
    }
}

fn valid_mmio_range(range: AddressRange<VirtAddr>, minimum: usize, exact: bool) -> bool {
    range.start().as_usize().is_multiple_of(PAGE_SIZE)
        && range.end().as_usize().is_multiple_of(PAGE_SIZE)
        && if exact {
            range.len() == minimum
        } else {
            range.len() >= minimum
        }
}

#[cfg(any(target_arch = "aarch64", test))]
fn register_address(
    component: &'static str,
    range: AddressRange<VirtAddr>,
    offset: usize,
) -> Result<usize, GicV3MmioError> {
    if !offset.is_multiple_of(size_of::<u32>()) {
        return Err(GicV3MmioError::MisalignedOffset { component, offset });
    }
    let end = offset
        .checked_add(size_of::<u32>())
        .ok_or(GicV3MmioError::AddressOverflow)?;
    if end > range.len() {
        return Err(GicV3MmioError::RegisterOutOfRange { component, offset });
    }
    range
        .start()
        .as_usize()
        .checked_add(offset)
        .ok_or(GicV3MmioError::AddressOverflow)
}

/// Concrete volatile GICv3 backend tied to an active final TTBR1 owner.
#[cfg(target_arch = "aarch64")]
pub struct MappedGicV3Registers<'a, const TABLES: usize> {
    _tables: &'a MaterializedKernelPageTables<TABLES>,
    layout: GicV3MmioLayout,
    _not_send_or_sync: PhantomData<*mut ()>,
}

#[cfg(target_arch = "aarch64")]
impl<'a, const TABLES: usize> MappedGicV3Registers<'a, TABLES> {
    /// Bind checked GIC device ranges to their permanently retained hierarchy.
    ///
    /// # Safety
    ///
    /// `tables` must be the active TTBR1 hierarchy that maps every byte in
    /// `layout` as EL1-only Device-nGnRnE RW/NX memory. The caller must grant
    /// this value unique ownership of those GICD/GICR ranges and current-CPU
    /// `ICC_*_EL1` state, keep IRQs masked until an `ActiveGicV3` is created
    /// behind a published dispatcher, and prevent concurrent CPU access. The
    /// resulting backend is deliberately neither `Send` nor `Sync`.
    pub const unsafe fn assume_active(
        tables: &'a MaterializedKernelPageTables<TABLES>,
        layout: GicV3MmioLayout,
    ) -> Self {
        Self {
            _tables: tables,
            layout,
            _not_send_or_sync: PhantomData,
        }
    }
}

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
/// before later polling or state publication. `acknowledge_group1` must return
/// the raw `ICC_IAR1_EL1` value, and `end_group1` must write the supplied INTID
/// to `ICC_EOIR1_EL1` in the prepared combined-deactivation mode. No interrupt
/// handler or other CPU may access the same mutable controller state
/// concurrently.
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

    /// Acknowledge the highest-priority pending Group-1 interrupt.
    fn acknowledge_group1(&mut self) -> Result<u32, Self::Error>;

    /// Perform combined priority drop and deactivation for one acknowledged
    /// Group-1 interrupt while `ICC_CTLR_EL1.EOImode == 0`.
    fn end_group1(&mut self, intid: u32) -> Result<(), Self::Error>;

    /// Complete earlier register writes before subsequent observations.
    fn synchronize(&mut self);
}

#[cfg(target_arch = "aarch64")]
// SAFETY: construction is unsafe and binds this backend to exact Device-nGnRnE
// ranges, one active table owner, and unique current-CPU interface ownership.
// Every implementation method uses volatile MMIO or one named system register;
// `synchronize` supplies the required device completion and context barrier.
unsafe impl<const TABLES: usize> GicV3RegisterAccess for MappedGicV3Registers<'_, TABLES> {
    type Error = GicV3MmioError;

    fn read_distributor(&mut self, offset: usize) -> Result<u32, Self::Error> {
        let address = self.layout.distributor_register(offset)? as *const u32;
        // SAFETY: the unsafe constructor proves this exact checked address is
        // an owned, active GICD Device mapping. Volatile preserves MMIO access.
        Ok(unsafe { ptr::read_volatile(address) })
    }

    fn write_distributor(&mut self, offset: usize, value: u32) -> Result<(), Self::Error> {
        let address = self.layout.distributor_register(offset)? as *mut u32;
        // SAFETY: the unsafe constructor proves unique access to this checked
        // active GICD Device mapping. Volatile preserves the register write.
        unsafe {
            ptr::write_volatile(address, value);
        }
        Ok(())
    }

    fn read_redistributor(&mut self, offset: usize) -> Result<u32, Self::Error> {
        let address = self.layout.redistributor_register(offset)? as *const u32;
        // SAFETY: the unsafe constructor proves this exact checked address is
        // in the selected current-CPU GICR mapping and is uniquely owned.
        Ok(unsafe { ptr::read_volatile(address) })
    }

    fn write_redistributor(&mut self, offset: usize, value: u32) -> Result<(), Self::Error> {
        let address = self.layout.redistributor_register(offset)? as *mut u32;
        // SAFETY: the unsafe constructor proves unique access to this checked
        // selected-GICR Device mapping. Volatile preserves the register write.
        unsafe {
            ptr::write_volatile(address, value);
        }
        Ok(())
    }

    fn read_cpu_interface(&mut self, register: CpuInterfaceRegister) -> Result<u64, Self::Error> {
        let value: u64;
        // SAFETY: backend construction grants unique access to the current
        // CPU interface, IRQs remain masked, and each match names the exact
        // architected EL1 system register represented by `register`.
        unsafe {
            match register {
                CpuInterfaceRegister::Sre => asm!(
                    "mrs {value}, ICC_SRE_EL1",
                    value = out(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::PriorityMask => asm!(
                    "mrs {value}, ICC_PMR_EL1",
                    value = out(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::BinaryPoint1 => asm!(
                    "mrs {value}, ICC_BPR1_EL1",
                    value = out(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::GroupEnable1 => asm!(
                    "mrs {value}, ICC_IGRPEN1_EL1",
                    value = out(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::Control => asm!(
                    "mrs {value}, ICC_CTLR_EL1",
                    value = out(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
            }
        }
        Ok(value)
    }

    fn write_cpu_interface(
        &mut self,
        register: CpuInterfaceRegister,
        value: u64,
    ) -> Result<(), Self::Error> {
        // SAFETY: backend construction grants unique access to the current
        // CPU interface with IRQs masked; the match writes only the selected
        // architected EL1 system register and does not access memory.
        unsafe {
            match register {
                CpuInterfaceRegister::Sre => asm!(
                    "msr ICC_SRE_EL1, {value}",
                    value = in(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::PriorityMask => asm!(
                    "msr ICC_PMR_EL1, {value}",
                    value = in(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::BinaryPoint1 => asm!(
                    "msr ICC_BPR1_EL1, {value}",
                    value = in(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::GroupEnable1 => asm!(
                    "msr ICC_IGRPEN1_EL1, {value}",
                    value = in(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
                CpuInterfaceRegister::Control => asm!(
                    "msr ICC_CTLR_EL1, {value}",
                    value = in(reg) value,
                    options(nomem, nostack, preserves_flags)
                ),
            }
        }
        Ok(())
    }

    fn acknowledge_group1(&mut self) -> Result<u32, Self::Error> {
        let value: u64;
        // SAFETY: backend construction grants unique access to the current
        // CPU interface. Reading ICC_IAR1_EL1 acknowledges at most one
        // Group-1 interrupt; the ActiveGicV3 owner serializes this side effect.
        unsafe {
            asm!(
                "mrs {value}, ICC_IAR1_EL1",
                value = out(reg) value,
                options(nomem, nostack, preserves_flags)
            );
        }
        Ok(value as u32)
    }

    fn end_group1(&mut self, intid: u32) -> Result<(), Self::Error> {
        let value = u64::from(intid & ICC_IAR1_INTID_MASK);
        // SAFETY: the ActiveGicV3 acknowledgement token proves this exact
        // INTID was returned by ICC_IAR1_EL1 and remains outstanding. The
        // prepared CPU interface selected combined EOI/deactivation mode.
        unsafe {
            asm!(
                "msr ICC_EOIR1_EL1, {value}",
                value = in(reg) value,
                options(nomem, nostack, preserves_flags)
            );
        }
        Ok(())
    }

    fn synchronize(&mut self) {
        // SAFETY: this barrier sequence accesses no memory through raw
        // pointers. It completes earlier device writes and synchronizes later
        // system-register observations on the uniquely owned boot CPU.
        unsafe {
            asm!("dsb sy", "isb", options(nostack, preserves_flags));
        }
    }
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

/// Proof that an IRQ dispatcher capable of handling timer and unexpected
/// Group-1 results has been permanently published for the current CPU.
#[derive(Debug)]
pub struct PublishedIrqDispatcher {
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl PublishedIrqDispatcher {
    /// Create the one-way dispatcher-publication proof.
    ///
    /// # Safety
    ///
    /// The caller must have installed a dispatcher reachable from every EL1
    /// IRQ vector that can run on this CPU. Its storage, exception stack, GIC
    /// owner lookup, and synchronization state must remain valid until the GIC
    /// is shut down. The dispatcher must complete every acknowledged non-
    /// special INTID, including unexpected ones, before returning.
    pub const unsafe fn assume_published() -> Self {
        Self {
            _not_send_or_sync: PhantomData,
        }
    }
}

/// Delivery transition or interrupt-cycle phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GicV3DeliveryStage {
    /// Validate and open Non-secure Group-1 distribution.
    EnableDistributor,
    /// Validate and open the current CPU's Group-1 interface.
    EnableCpuInterface,
    /// Read `ICC_IAR1_EL1`.
    Acknowledge,
    /// Write `ICC_EOIR1_EL1` in combined deactivate mode.
    EndOfInterrupt,
    /// Close the current CPU's Group-1 interface.
    DisableCpuInterface,
    /// Close distributor delivery and wait for completion.
    DisableDistributor,
}

/// Failure while changing delivery state or completing one IRQ cycle.
#[derive(Debug, Eq, PartialEq)]
pub enum GicV3DeliveryError<E> {
    /// One backend register operation failed.
    Access {
        /// Operation phase.
        stage: GicV3DeliveryStage,
        /// Backend-specific failure.
        error: E,
    },
    /// A distributor write-pending transition exceeded the configured budget.
    Timeout {
        /// Timed-out phase.
        stage: GicV3DeliveryStage,
    },
    /// Affinity routing was unexpectedly clear.
    AffinityRoutingUnavailable {
        /// Observed `GICD_CTLR` value.
        observed: u32,
    },
    /// Distributor Group-1 gates were not in the required state.
    DistributorGateState {
        /// Required enabled state.
        expected_enabled: bool,
        /// Observed `GICD_CTLR` value.
        observed: u32,
    },
    /// The current CPU Group-1 gate was not in the required state.
    CpuInterfaceGateState {
        /// Required enabled state.
        expected_enabled: bool,
        /// Observed `ICC_IGRPEN1_EL1` value.
        observed: u64,
    },
    /// `ICC_CTLR_EL1` did not retain combined EOI/deactivation mode.
    SplitEoiMode {
        /// Observed `ICC_CTLR_EL1` value.
        observed: u64,
    },
}

/// Failed activation retaining register and dispatcher ownership.
pub struct GicV3ActivationFailure<B: GicV3RegisterAccess> {
    backend: B,
    configuration: GicV3Configuration,
    dispatcher: PublishedIrqDispatcher,
    error: GicV3DeliveryError<B::Error>,
    recovery_error: Option<GicV3DeliveryError<B::Error>>,
}

/// Recoverable parts of a failed delivery activation.
pub type GicV3ActivationFailureParts<B> = (
    B,
    GicV3Configuration,
    PublishedIrqDispatcher,
    GicV3DeliveryError<<B as GicV3RegisterAccess>::Error>,
    Option<GicV3DeliveryError<<B as GicV3RegisterAccess>::Error>>,
);

impl<B: GicV3RegisterAccess> GicV3ActivationFailure<B> {
    /// Return the failure that stopped activation.
    pub const fn error(&self) -> &GicV3DeliveryError<B::Error> {
        &self.error
    }

    /// Return the first failure while attempting to close delivery again.
    pub const fn recovery_error(&self) -> Option<&GicV3DeliveryError<B::Error>> {
        self.recovery_error.as_ref()
    }

    /// Recover all ownership and failure evidence.
    ///
    /// Neither a successful recovery nor the absence of `recovery_error`
    /// authorizes IRQ unmasking; the caller must re-run preparation before a
    /// later activation attempt.
    pub fn into_parts(self) -> GicV3ActivationFailureParts<B> {
        (
            self.backend,
            self.configuration,
            self.dispatcher,
            self.error,
            self.recovery_error,
        )
    }
}

/// GICv3 owner with Group-1 distributor and CPU delivery enabled.
pub struct ActiveGicV3<B> {
    backend: B,
    configuration: GicV3Configuration,
    dispatcher: PublishedIrqDispatcher,
}

impl<B> ActiveGicV3<B> {
    /// Return the retained timer configuration.
    pub const fn configuration(&self) -> GicV3Configuration {
        self.configuration
    }

    /// Borrow the backend for read-only diagnostics.
    pub const fn backend(&self) -> &B {
        &self.backend
    }
}

/// Interrupt classification produced by one Group-1 acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GicV3InterruptSource {
    /// The configured non-secure physical timer PPI.
    Timer,
    /// An acknowledged INTID for which Phoenix has no published handler.
    Unexpected,
}

/// Result of reading `ICC_IAR1_EL1`.
#[must_use = "a non-spurious GIC acknowledgement must be completed"]
pub enum GicV3Acknowledge<'a, B: GicV3RegisterAccess> {
    /// One of the architectural special INTIDs 1020 through 1023. It must not
    /// be written to `ICC_EOIR1_EL1`.
    Spurious {
        /// Exact special INTID returned by the CPU interface.
        intid: u32,
    },
    /// A real acknowledged interrupt whose token must be completed.
    Interrupt(AcknowledgedGicV3Interrupt<'a, B>),
}

/// One outstanding Group-1 interrupt, borrowing the controller exclusively.
#[must_use = "dropping an acknowledged interrupt without completion leaves it active"]
pub struct AcknowledgedGicV3Interrupt<'a, B: GicV3RegisterAccess> {
    controller: &'a mut ActiveGicV3<B>,
    intid: u32,
    source: GicV3InterruptSource,
}

impl<'a, B: GicV3RegisterAccess> AcknowledgedGicV3Interrupt<'a, B> {
    /// Return the acknowledged architectural INTID.
    pub const fn intid(&self) -> u32 {
        self.intid
    }

    /// Classify the INTID against the configured timer PPI.
    pub const fn source(&self) -> GicV3InterruptSource {
        self.source
    }

    /// Perform combined priority drop and deactivation.
    ///
    /// On access failure, ownership of the still-outstanding token is returned
    /// so the caller can retry or retain explicit terminal failure evidence.
    pub fn complete(self) -> Result<(), GicV3CompletionFailure<'a, B>> {
        let Self {
            controller,
            intid,
            source,
        } = self;
        if let Err(error) = controller.backend.end_group1(intid) {
            return Err(GicV3CompletionFailure {
                acknowledgement: AcknowledgedGicV3Interrupt {
                    controller,
                    intid,
                    source,
                },
                error: GicV3DeliveryError::Access {
                    stage: GicV3DeliveryStage::EndOfInterrupt,
                    error,
                },
            });
        }
        controller.backend.synchronize();
        Ok(())
    }
}

/// Failed EOI retaining the outstanding acknowledgement token.
pub struct GicV3CompletionFailure<'a, B: GicV3RegisterAccess> {
    acknowledgement: AcknowledgedGicV3Interrupt<'a, B>,
    error: GicV3DeliveryError<B::Error>,
}

impl<'a, B: GicV3RegisterAccess> GicV3CompletionFailure<'a, B> {
    /// Return the exact completion failure.
    pub const fn error(&self) -> &GicV3DeliveryError<B::Error> {
        &self.error
    }

    /// Recover the outstanding token and failure evidence.
    pub fn into_parts(
        self,
    ) -> (
        AcknowledgedGicV3Interrupt<'a, B>,
        GicV3DeliveryError<B::Error>,
    ) {
        (self.acknowledgement, self.error)
    }
}

/// Failed shutdown retaining indeterminate hardware ownership.
pub struct GicV3ShutdownFailure<B: GicV3RegisterAccess> {
    backend: B,
    configuration: GicV3Configuration,
    dispatcher: PublishedIrqDispatcher,
    error: GicV3DeliveryError<B::Error>,
}

/// Recoverable parts of a failed delivery shutdown.
pub type GicV3ShutdownFailureParts<B> = (
    B,
    GicV3Configuration,
    PublishedIrqDispatcher,
    GicV3DeliveryError<<B as GicV3RegisterAccess>::Error>,
);

impl<B: GicV3RegisterAccess> GicV3ShutdownFailure<B> {
    /// Return the first shutdown failure.
    pub const fn error(&self) -> &GicV3DeliveryError<B::Error> {
        &self.error
    }

    /// Recover ownership and explicit indeterminate-state evidence.
    pub fn into_parts(self) -> GicV3ShutdownFailureParts<B> {
        (
            self.backend,
            self.configuration,
            self.dispatcher,
            self.error,
        )
    }
}

/// Open Group-1 delivery only after the IRQ dispatcher is published.
///
/// This transition does not change `DAIF.I`; the caller controls the separate
/// architectural exception mask.
///
/// # Safety
///
/// `PSTATE.I` must remain masked throughout this transition. The dispatcher
/// proof must describe the current CPU and this exact retained GIC owner, and
/// no other context may access the controller concurrently.
pub unsafe fn activate<B: GicV3RegisterAccess>(
    prepared: PreparedGicV3<B>,
    dispatcher: PublishedIrqDispatcher,
) -> Result<ActiveGicV3<B>, GicV3ActivationFailure<B>> {
    let PreparedGicV3 {
        mut backend,
        configuration,
    } = prepared;
    match enable_delivery(&mut backend, configuration) {
        Ok(()) => Ok(ActiveGicV3 {
            backend,
            configuration,
            dispatcher,
        }),
        Err(error) => {
            let recovery_error = close_delivery(&mut backend, configuration);
            Err(GicV3ActivationFailure {
                backend,
                configuration,
                dispatcher,
                error,
                recovery_error,
            })
        }
    }
}

impl<B: GicV3RegisterAccess> ActiveGicV3<B> {
    /// Acknowledge at most one Group-1 interrupt.
    ///
    /// Special INTIDs 1020 through 1023 are returned without an outstanding
    /// token and must not be EOI'd. Every other result exclusively borrows the
    /// controller until `AcknowledgedGicV3Interrupt::complete` succeeds.
    pub fn acknowledge(&mut self) -> Result<GicV3Acknowledge<'_, B>, GicV3DeliveryError<B::Error>> {
        let raw =
            self.backend
                .acknowledge_group1()
                .map_err(|error| GicV3DeliveryError::Access {
                    stage: GicV3DeliveryStage::Acknowledge,
                    error,
                })?;
        self.backend.synchronize();
        let intid = raw & ICC_IAR1_INTID_MASK;
        if (SPECIAL_INTID_MIN..=SPECIAL_INTID_MAX).contains(&intid) {
            return Ok(GicV3Acknowledge::Spurious { intid });
        }
        let source = if intid == u32::from(self.configuration.timer().intid()) {
            GicV3InterruptSource::Timer
        } else {
            GicV3InterruptSource::Unexpected
        };
        Ok(GicV3Acknowledge::Interrupt(AcknowledgedGicV3Interrupt {
            controller: self,
            intid,
            source,
        }))
    }

    /// Close CPU and distributor Group-1 delivery and recover prepared state.
    ///
    /// # Safety
    ///
    /// The caller must first mask `PSTATE.I`, prevent new handler entry, and
    /// ensure that no acknowledgement token is outstanding. The exclusive
    /// `self` value enforces only the last condition.
    pub unsafe fn shutdown(
        self,
    ) -> Result<(PreparedGicV3<B>, PublishedIrqDispatcher), GicV3ShutdownFailure<B>> {
        let Self {
            mut backend,
            configuration,
            dispatcher,
        } = self;
        if let Some(error) = close_delivery(&mut backend, configuration) {
            return Err(GicV3ShutdownFailure {
                backend,
                configuration,
                dispatcher,
                error,
            });
        }
        Ok((
            PreparedGicV3 {
                backend,
                configuration,
            },
            dispatcher,
        ))
    }
}

fn enable_delivery<B: GicV3RegisterAccess>(
    backend: &mut B,
    configuration: GicV3Configuration,
) -> Result<(), GicV3DeliveryError<B::Error>> {
    let cpu_stage = GicV3DeliveryStage::EnableCpuInterface;
    let cpu = delivery_read_cpu(backend, cpu_stage, CpuInterfaceRegister::GroupEnable1)?;
    if cpu & 1 != 0 {
        return Err(GicV3DeliveryError::CpuInterfaceGateState {
            expected_enabled: false,
            observed: cpu,
        });
    }
    let control = delivery_read_cpu(backend, cpu_stage, CpuInterfaceRegister::Control)?;
    if control & ICC_CTLR_EL1_EOI_MODE != 0 {
        return Err(GicV3DeliveryError::SplitEoiMode { observed: control });
    }

    let distributor_stage = GicV3DeliveryStage::EnableDistributor;
    let control = delivery_read_distributor(backend, distributor_stage)?;
    if control & GICD_CTLR_ARE_NS == 0 {
        return Err(GicV3DeliveryError::AffinityRoutingUnavailable { observed: control });
    }
    if control & (GICD_CTLR_ENABLE_G1 | GICD_CTLR_ENABLE_G1A) != 0 {
        return Err(GicV3DeliveryError::DistributorGateState {
            expected_enabled: false,
            observed: control,
        });
    }
    delivery_write_distributor(
        backend,
        distributor_stage,
        control | GICD_CTLR_ENABLE_NONSECURE_GROUP1,
    )?;
    backend.synchronize();
    delivery_poll_distributor(backend, configuration, distributor_stage)?;
    let observed = delivery_read_distributor(backend, distributor_stage)?;
    if observed & GICD_CTLR_ARE_NS == 0 {
        return Err(GicV3DeliveryError::AffinityRoutingUnavailable { observed });
    }
    if observed & GICD_CTLR_ENABLE_NONSECURE_GROUP1 == 0 || observed & GICD_CTLR_ENABLE_G1 != 0 {
        return Err(GicV3DeliveryError::DistributorGateState {
            expected_enabled: true,
            observed,
        });
    }

    delivery_write_cpu(backend, cpu_stage, CpuInterfaceRegister::GroupEnable1, 1)?;
    backend.synchronize();
    let observed = delivery_read_cpu(backend, cpu_stage, CpuInterfaceRegister::GroupEnable1)?;
    if observed & 1 == 0 {
        return Err(GicV3DeliveryError::CpuInterfaceGateState {
            expected_enabled: true,
            observed,
        });
    }
    Ok(())
}

fn close_delivery<B: GicV3RegisterAccess>(
    backend: &mut B,
    configuration: GicV3Configuration,
) -> Option<GicV3DeliveryError<B::Error>> {
    let mut first = None;
    let cpu_stage = GicV3DeliveryStage::DisableCpuInterface;
    if let Err(error) =
        delivery_write_cpu(backend, cpu_stage, CpuInterfaceRegister::GroupEnable1, 0)
    {
        first = Some(error);
    }
    backend.synchronize();
    match delivery_read_cpu(backend, cpu_stage, CpuInterfaceRegister::GroupEnable1) {
        Ok(observed) if observed & 1 != 0 && first.is_none() => {
            first = Some(GicV3DeliveryError::CpuInterfaceGateState {
                expected_enabled: false,
                observed,
            });
        }
        Err(error) if first.is_none() => first = Some(error),
        Ok(_) | Err(_) => {}
    }

    let distributor_stage = GicV3DeliveryStage::DisableDistributor;
    match delivery_read_distributor(backend, distributor_stage) {
        Ok(control) => {
            if let Err(error) = delivery_write_distributor(
                backend,
                distributor_stage,
                control & !(GICD_CTLR_ENABLE_G1 | GICD_CTLR_ENABLE_G1A),
            ) && first.is_none()
            {
                first = Some(error);
            }
            backend.synchronize();
            if let Err(error) = delivery_poll_distributor(backend, configuration, distributor_stage)
                && first.is_none()
            {
                first = Some(error);
            }
            match delivery_read_distributor(backend, distributor_stage) {
                Ok(observed) => {
                    let error = if observed & GICD_CTLR_ARE_NS == 0 {
                        Some(GicV3DeliveryError::AffinityRoutingUnavailable { observed })
                    } else if observed & (GICD_CTLR_ENABLE_G1 | GICD_CTLR_ENABLE_G1A) != 0 {
                        Some(GicV3DeliveryError::DistributorGateState {
                            expected_enabled: false,
                            observed,
                        })
                    } else {
                        None
                    };
                    if first.is_none() {
                        first = error;
                    }
                }
                Err(error) if first.is_none() => first = Some(error),
                Err(_) => {}
            }
        }
        Err(error) if first.is_none() => first = Some(error),
        Err(_) => {}
    }
    first
}

fn delivery_poll_distributor<B: GicV3RegisterAccess>(
    backend: &mut B,
    configuration: GicV3Configuration,
    stage: GicV3DeliveryStage,
) -> Result<(), GicV3DeliveryError<B::Error>> {
    for _ in 0..configuration.poll_budget().get() {
        if delivery_read_distributor(backend, stage)? & GICD_CTLR_RWP == 0 {
            return Ok(());
        }
    }
    Err(GicV3DeliveryError::Timeout { stage })
}

fn delivery_read_distributor<B: GicV3RegisterAccess>(
    backend: &mut B,
    stage: GicV3DeliveryStage,
) -> Result<u32, GicV3DeliveryError<B::Error>> {
    backend
        .read_distributor(GICD_CTLR)
        .map_err(|error| GicV3DeliveryError::Access { stage, error })
}

fn delivery_write_distributor<B: GicV3RegisterAccess>(
    backend: &mut B,
    stage: GicV3DeliveryStage,
    value: u32,
) -> Result<(), GicV3DeliveryError<B::Error>> {
    backend
        .write_distributor(GICD_CTLR, value)
        .map_err(|error| GicV3DeliveryError::Access { stage, error })
}

fn delivery_read_cpu<B: GicV3RegisterAccess>(
    backend: &mut B,
    stage: GicV3DeliveryStage,
    register: CpuInterfaceRegister,
) -> Result<u64, GicV3DeliveryError<B::Error>> {
    backend
        .read_cpu_interface(register)
        .map_err(|error| GicV3DeliveryError::Access { stage, error })
}

fn delivery_write_cpu<B: GicV3RegisterAccess>(
    backend: &mut B,
    stage: GicV3DeliveryStage,
    register: CpuInterfaceRegister,
    value: u64,
) -> Result<(), GicV3DeliveryError<B::Error>> {
    backend
        .write_cpu_interface(register, value)
        .map_err(|error| GicV3DeliveryError::Access { stage, error })
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
        CpuAffinity, CpuInterfaceRegister, GIC_DISTRIBUTOR_MIN_SIZE, GIC_REDISTRIBUTOR_FRAME_SIZE,
        GICD_CTLR, GICD_CTLR_ARE_NS, GICD_CTLR_ENABLE_G1, GICD_CTLR_ENABLE_G1A, GICD_CTLR_RWP,
        GICD_PIDR2, GICR_ICACTIVER0, GICR_ICENABLER0, GICR_ICFGR1, GICR_ICPENDR0, GICR_IGROUPR0,
        GICR_IGRPMODR0, GICR_IPRIORITYR, GICR_ISENABLER0, GICR_PIDR2, GICR_TYPER, GICR_TYPER_LAST,
        GICR_TYPER_VLPIS, GICR_WAKER, GICR_WAKER_CHILDREN_ASLEEP, GICR_WAKER_PROCESSOR_SLEEP,
        GicRedistributorDiscoveryAccess, GicV3Acknowledge, GicV3Configuration,
        GicV3ConfigurationError, GicV3DeliveryError, GicV3DeliveryStage, GicV3InterruptSource,
        GicV3MmioError, GicV3MmioLayout, GicV3PreparationError, GicV3PreparationStage,
        GicV3RegisterAccess, ICC_CTLR_EL1_EOI_MODE, ICC_SRE_EL1_SRE, InterruptTrigger,
        PrivateInterruptError, PrivatePeripheralInterrupt, PublishedIrqDispatcher,
        RedistributorSelectionError, SPECIAL_INTID_MAX, activate, prepare, select_redistributor,
    };
    use crate::memory::{AddressRange, PAGE_SIZE, PhysAddr, VirtAddr};

    #[derive(Default)]
    struct DiscoveryRegisters {
        values: BTreeMap<usize, u64>,
    }

    impl DiscoveryRegisters {
        fn add_frame(&mut self, base: usize, affinity: u32, flags: u64, revision: u8) {
            self.values
                .insert(base + GICR_PIDR2, u64::from(revision) << 4);
            self.values
                .insert(base + GICR_TYPER, u64::from(affinity) << 32 | flags);
        }
    }

    impl GicRedistributorDiscoveryAccess for DiscoveryRegisters {
        type Error = usize;

        fn read_u32(&mut self, address: PhysAddr) -> Result<u32, Self::Error> {
            self.values
                .get(&address.as_usize())
                .copied()
                .map(|value| value as u32)
                .ok_or(address.as_usize())
        }

        fn read_u64(&mut self, address: PhysAddr) -> Result<u64, Self::Error> {
            self.values
                .get(&address.as_usize())
                .copied()
                .ok_or(address.as_usize())
        }
    }

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
        AcknowledgeGroup1(u32),
        EndGroup1(u32),
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
        distributor_rwp_forever: bool,
        acknowledgements: Vec<u32>,
        next_acknowledgement: usize,
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
                distributor_rwp_forever: false,
                acknowledgements: Vec::new(),
                next_acknowledgement: 0,
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
            let mut value = self.distributor(offset);
            if offset == GICD_CTLR && self.distributor_rwp_forever {
                value |= GICD_CTLR_RWP;
            }
            Ok(value)
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

        fn acknowledge_group1(&mut self) -> Result<u32, Self::Error> {
            self.before_operation()?;
            let value = self
                .acknowledgements
                .get(self.next_acknowledgement)
                .copied()
                .unwrap_or(SPECIAL_INTID_MAX);
            self.next_acknowledgement += 1;
            self.operations.push(Operation::AcknowledgeGroup1(value));
            Ok(value)
        }

        fn end_group1(&mut self, intid: u32) -> Result<(), Self::Error> {
            self.before_operation()?;
            self.operations.push(Operation::EndGroup1(intid));
            Ok(())
        }

        fn synchronize(&mut self) {
            self.operations.push(Operation::Synchronize);
        }
    }

    #[test]
    fn affinity_selection_skips_vlpi_frames_and_returns_exact_rd_sgi_pair() {
        let base = 0x080a_0000;
        let affinity = CpuAffinity::from_mpidr(0x0000_00aa_00bb_ccdd);
        assert_eq!(affinity.raw(), 0xaabb_ccdd);
        let mut registers = DiscoveryRegisters::default();
        registers.add_frame(base, 0x0000_0001, GICR_TYPER_VLPIS, 3);
        let selected_base = base + 2 * GIC_REDISTRIBUTOR_FRAME_SIZE;
        registers.add_frame(selected_base, affinity.raw(), GICR_TYPER_LAST, 4);
        let region = AddressRange::new(
            PhysAddr::new(base),
            PhysAddr::new(selected_base + GIC_REDISTRIBUTOR_FRAME_SIZE),
        )
        .unwrap();

        let selected = select_redistributor(region, affinity, &mut registers).unwrap();
        assert_eq!(selected.range().start(), PhysAddr::new(selected_base));
        assert_eq!(selected.range().len(), GIC_REDISTRIBUTOR_FRAME_SIZE);
        assert_eq!(selected.typer() & GICR_TYPER_LAST, GICR_TYPER_LAST);
    }

    #[test]
    fn affinity_selection_rejects_ambiguous_invalid_and_unterminated_regions() {
        let base = 0x080a_0000;
        let affinity = CpuAffinity::from_mpidr(2);
        let two_frames = AddressRange::new(
            PhysAddr::new(base),
            PhysAddr::new(base + 2 * GIC_REDISTRIBUTOR_FRAME_SIZE),
        )
        .unwrap();

        let mut duplicate = DiscoveryRegisters::default();
        duplicate.add_frame(base, affinity.raw(), 0, 3);
        duplicate.add_frame(
            base + GIC_REDISTRIBUTOR_FRAME_SIZE,
            affinity.raw(),
            GICR_TYPER_LAST,
            3,
        );
        assert_eq!(
            select_redistributor(two_frames, affinity, &mut duplicate),
            Err(RedistributorSelectionError::DuplicateAffinity {
                affinity: affinity.raw()
            })
        );

        let mut unsupported = DiscoveryRegisters::default();
        unsupported.add_frame(base, affinity.raw(), GICR_TYPER_LAST, 2);
        assert_eq!(
            select_redistributor(two_frames, affinity, &mut unsupported),
            Err(RedistributorSelectionError::UnsupportedArchitecture { base, revision: 2 })
        );

        let one_frame = AddressRange::new(
            PhysAddr::new(base),
            PhysAddr::new(base + GIC_REDISTRIBUTOR_FRAME_SIZE),
        )
        .unwrap();
        let mut unterminated = DiscoveryRegisters::default();
        unterminated.add_frame(base, affinity.raw(), 0, 3);
        assert_eq!(
            select_redistributor(one_frame, affinity, &mut unterminated),
            Err(RedistributorSelectionError::MissingLast)
        );

        let truncated = AddressRange::new(
            PhysAddr::new(base),
            PhysAddr::new(base + GIC_REDISTRIBUTOR_FRAME_SIZE + PAGE_SIZE),
        )
        .unwrap();
        assert_eq!(
            select_redistributor(truncated, affinity, &mut unterminated),
            Err(RedistributorSelectionError::TruncatedFrame {
                base: base + GIC_REDISTRIBUTOR_FRAME_SIZE
            })
        );

        let mut other = DiscoveryRegisters::default();
        other.add_frame(base, affinity.raw() + 1, GICR_TYPER_LAST, 3);
        assert_eq!(
            select_redistributor(one_frame, affinity, &mut other),
            Err(RedistributorSelectionError::AffinityNotFound {
                affinity: affinity.raw()
            })
        );

        let missing_address = base + GICR_PIDR2;
        assert_eq!(
            select_redistributor(one_frame, affinity, &mut DiscoveryRegisters::default()),
            Err(RedistributorSelectionError::Access {
                address: missing_address,
                error: missing_address,
            })
        );
    }

    #[test]
    fn mapped_register_layout_checks_exact_ownership_and_every_offset() {
        let distributor_start = 0xffff_ff80_0800_0000;
        let redistributor_start = 0xffff_ff80_080a_0000;
        let distributor = AddressRange::new(
            VirtAddr::new(distributor_start),
            VirtAddr::new(distributor_start + GIC_DISTRIBUTOR_MIN_SIZE),
        )
        .unwrap();
        let redistributor = AddressRange::new(
            VirtAddr::new(redistributor_start),
            VirtAddr::new(redistributor_start + GIC_REDISTRIBUTOR_FRAME_SIZE),
        )
        .unwrap();
        let layout = GicV3MmioLayout::new(distributor, redistributor).unwrap();
        assert_eq!(
            layout.distributor_register(GICD_PIDR2),
            Ok(distributor_start + GICD_PIDR2)
        );
        assert_eq!(
            layout.redistributor_register(GICR_PIDR2),
            Ok(redistributor_start + GICR_PIDR2)
        );
        assert_eq!(
            layout.distributor_register(1),
            Err(GicV3MmioError::MisalignedOffset {
                component: "GICD",
                offset: 1
            })
        );
        assert_eq!(
            layout.redistributor_register(GIC_REDISTRIBUTOR_FRAME_SIZE),
            Err(GicV3MmioError::RegisterOutOfRange {
                component: "GICR",
                offset: GIC_REDISTRIBUTOR_FRAME_SIZE
            })
        );

        let short = AddressRange::new(
            VirtAddr::new(redistributor_start),
            VirtAddr::new(redistributor_start + GIC_REDISTRIBUTOR_FRAME_SIZE - PAGE_SIZE),
        )
        .unwrap();
        assert_eq!(
            GicV3MmioLayout::new(distributor, short),
            Err(GicV3MmioError::InvalidRange { component: "GICR" })
        );
        let overlapping = AddressRange::new(
            VirtAddr::new(distributor_start),
            VirtAddr::new(distributor_start + GIC_REDISTRIBUTOR_FRAME_SIZE),
        )
        .unwrap();
        assert_eq!(
            GicV3MmioLayout::new(distributor, overlapping),
            Err(GicV3MmioError::OverlappingRanges)
        );
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

    fn prepared_registers() -> super::PreparedGicV3<MockRegisters> {
        // SAFETY: the deterministic mock is uniquely owned and models the
        // required externally masked, single-CPU preparation environment.
        unsafe {
            prepare(
                MockRegisters::valid(),
                configuration(InterruptTrigger::LevelHigh),
            )
        }
        .unwrap()
    }

    fn published_dispatcher() -> PublishedIrqDispatcher {
        // SAFETY: delivery tests model a permanently published dispatcher
        // that classifies every result and completes every real INTID.
        unsafe { PublishedIrqDispatcher::assume_published() }
    }

    #[test]
    fn activation_opens_only_non_secure_group1_and_shutdown_restores_prepared_state() {
        let mut prepared = prepared_registers();
        prepared.backend.operations.clear();
        prepared.backend.operation_count = 0;
        // SAFETY: the mock models masked IRQs, one CPU, and the dispatcher
        // token above models a handler published before either gate opens.
        let active = match unsafe { activate(prepared, published_dispatcher()) } {
            Ok(active) => active,
            Err(failure) => panic!("activation failed unexpectedly: {:?}", failure.error()),
        };
        assert_eq!(
            active.backend().distributor(GICD_CTLR),
            GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_G1A
        );
        assert_eq!(active.backend().cpu(CpuInterfaceRegister::GroupEnable1), 1);
        let operations = &active.backend().operations;
        let distributor_enable = operations
            .iter()
            .position(|operation| {
                *operation
                    == Operation::WriteDistributor(
                        GICD_CTLR,
                        GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_G1A,
                    )
            })
            .unwrap();
        let cpu_enable = operations
            .iter()
            .position(|operation| {
                *operation == Operation::WriteCpu(CpuInterfaceRegister::GroupEnable1, 1)
            })
            .unwrap();
        assert!(distributor_enable < cpu_enable);

        // SAFETY: the mock models DAIF.I masked and no outstanding token.
        let (prepared, _dispatcher) = match unsafe { active.shutdown() } {
            Ok(parts) => parts,
            Err(failure) => panic!("shutdown failed unexpectedly: {:?}", failure.error()),
        };
        assert_eq!(prepared.backend().distributor(GICD_CTLR), GICD_CTLR_ARE_NS);
        assert_eq!(
            prepared.backend().cpu(CpuInterfaceRegister::GroupEnable1),
            0
        );
        let operations = &prepared.backend().operations;
        let cpu_disable = operations
            .iter()
            .rposition(|operation| {
                *operation == Operation::WriteCpu(CpuInterfaceRegister::GroupEnable1, 0)
            })
            .unwrap();
        let distributor_disable = operations
            .iter()
            .rposition(|operation| {
                *operation == Operation::WriteDistributor(GICD_CTLR, GICD_CTLR_ARE_NS)
            })
            .unwrap();
        assert!(cpu_disable < distributor_disable);
    }

    #[test]
    fn activation_failure_closes_gates_and_retains_recovery_evidence() {
        let mut prepared = prepared_registers();
        prepared.backend.operations.clear();
        prepared.backend.operation_count = 0;
        prepared.backend.fail_at = Some(7);
        // SAFETY: the deterministic mock satisfies the masked single-owner
        // activation contract and injects one CPU-gate write failure.
        let failure = match unsafe { activate(prepared, published_dispatcher()) } {
            Ok(_) => panic!("injected activation failure unexpectedly succeeded"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            &GicV3DeliveryError::Access {
                stage: GicV3DeliveryStage::EnableCpuInterface,
                error: MockError::Injected,
            }
        );
        assert_eq!(failure.recovery_error(), None);
        let (registers, _, _dispatcher, _, _) = failure.into_parts();
        assert_eq!(registers.distributor(GICD_CTLR), GICD_CTLR_ARE_NS);
        assert_eq!(registers.cpu(CpuInterfaceRegister::GroupEnable1), 0);

        let mut split_eoi = prepared_registers();
        let control = split_eoi.backend.cpu(CpuInterfaceRegister::Control);
        split_eoi.backend.cpu.insert(
            CpuInterfaceRegister::Control,
            control | ICC_CTLR_EL1_EOI_MODE,
        );
        // SAFETY: the mock satisfies the ownership contract and deliberately
        // violates the required combined EOI/deactivation readback.
        let failure = match unsafe { activate(split_eoi, published_dispatcher()) } {
            Ok(_) => panic!("split EOI mode unexpectedly activated"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            &GicV3DeliveryError::SplitEoiMode {
                observed: control | ICC_CTLR_EL1_EOI_MODE,
            }
        );

        let mut timeout = prepared_registers();
        timeout.backend.distributor_rwp_forever = true;
        // SAFETY: the mock satisfies the activation contract but models a
        // distributor whose write-pending indication never clears.
        let failure = match unsafe { activate(timeout, published_dispatcher()) } {
            Ok(_) => panic!("stuck distributor unexpectedly activated"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            &GicV3DeliveryError::Timeout {
                stage: GicV3DeliveryStage::EnableDistributor,
            }
        );
        assert_eq!(
            failure.recovery_error(),
            Some(&GicV3DeliveryError::Timeout {
                stage: GicV3DeliveryStage::DisableDistributor,
            })
        );
    }

    #[test]
    fn acknowledgement_classifies_special_timer_and_unexpected_intids() {
        let mut prepared = prepared_registers();
        prepared.backend.acknowledgements = std::vec![1020, 1021, 1022, 1023, 0xab00_001e, 29,];
        // SAFETY: the mock models a published dispatcher with IRQs externally
        // masked during gate activation.
        let mut active = match unsafe { activate(prepared, published_dispatcher()) } {
            Ok(active) => active,
            Err(failure) => panic!("activation failed unexpectedly: {:?}", failure.error()),
        };
        active.backend.operations.clear();
        for expected in 1020..=1023 {
            match active.acknowledge().unwrap() {
                GicV3Acknowledge::Spurious { intid } => assert_eq!(intid, expected),
                GicV3Acknowledge::Interrupt(_) => panic!("special INTID became a real interrupt"),
            }
        }

        let timer = match active.acknowledge().unwrap() {
            GicV3Acknowledge::Interrupt(interrupt) => interrupt,
            GicV3Acknowledge::Spurious { .. } => panic!("timer became spurious"),
        };
        assert_eq!(timer.intid(), 30);
        assert_eq!(timer.source(), GicV3InterruptSource::Timer);
        timer.complete().unwrap_or_else(|failure| {
            panic!("timer EOI failed unexpectedly: {:?}", failure.error())
        });

        let unexpected = match active.acknowledge().unwrap() {
            GicV3Acknowledge::Interrupt(interrupt) => interrupt,
            GicV3Acknowledge::Spurious { .. } => panic!("unexpected INTID became spurious"),
        };
        assert_eq!(unexpected.intid(), 29);
        assert_eq!(unexpected.source(), GicV3InterruptSource::Unexpected);
        unexpected
            .complete()
            .unwrap_or_else(|failure| panic!("unexpected INTID EOI failed: {:?}", failure.error()));

        assert_eq!(
            active
                .backend()
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::EndGroup1(_)))
                .copied()
                .collect::<Vec<_>>(),
            std::vec![Operation::EndGroup1(30), Operation::EndGroup1(29)]
        );
    }

    #[test]
    fn failed_eoi_returns_the_outstanding_token_and_shutdown_failure_is_explicit() {
        let mut prepared = prepared_registers();
        prepared.backend.acknowledgements = std::vec![30];
        // SAFETY: the mock models a published dispatcher with IRQs externally
        // masked during activation.
        let mut active = match unsafe { activate(prepared, published_dispatcher()) } {
            Ok(active) => active,
            Err(failure) => panic!("activation failed unexpectedly: {:?}", failure.error()),
        };
        active.backend.operation_count = 0;
        active.backend.fail_at = Some(2);
        let acknowledgement = match active.acknowledge().unwrap() {
            GicV3Acknowledge::Interrupt(interrupt) => interrupt,
            GicV3Acknowledge::Spurious { .. } => panic!("timer became spurious"),
        };
        let failure = match acknowledgement.complete() {
            Ok(()) => panic!("injected EOI failure unexpectedly succeeded"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            &GicV3DeliveryError::Access {
                stage: GicV3DeliveryStage::EndOfInterrupt,
                error: MockError::Injected,
            }
        );
        let (acknowledgement, _) = failure.into_parts();
        acknowledgement
            .complete()
            .unwrap_or_else(|failure| panic!("EOI retry failed: {:?}", failure.error()));

        active.backend.operation_count = 0;
        active.backend.fail_at = Some(1);
        // SAFETY: the acknowledgement completed and the mock models DAIF.I
        // masked before the shutdown attempt.
        let failure = match unsafe { active.shutdown() } {
            Ok(_) => panic!("injected shutdown failure unexpectedly succeeded"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            &GicV3DeliveryError::Access {
                stage: GicV3DeliveryStage::DisableCpuInterface,
                error: MockError::Injected,
            }
        );
        let (registers, _, _dispatcher, _) = failure.into_parts();
        assert_eq!(registers.cpu(CpuInterfaceRegister::GroupEnable1), 1);
        assert_eq!(registers.distributor(GICD_CTLR), GICD_CTLR_ARE_NS);
    }
}
