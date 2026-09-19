//! Strict interrupt topology discovery for the pinned AArch64 QEMU `virt` board.

use core::str;

pub use crate::arch::aarch64::gicv3::{InterruptTrigger, PrivatePeripheralInterrupt as GicPpi};
use crate::arch::aarch64::gicv3::{PPI_INTID_MAX, PPI_INTID_MIN};
use crate::dtb::{DeviceTree, Error as DeviceTreeError, Event};
use crate::memory::{AddressRange, PhysAddr};

const ROOT_ADDRESS_CELLS: u32 = 2;
const ROOT_SIZE_CELLS: u32 = 2;
const GIC_INTERRUPT_CELLS: u32 = 3;
const GIC_DISTRIBUTOR_ALIGNMENT: u64 = 64 * 1024;
const GIC_DISTRIBUTOR_MIN_SIZE: u64 = 64 * 1024;
const GIC_REDISTRIBUTOR_ALIGNMENT: u64 = 128 * 1024;
const GIC_REDISTRIBUTOR_MIN_SIZE: u64 = 128 * 1024;
const MAX_PHYSICAL_ADDRESS_EXCLUSIVE: u64 = 1_u64 << 48;
const GIC_PPI_TYPE: u32 = 1;
const GIC_PPI_MAX_NUMBER: u32 = (PPI_INTID_MAX - PPI_INTID_MIN) as u32;
const GIC_TRIGGER_MASK: u32 = 0x0f;
const IRQ_TYPE_EDGE_RISING: u32 = 1;
const IRQ_TYPE_LEVEL_HIGH: u32 = 4;
const ARCH_TIMER_INTERRUPT_COUNT: usize = 4;
const INTERRUPT_SPECIFIER_CELLS: usize = 3;
const INTERRUPT_SPECIFIER_BYTES: usize = INTERRUPT_SPECIFIER_CELLS * 4;

/// Four interrupts in the order required by the ARM architected-timer binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArchitectedTimerInterrupts {
    secure_physical: GicPpi,
    non_secure_physical: GicPpi,
    virtual_timer: GicPpi,
    hypervisor: GicPpi,
}

impl ArchitectedTimerInterrupts {
    /// Secure physical timer interrupt.
    pub const fn secure_physical(self) -> GicPpi {
        self.secure_physical
    }

    /// Non-secure physical timer interrupt used by Phoenix's `CNTP_*` owner.
    pub const fn non_secure_physical(self) -> GicPpi {
        self.non_secure_physical
    }

    /// Virtual timer interrupt.
    pub const fn virtual_timer(self) -> GicPpi {
        self.virtual_timer
    }

    /// Hypervisor physical timer interrupt.
    pub const fn hypervisor(self) -> GicPpi {
        self.hypervisor
    }
}

/// Validated GICv3 and architected-timer topology for QEMU `virt`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptTopology {
    controller_phandle: u32,
    distributor: AddressRange<PhysAddr>,
    redistributor: AddressRange<PhysAddr>,
    timers: ArchitectedTimerInterrupts,
}

impl InterruptTopology {
    /// Return the nonzero DTB handle referenced by interrupt consumers.
    pub const fn controller_phandle(self) -> u32 {
        self.controller_phandle
    }

    /// Return the complete first GIC distributor MMIO region.
    pub const fn distributor(self) -> AddressRange<PhysAddr> {
        self.distributor
    }

    /// Return the complete first GIC redistributor MMIO region.
    pub const fn redistributor(self) -> AddressRange<PhysAddr> {
        self.redistributor
    }

    /// Return all four architected timer PPIs in binding order.
    pub const fn timers(self) -> ArchitectedTimerInterrupts {
        self.timers
    }
}

/// Failure while interpreting the pinned QEMU `virt` interrupt topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptTopologyError {
    /// A validated tree unexpectedly failed during a later traversal.
    DeviceTree(DeviceTreeError),
    /// A required root property was absent.
    MissingRootProperty {
        /// Missing property.
        name: &'static str,
    },
    /// A root property appeared more than once.
    DuplicateRootProperty {
        /// Duplicated property.
        name: &'static str,
    },
    /// The pinned platform requires two 32-bit address and size cells.
    UnsupportedRootCells {
        /// Observed address cell count.
        address: u32,
        /// Observed size cell count.
        size: u32,
    },
    /// A required direct root child was absent.
    MissingNode {
        /// Logical node role.
        node: &'static str,
    },
    /// More than one direct root child claimed a required role.
    DuplicateNode {
        /// Logical node role.
        node: &'static str,
    },
    /// A required node is disabled or has another unsupported status.
    UnavailableNode {
        /// Logical node role.
        node: &'static str,
    },
    /// A required node lacks its expected compatible string.
    IncompatibleNode {
        /// Logical node role.
        node: &'static str,
    },
    /// A required node property was absent.
    MissingNodeProperty {
        /// Logical node role.
        node: &'static str,
        /// Missing property.
        name: &'static str,
    },
    /// A node property appeared more than once.
    DuplicateNodeProperty {
        /// Logical node role.
        node: &'static str,
        /// Duplicated property.
        name: &'static str,
    },
    /// A property did not have its required encoded shape.
    MalformedProperty {
        /// Logical node role.
        node: &'static str,
        /// Malformed property.
        name: &'static str,
    },
    /// `phandle` and `linux,phandle` disagree.
    ConflictingPhandle {
        /// Standard handle.
        phandle: u32,
        /// Legacy handle.
        linux_phandle: u32,
    },
    /// A consumer references a controller other than the discovered GICv3.
    WrongInterruptParent {
        /// Logical consumer (`root` or `timer`).
        node: &'static str,
        /// GICv3 controller handle.
        expected: u32,
        /// Referenced handle.
        observed: u32,
    },
    /// The GIC register list does not contain exactly distributor and redistributor entries.
    InvalidGicRegisterList {
        /// Observed byte length.
        length: usize,
        /// Required byte length.
        expected: usize,
    },
    /// A GIC MMIO range is empty, overflowing, too small, misaligned, or too wide.
    InvalidGicRange {
        /// Logical region (`distributor` or `redistributor`).
        region: &'static str,
        /// Supplied physical address.
        address: u64,
        /// Supplied byte size.
        size: u64,
    },
    /// Distributor and redistributor regions overlap.
    OverlappingGicRanges,
    /// The architected timer does not supply exactly four three-cell specifiers.
    InvalidTimerInterruptList {
        /// Observed byte length.
        length: usize,
        /// Required byte length.
        expected: usize,
    },
    /// One architected timer entry is not a supported PPI specifier.
    InvalidTimerInterrupt {
        /// Binding-order index: secure, physical, virtual, hypervisor.
        index: usize,
        /// Raw interrupt type cell.
        interrupt_type: u32,
        /// Raw interrupt number cell.
        number: u32,
        /// Raw flags cell.
        flags: u32,
    },
    /// Two architected timer roles alias one PPI INTID.
    DuplicateTimerInterrupt {
        /// Repeated architectural interrupt ID.
        intid: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RootContract {
    address_cells: u32,
    size_cells: u32,
    interrupt_parent: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NodeKind {
    Gic,
    Timer,
}

impl NodeKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Gic => "gic",
            Self::Timer => "timer",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NodeProperties<'a> {
    compatible: Option<&'a [u8]>,
    status: Option<&'a [u8]>,
    reg: Option<&'a [u8]>,
    interrupt_controller: Option<&'a [u8]>,
    interrupt_cells: Option<&'a [u8]>,
    phandle: Option<&'a [u8]>,
    linux_phandle: Option<&'a [u8]>,
    redistributor_regions: Option<&'a [u8]>,
    interrupts: Option<&'a [u8]>,
    interrupt_parent: Option<&'a [u8]>,
}

/// Discover and validate the interrupt topology from a borrowed DTB.
pub fn discover(tree: DeviceTree<'_>) -> Result<InterruptTopology, InterruptTopologyError> {
    let root = root_contract(tree)?;
    if root.address_cells != ROOT_ADDRESS_CELLS || root.size_cells != ROOT_SIZE_CELLS {
        return Err(InterruptTopologyError::UnsupportedRootCells {
            address: root.address_cells,
            size: root.size_cells,
        });
    }
    let (gic, timer) = required_nodes(tree)?;
    let (controller_phandle, distributor, redistributor) = parse_gic(gic)?;
    if root.interrupt_parent != controller_phandle {
        return Err(InterruptTopologyError::WrongInterruptParent {
            node: "root",
            expected: controller_phandle,
            observed: root.interrupt_parent,
        });
    }
    let timers = parse_timer(timer, root.interrupt_parent, controller_phandle)?;
    Ok(InterruptTopology {
        controller_phandle,
        distributor,
        redistributor,
        timers,
    })
}

fn root_contract(tree: DeviceTree<'_>) -> Result<RootContract, InterruptTopologyError> {
    let mut depth = 0_usize;
    let mut address_cells = None;
    let mut size_cells = None;
    let mut interrupt_parent = None;
    for event in tree.events() {
        match event.map_err(InterruptTopologyError::DeviceTree)? {
            Event::BeginNode { .. } => depth += 1,
            Event::EndNode => depth -= 1,
            Event::Property { name, value } if depth == 1 => match name {
                "#address-cells" => set_root_scalar(&mut address_cells, "#address-cells", value)?,
                "#size-cells" => set_root_scalar(&mut size_cells, "#size-cells", value)?,
                "interrupt-parent" => {
                    set_root_scalar(&mut interrupt_parent, "interrupt-parent", value)?
                }
                _ => {}
            },
            Event::Property { .. } | Event::Nop | Event::End => {}
        }
    }
    Ok(RootContract {
        address_cells: address_cells.ok_or(InterruptTopologyError::MissingRootProperty {
            name: "#address-cells",
        })?,
        size_cells: size_cells.ok_or(InterruptTopologyError::MissingRootProperty {
            name: "#size-cells",
        })?,
        interrupt_parent: interrupt_parent.ok_or(InterruptTopologyError::MissingRootProperty {
            name: "interrupt-parent",
        })?,
    })
}

fn set_root_scalar(
    slot: &mut Option<u32>,
    name: &'static str,
    value: &[u8],
) -> Result<(), InterruptTopologyError> {
    if slot.is_some() {
        return Err(InterruptTopologyError::DuplicateRootProperty { name });
    }
    *slot = Some(parse_scalar("root", name, value)?);
    Ok(())
}

fn required_nodes(
    tree: DeviceTree<'_>,
) -> Result<(NodeProperties<'_>, NodeProperties<'_>), InterruptTopologyError> {
    let mut depth = 0_usize;
    let mut current: Option<(NodeKind, NodeProperties<'_>)> = None;
    let mut gic = None;
    let mut timer = None;
    for event in tree.events() {
        match event.map_err(InterruptTopologyError::DeviceTree)? {
            Event::BeginNode { name } => {
                depth += 1;
                if depth == 2 {
                    current = match base_name(name) {
                        "intc" | "interrupt-controller" => {
                            Some((NodeKind::Gic, NodeProperties::default()))
                        }
                        "timer" => Some((NodeKind::Timer, NodeProperties::default())),
                        _ => None,
                    };
                }
            }
            Event::EndNode => {
                if depth == 2
                    && let Some((kind, properties)) = current.take()
                {
                    let slot = match kind {
                        NodeKind::Gic => &mut gic,
                        NodeKind::Timer => &mut timer,
                    };
                    if slot.replace(properties).is_some() {
                        return Err(InterruptTopologyError::DuplicateNode { node: kind.name() });
                    }
                }
                depth -= 1;
            }
            Event::Property { name, value } if depth == 2 => {
                if let Some((kind, properties)) = &mut current {
                    capture_property(*kind, properties, name, value)?;
                }
            }
            Event::Property { .. } | Event::Nop | Event::End => {}
        }
    }
    Ok((
        gic.ok_or(InterruptTopologyError::MissingNode { node: "gic" })?,
        timer.ok_or(InterruptTopologyError::MissingNode { node: "timer" })?,
    ))
}

fn capture_property<'a>(
    kind: NodeKind,
    properties: &mut NodeProperties<'a>,
    name: &'a str,
    value: &'a [u8],
) -> Result<(), InterruptTopologyError> {
    let slot = match name {
        "compatible" => &mut properties.compatible,
        "status" => &mut properties.status,
        "reg" => &mut properties.reg,
        "interrupt-controller" => &mut properties.interrupt_controller,
        "#interrupt-cells" => &mut properties.interrupt_cells,
        "phandle" => &mut properties.phandle,
        "linux,phandle" => &mut properties.linux_phandle,
        "#redistributor-regions" => &mut properties.redistributor_regions,
        "interrupts" => &mut properties.interrupts,
        "interrupt-parent" => &mut properties.interrupt_parent,
        _ => return Ok(()),
    };
    if slot.replace(value).is_some() {
        return Err(InterruptTopologyError::DuplicateNodeProperty {
            node: kind.name(),
            name: property_name(name),
        });
    }
    Ok(())
}

fn parse_gic(
    properties: NodeProperties<'_>,
) -> Result<(u32, AddressRange<PhysAddr>, AddressRange<PhysAddr>), InterruptTopologyError> {
    validate_node_status(NodeKind::Gic, properties.status)?;
    require_compatible(NodeKind::Gic, properties.compatible, b"arm,gic-v3")?;
    let marker = required_property(
        NodeKind::Gic,
        "interrupt-controller",
        properties.interrupt_controller,
    )?;
    if !marker.is_empty() {
        return Err(InterruptTopologyError::MalformedProperty {
            node: "gic",
            name: "interrupt-controller",
        });
    }
    let interrupt_cells = parse_scalar(
        "gic",
        "#interrupt-cells",
        required_property(
            NodeKind::Gic,
            "#interrupt-cells",
            properties.interrupt_cells,
        )?,
    )?;
    if interrupt_cells != GIC_INTERRUPT_CELLS {
        return Err(InterruptTopologyError::MalformedProperty {
            node: "gic",
            name: "#interrupt-cells",
        });
    }
    if let Some(value) = properties.redistributor_regions
        && parse_scalar("gic", "#redistributor-regions", value)? != 1
    {
        return Err(InterruptTopologyError::MalformedProperty {
            node: "gic",
            name: "#redistributor-regions",
        });
    }
    let controller_phandle = parse_phandle(properties)?;
    let (distributor, redistributor) =
        parse_gic_ranges(required_property(NodeKind::Gic, "reg", properties.reg)?)?;
    Ok((controller_phandle, distributor, redistributor))
}

fn parse_phandle(properties: NodeProperties<'_>) -> Result<u32, InterruptTopologyError> {
    let phandle = properties
        .phandle
        .map(|value| parse_scalar("gic", "phandle", value))
        .transpose()?;
    let linux_phandle = properties
        .linux_phandle
        .map(|value| parse_scalar("gic", "linux,phandle", value))
        .transpose()?;
    if let (Some(phandle), Some(linux_phandle)) = (phandle, linux_phandle)
        && phandle != linux_phandle
    {
        return Err(InterruptTopologyError::ConflictingPhandle {
            phandle,
            linux_phandle,
        });
    }
    let phandle = phandle
        .or(linux_phandle)
        .ok_or(InterruptTopologyError::MissingNodeProperty {
            node: "gic",
            name: "phandle",
        })?;
    if phandle == 0 {
        return Err(InterruptTopologyError::MalformedProperty {
            node: "gic",
            name: "phandle",
        });
    }
    Ok(phandle)
}

fn parse_gic_ranges(
    value: &[u8],
) -> Result<(AddressRange<PhysAddr>, AddressRange<PhysAddr>), InterruptTopologyError> {
    const ENTRY_BYTES: usize = (ROOT_ADDRESS_CELLS as usize + ROOT_SIZE_CELLS as usize) * 4;
    const EXPECTED_BYTES: usize = ENTRY_BYTES * 2;
    if value.len() != EXPECTED_BYTES {
        return Err(InterruptTopologyError::InvalidGicRegisterList {
            length: value.len(),
            expected: EXPECTED_BYTES,
        });
    }
    let distributor = parse_gic_range(
        "distributor",
        &value[..ENTRY_BYTES],
        GIC_DISTRIBUTOR_ALIGNMENT,
        GIC_DISTRIBUTOR_MIN_SIZE,
    )?;
    let redistributor = parse_gic_range(
        "redistributor",
        &value[ENTRY_BYTES..],
        GIC_REDISTRIBUTOR_ALIGNMENT,
        GIC_REDISTRIBUTOR_MIN_SIZE,
    )?;
    if distributor.overlaps(redistributor) {
        return Err(InterruptTopologyError::OverlappingGicRanges);
    }
    Ok((distributor, redistributor))
}

fn parse_gic_range(
    region: &'static str,
    entry: &[u8],
    alignment: u64,
    minimum_size: u64,
) -> Result<AddressRange<PhysAddr>, InterruptTopologyError> {
    let address = read_two_cells(&entry[..8]);
    let size = read_two_cells(&entry[8..]);
    let Some(end) = address.checked_add(size) else {
        return Err(InterruptTopologyError::InvalidGicRange {
            region,
            address,
            size,
        });
    };
    if size < minimum_size
        || !address.is_multiple_of(alignment)
        || !size.is_multiple_of(4096)
        || end > MAX_PHYSICAL_ADDRESS_EXCLUSIVE
    {
        return Err(InterruptTopologyError::InvalidGicRange {
            region,
            address,
            size,
        });
    }
    let (Ok(start), Ok(end)) = (usize::try_from(address), usize::try_from(end)) else {
        return Err(InterruptTopologyError::InvalidGicRange {
            region,
            address,
            size,
        });
    };
    AddressRange::new(PhysAddr::new(start), PhysAddr::new(end)).map_err(|_| {
        InterruptTopologyError::InvalidGicRange {
            region,
            address,
            size,
        }
    })
}

fn parse_timer(
    properties: NodeProperties<'_>,
    inherited_parent: u32,
    controller_phandle: u32,
) -> Result<ArchitectedTimerInterrupts, InterruptTopologyError> {
    validate_node_status(NodeKind::Timer, properties.status)?;
    require_compatible(NodeKind::Timer, properties.compatible, b"arm,armv8-timer")?;
    let parent = properties
        .interrupt_parent
        .map(|value| parse_scalar("timer", "interrupt-parent", value))
        .transpose()?
        .unwrap_or(inherited_parent);
    if parent != controller_phandle {
        return Err(InterruptTopologyError::WrongInterruptParent {
            node: "timer",
            expected: controller_phandle,
            observed: parent,
        });
    }
    parse_timer_interrupts(required_property(
        NodeKind::Timer,
        "interrupts",
        properties.interrupts,
    )?)
}

fn parse_timer_interrupts(
    value: &[u8],
) -> Result<ArchitectedTimerInterrupts, InterruptTopologyError> {
    const EXPECTED_BYTES: usize = ARCH_TIMER_INTERRUPT_COUNT * INTERRUPT_SPECIFIER_BYTES;
    if value.len() != EXPECTED_BYTES {
        return Err(InterruptTopologyError::InvalidTimerInterruptList {
            length: value.len(),
            expected: EXPECTED_BYTES,
        });
    }
    let mut ppis: [Option<GicPpi>; ARCH_TIMER_INTERRUPT_COUNT] = [None; ARCH_TIMER_INTERRUPT_COUNT];
    for (index, entry) in value.chunks_exact(INTERRUPT_SPECIFIER_BYTES).enumerate() {
        let ppi = parse_ppi(index, entry)?;
        if ppis[..index]
            .iter()
            .flatten()
            .any(|existing| existing.intid() == ppi.intid())
        {
            return Err(InterruptTopologyError::DuplicateTimerInterrupt { intid: ppi.intid() });
        }
        ppis[index] = Some(ppi);
    }
    let [
        Some(secure_physical),
        Some(non_secure_physical),
        Some(virtual_timer),
        Some(hypervisor),
    ] = ppis
    else {
        unreachable!("exact timer interrupt list length fills every slot")
    };
    Ok(ArchitectedTimerInterrupts {
        secure_physical,
        non_secure_physical,
        virtual_timer,
        hypervisor,
    })
}

fn parse_ppi(index: usize, entry: &[u8]) -> Result<GicPpi, InterruptTopologyError> {
    let interrupt_type = read_cell(&entry[..4]);
    let number = read_cell(&entry[4..8]);
    let flags = read_cell(&entry[8..12]);
    let trigger = match flags & GIC_TRIGGER_MASK {
        IRQ_TYPE_EDGE_RISING => InterruptTrigger::EdgeRising,
        IRQ_TYPE_LEVEL_HIGH => InterruptTrigger::LevelHigh,
        _ => {
            return Err(InterruptTopologyError::InvalidTimerInterrupt {
                index,
                interrupt_type,
                number,
                flags,
            });
        }
    };
    if interrupt_type != GIC_PPI_TYPE
        || number > GIC_PPI_MAX_NUMBER
        || flags & !GIC_TRIGGER_MASK != 0
    {
        return Err(InterruptTopologyError::InvalidTimerInterrupt {
            index,
            interrupt_type,
            number,
            flags,
        });
    }
    GicPpi::new(PPI_INTID_MIN + number as u16, trigger).map_err(|_| {
        InterruptTopologyError::InvalidTimerInterrupt {
            index,
            interrupt_type,
            number,
            flags,
        }
    })
}

fn validate_node_status(
    kind: NodeKind,
    status: Option<&[u8]>,
) -> Result<(), InterruptTopologyError> {
    let Some(status) = status else {
        return Ok(());
    };
    let status = parse_single_string(kind.name(), "status", status)?;
    if matches!(status, "okay" | "ok") {
        Ok(())
    } else {
        Err(InterruptTopologyError::UnavailableNode { node: kind.name() })
    }
}

fn require_compatible(
    kind: NodeKind,
    compatible: Option<&[u8]>,
    expected: &[u8],
) -> Result<(), InterruptTopologyError> {
    let value = required_property(kind, "compatible", compatible)?;
    let mut found = false;
    parse_string_list(kind.name(), "compatible", value, |entry| {
        found |= entry == expected;
    })?;
    if found {
        Ok(())
    } else {
        Err(InterruptTopologyError::IncompatibleNode { node: kind.name() })
    }
}

fn required_property<'a>(
    kind: NodeKind,
    name: &'static str,
    value: Option<&'a [u8]>,
) -> Result<&'a [u8], InterruptTopologyError> {
    value.ok_or(InterruptTopologyError::MissingNodeProperty {
        node: kind.name(),
        name,
    })
}

fn parse_scalar(
    node: &'static str,
    name: &'static str,
    value: &[u8],
) -> Result<u32, InterruptTopologyError> {
    let bytes: [u8; 4] = value
        .try_into()
        .map_err(|_| InterruptTopologyError::MalformedProperty { node, name })?;
    Ok(u32::from_be_bytes(bytes))
}

fn parse_single_string<'a>(
    node: &'static str,
    name: &'static str,
    value: &'a [u8],
) -> Result<&'a str, InterruptTopologyError> {
    let Some((&0, text)) = value.split_last() else {
        return Err(InterruptTopologyError::MalformedProperty { node, name });
    };
    if text.is_empty() || text.contains(&0) {
        return Err(InterruptTopologyError::MalformedProperty { node, name });
    }
    str::from_utf8(text).map_err(|_| InterruptTopologyError::MalformedProperty { node, name })
}

fn parse_string_list(
    node: &'static str,
    name: &'static str,
    value: &[u8],
    mut consume: impl FnMut(&[u8]),
) -> Result<(), InterruptTopologyError> {
    if value.is_empty() || value.last() != Some(&0) {
        return Err(InterruptTopologyError::MalformedProperty { node, name });
    }
    let entries = &value[..value.len() - 1];
    if entries.is_empty() {
        return Err(InterruptTopologyError::MalformedProperty { node, name });
    }
    for entry in entries.split(|byte| *byte == 0) {
        if entry.is_empty() || str::from_utf8(entry).is_err() {
            return Err(InterruptTopologyError::MalformedProperty { node, name });
        }
        consume(entry);
    }
    Ok(())
}

fn read_cell(value: &[u8]) -> u32 {
    u32::from_be_bytes(value.try_into().expect("validated cell width"))
}

fn read_two_cells(value: &[u8]) -> u64 {
    (u64::from(read_cell(&value[..4])) << 32) | u64::from(read_cell(&value[4..]))
}

fn base_name(name: &str) -> &str {
    name.split_once('@').map_or(name, |(base, _)| base)
}

fn property_name(name: &str) -> &'static str {
    match name {
        "compatible" => "compatible",
        "status" => "status",
        "reg" => "reg",
        "interrupt-controller" => "interrupt-controller",
        "#interrupt-cells" => "#interrupt-cells",
        "phandle" => "phandle",
        "linux,phandle" => "linux,phandle",
        "#redistributor-regions" => "#redistributor-regions",
        "interrupts" => "interrupts",
        "interrupt-parent" => "interrupt-parent",
        _ => unreachable!("only captured properties are reported"),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::{InterruptTopologyError, InterruptTrigger, discover};
    use crate::dtb::DeviceTree;
    use crate::memory::{AddressRange, PhysAddr};

    const FDT_MAGIC: u32 = 0xd00d_feed;
    const FDT_BEGIN_NODE: u32 = 1;
    const FDT_END_NODE: u32 = 2;
    const FDT_PROP: u32 = 3;
    const FDT_END: u32 = 9;

    #[derive(Clone, Copy)]
    struct FixtureOptions<'a> {
        gic_reg: &'a [u8],
        timer_interrupts: &'a [u8],
        root_parent: u32,
        timer_parent: Option<u32>,
        gic_status: Option<&'a str>,
        timer_status: Option<&'a str>,
        duplicate_gic: bool,
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn pad_4(bytes: &mut Vec<u8>) {
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
    }

    fn begin_node(structure: &mut Vec<u8>, name: &str) {
        push_u32(structure, FDT_BEGIN_NODE);
        structure.extend_from_slice(name.as_bytes());
        structure.push(0);
        pad_4(structure);
    }

    fn property(structure: &mut Vec<u8>, offset: u32, value: &[u8]) {
        push_u32(structure, FDT_PROP);
        push_u32(structure, value.len() as u32);
        push_u32(structure, offset);
        structure.extend_from_slice(value);
        pad_4(structure);
    }

    fn string_offset(strings: &mut Vec<u8>, name: &str) -> u32 {
        let offset = strings.len() as u32;
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
        offset
    }

    fn two_cell_reg(entries: &[(u64, u64)]) -> Vec<u8> {
        let mut value = Vec::new();
        for (address, size) in entries {
            push_u32(&mut value, (address >> 32) as u32);
            push_u32(&mut value, *address as u32);
            push_u32(&mut value, (size >> 32) as u32);
            push_u32(&mut value, *size as u32);
        }
        value
    }

    fn timer_interrupts(specifiers: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut value = Vec::new();
        for (kind, number, flags) in specifiers {
            push_u32(&mut value, *kind);
            push_u32(&mut value, *number);
            push_u32(&mut value, *flags);
        }
        value
    }

    fn fixture(options: FixtureOptions<'_>) -> Vec<u8> {
        let mut strings = Vec::new();
        let address_cells = string_offset(&mut strings, "#address-cells");
        let size_cells = string_offset(&mut strings, "#size-cells");
        let interrupt_parent = string_offset(&mut strings, "interrupt-parent");
        let compatible = string_offset(&mut strings, "compatible");
        let reg = string_offset(&mut strings, "reg");
        let interrupt_controller = string_offset(&mut strings, "interrupt-controller");
        let interrupt_cells = string_offset(&mut strings, "#interrupt-cells");
        let phandle = string_offset(&mut strings, "phandle");
        let redistributor_regions = string_offset(&mut strings, "#redistributor-regions");
        let interrupts = string_offset(&mut strings, "interrupts");
        let status = string_offset(&mut strings, "status");

        let mut structure = Vec::new();
        begin_node(&mut structure, "");
        property(&mut structure, address_cells, &2_u32.to_be_bytes());
        property(&mut structure, size_cells, &2_u32.to_be_bytes());
        property(
            &mut structure,
            interrupt_parent,
            &options.root_parent.to_be_bytes(),
        );
        append_gic_node(
            &mut structure,
            GicPropertyOffsets {
                compatible,
                reg,
                interrupt_controller,
                interrupt_cells,
                phandle,
                redistributor_regions,
                status,
            },
            options.gic_reg,
            options.gic_status,
        );
        if options.duplicate_gic {
            append_gic_node(
                &mut structure,
                GicPropertyOffsets {
                    compatible,
                    reg,
                    interrupt_controller,
                    interrupt_cells,
                    phandle,
                    redistributor_regions,
                    status,
                },
                options.gic_reg,
                options.gic_status,
            );
        }
        begin_node(&mut structure, "timer");
        property(
            &mut structure,
            compatible,
            b"arm,armv8-timer\0arm,armv7-timer\0",
        );
        property(&mut structure, interrupts, options.timer_interrupts);
        if let Some(parent) = options.timer_parent {
            property(&mut structure, interrupt_parent, &parent.to_be_bytes());
        }
        if let Some(node_status) = options.timer_status {
            let mut encoded = node_status.as_bytes().to_vec();
            encoded.push(0);
            property(&mut structure, status, &encoded);
        }
        push_u32(&mut structure, FDT_END_NODE);
        push_u32(&mut structure, FDT_END_NODE);
        push_u32(&mut structure, FDT_END);

        let mut reservations = Vec::new();
        push_u64(&mut reservations, 0);
        push_u64(&mut reservations, 0);
        let reservations_offset = 40_usize;
        let structure_offset = reservations_offset + reservations.len();
        let strings_offset = structure_offset + structure.len();
        let total_size = strings_offset + strings.len();
        let mut blob = Vec::new();
        for field in [
            FDT_MAGIC,
            total_size as u32,
            structure_offset as u32,
            strings_offset as u32,
            reservations_offset as u32,
            17,
            16,
            0,
            strings.len() as u32,
            structure.len() as u32,
        ] {
            push_u32(&mut blob, field);
        }
        blob.extend_from_slice(&reservations);
        blob.extend_from_slice(&structure);
        blob.extend_from_slice(&strings);
        blob
    }

    #[derive(Clone, Copy)]
    struct GicPropertyOffsets {
        compatible: u32,
        reg: u32,
        interrupt_controller: u32,
        interrupt_cells: u32,
        phandle: u32,
        redistributor_regions: u32,
        status: u32,
    }

    fn append_gic_node(
        structure: &mut Vec<u8>,
        offsets: GicPropertyOffsets,
        registers: &[u8],
        node_status: Option<&str>,
    ) {
        begin_node(structure, "intc@8000000");
        property(structure, offsets.compatible, b"arm,gic-v3\0");
        property(structure, offsets.reg, registers);
        property(structure, offsets.interrupt_controller, &[]);
        property(structure, offsets.interrupt_cells, &3_u32.to_be_bytes());
        property(structure, offsets.phandle, &0x8001_u32.to_be_bytes());
        property(
            structure,
            offsets.redistributor_regions,
            &1_u32.to_be_bytes(),
        );
        if let Some(node_status) = node_status {
            let mut encoded = node_status.as_bytes().to_vec();
            encoded.push(0);
            property(structure, offsets.status, &encoded);
        }
        push_u32(structure, FDT_END_NODE);
    }

    fn valid_registers() -> Vec<u8> {
        two_cell_reg(&[(0x0800_0000, 0x1_0000), (0x080a_0000, 0xf6_0000)])
    }

    fn valid_interrupts() -> Vec<u8> {
        timer_interrupts(&[(1, 13, 4), (1, 14, 4), (1, 11, 4), (1, 10, 4)])
    }

    fn options<'a>(registers: &'a [u8], interrupts: &'a [u8]) -> FixtureOptions<'a> {
        FixtureOptions {
            gic_reg: registers,
            timer_interrupts: interrupts,
            root_parent: 0x8001,
            timer_parent: None,
            gic_status: None,
            timer_status: Some("okay"),
            duplicate_gic: false,
        }
    }

    #[test]
    fn discovers_gicv3_regions_and_all_architected_timer_ppis() {
        let registers = valid_registers();
        let interrupts = valid_interrupts();
        let blob = fixture(options(&registers, &interrupts));
        let topology = discover(DeviceTree::from_bytes(&blob).unwrap()).unwrap();

        assert_eq!(topology.controller_phandle(), 0x8001);
        assert_eq!(
            topology.distributor(),
            AddressRange::new(PhysAddr::new(0x0800_0000), PhysAddr::new(0x0801_0000)).unwrap()
        );
        assert_eq!(
            topology.redistributor(),
            AddressRange::new(PhysAddr::new(0x080a_0000), PhysAddr::new(0x0900_0000)).unwrap()
        );
        let timers = topology.timers();
        assert_eq!(timers.secure_physical().intid(), 29);
        assert_eq!(timers.non_secure_physical().intid(), 30);
        assert_eq!(timers.virtual_timer().intid(), 27);
        assert_eq!(timers.hypervisor().intid(), 26);
        assert_eq!(
            timers.non_secure_physical().trigger(),
            InterruptTrigger::LevelHigh
        );
    }

    #[test]
    fn rejects_duplicate_controller_and_wrong_parent_links() {
        let registers = valid_registers();
        let interrupts = valid_interrupts();
        let mut duplicate = options(&registers, &interrupts);
        duplicate.duplicate_gic = true;
        let blob = fixture(duplicate);
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::DuplicateNode { node: "gic" })
        );

        let mut wrong_root = options(&registers, &interrupts);
        wrong_root.root_parent = 9;
        let blob = fixture(wrong_root);
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::WrongInterruptParent {
                node: "root",
                expected: 0x8001,
                observed: 9,
            })
        );

        let mut wrong_timer = options(&registers, &interrupts);
        wrong_timer.timer_parent = Some(10);
        let blob = fixture(wrong_timer);
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::WrongInterruptParent {
                node: "timer",
                expected: 0x8001,
                observed: 10,
            })
        );
    }

    #[test]
    fn rejects_disabled_nodes_and_malformed_gic_ranges() {
        let registers = valid_registers();
        let interrupts = valid_interrupts();
        let mut disabled = options(&registers, &interrupts);
        disabled.gic_status = Some("disabled");
        let blob = fixture(disabled);
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::UnavailableNode { node: "gic" })
        );

        let short = two_cell_reg(&[(0x0800_0000, 0x1_0000)]);
        let blob = fixture(options(&short, &interrupts));
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::InvalidGicRegisterList {
                length: 16,
                expected: 32,
            })
        );

        let misaligned = two_cell_reg(&[(0x0800_1000, 0x1_0000), (0x080a_0000, 0xf6_0000)]);
        let blob = fixture(options(&misaligned, &interrupts));
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::InvalidGicRange {
                region: "distributor",
                address: 0x0800_1000,
                size: 0x1_0000,
            })
        );
    }

    #[test]
    fn rejects_malformed_or_aliasing_timer_ppis() {
        let registers = valid_registers();
        let short = timer_interrupts(&[(1, 13, 4)]);
        let blob = fixture(options(&registers, &short));
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::InvalidTimerInterruptList {
                length: 12,
                expected: 48,
            })
        );

        let invalid = timer_interrupts(&[(1, 13, 4), (0, 14, 4), (1, 11, 4), (1, 10, 4)]);
        let blob = fixture(options(&registers, &invalid));
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::InvalidTimerInterrupt {
                index: 1,
                interrupt_type: 0,
                number: 14,
                flags: 4,
            })
        );

        let duplicate = timer_interrupts(&[(1, 13, 4), (1, 13, 4), (1, 11, 4), (1, 10, 4)]);
        let blob = fixture(options(&registers, &duplicate));
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::DuplicateTimerInterrupt { intid: 29 })
        );

        let reserved_flags =
            timer_interrupts(&[(1, 13, 4), (1, 14, 0x104), (1, 11, 4), (1, 10, 4)]);
        let blob = fixture(options(&registers, &reserved_flags));
        assert_eq!(
            discover(DeviceTree::from_bytes(&blob).unwrap()),
            Err(InterruptTopologyError::InvalidTimerInterrupt {
                index: 1,
                interrupt_type: 1,
                number: 14,
                flags: 0x104,
            })
        );
    }
}
