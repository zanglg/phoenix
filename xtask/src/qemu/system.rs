//! Independent system oracle: exact faults from the linked ELF, ordered lifetimes,
//! and negative controls that must be rejected despite a successful guest exit.
use super::*;
use crate::{capture_command, llvm_tool, parse_nm_symbols};
use std::collections::BTreeMap;

struct Fault {
    kind: &'static str,
    pc: u64,
    address: Option<u64>,
    esr: u64,
}
struct Contract {
    faults: [Fault; 9],
}
impl Contract {
    fn load(directory: &Path) -> Option<Self> {
        let output = capture_command(
            Command::new(llvm_tool("llvm-nm")?)
                .arg("--defined-only")
                .arg(directory.join("phoenix-fault.elf")),
        )?;
        Self::from_symbols(&parse_nm_symbols(&output))
    }
    fn from_symbols(symbols: &BTreeMap<String, u64>) -> Option<Self> {
        let pc = |name: &str| symbols.get(name).copied();
        Some(Self {
            faults: [
                Fault {
                    kind: "Permission",
                    pc: pc("fault_kernel_read")?,
                    address: Some(0xffff_ff80_4008_0000),
                    esr: 0x9200_000f,
                },
                Fault {
                    kind: "Translation",
                    pc: pc("fault_translation")?,
                    address: Some(0),
                    esr: 0x9200_0006,
                },
                Fault {
                    kind: "Permission",
                    pc: 0x7fff_ffe000,
                    address: Some(0x7fff_ffe000),
                    esr: 0x8200_000f,
                },
                Fault {
                    kind: "IllegalInstruction",
                    pc: pc("fault_illegal")?,
                    address: None,
                    esr: 0x0200_0000,
                },
                // SVC saves the following instruction; abort/undefined saves its own PC.
                Fault {
                    kind: "BadSystemCall",
                    pc: pc("fault_svc")?.checked_add(4)?,
                    address: None,
                    esr: 0x5600_0001,
                },
                Fault {
                    kind: "Permission",
                    pc: pc("fault_code_write")?,
                    address: Some(pc("_start")?),
                    esr: 0x9200_004f,
                },
                Fault {
                    kind: "Translation",
                    pc: pc("fault_low_mmio")?,
                    address: Some(0x0900_0000),
                    esr: 0x9200_0006,
                },
                Fault {
                    kind: "Translation",
                    pc: pc("fault_low_ram")?,
                    address: Some(0x4000_0000),
                    esr: 0x9200_0005,
                },
                Fault {
                    kind: "Translation",
                    pc: pc("fault_stack_guard")?,
                    address: Some(0x7fff_ffa000),
                    esr: 0x9200_0007,
                },
            ],
        })
    }

    fn valid(&self, output: &[u8], rounds: usize, stress: bool) -> bool {
        let text = String::from_utf8_lossy(output).replace('\r', "");
        if text.contains(PANIC_SENTINEL) || text.contains(EXCEPTION_SENTINEL) {
            return false;
        }
        let mut expected = Vec::new();
        if stress {
            expected.push("PHOENIX_RECOVERY frames=10 backend=21 asids=254 retry=ok".to_owned());
        }
        expected.push("PHOENIX_SYSTEM_ENTER pid=0.1 asid=1".to_owned());
        let reap = |generation, fault| {
            format!(
                "child: reaped=1.{generation} fault={fault} frames=restored asid=2 slots=restored"
            )
        };
        let mut generation = 0;
        for _ in 0..rounds {
            for _ in 0..2 {
                generation += 1;
                expected.push(reap(generation, false));
            }
            for fault in &self.faults {
                generation += 1;
                expected.push(format!(
                    "PHOENIX_USER_FAULT pid=1.{generation} kind={} pc={:#x} address={:?} esr={:#x}",
                    fault.kind, fault.pc, fault.address, fault.esr
                ));
                expected.push(reap(generation, true));
            }
        }
        generation += 1;
        expected.push(reap(generation, false));
        expected.push(format!(
            "system: children={generation} faults={} console-read=3 frame-accounting=ok",
            rounds * 9
        ));
        expected.push("PHOENIX_SYSTEM_EXIT status=0 fault=false".to_owned());
        let actual: Vec<_> = text
            .lines()
            .filter(|line| {
                line.starts_with("PHOENIX_RECOVERY")
                    || line.starts_with("PHOENIX_SYSTEM_")
                    || line.starts_with(USER_FAULT_SENTINEL)
                    || line.starts_with("child: reaped=")
                    || line.starts_with("system: children=")
            })
            .collect();
        actual == expected
            && contains_ordered(
                text.as_bytes(),
                &[
                    "Phoenix system: init (type ok then Enter)",
                    "Phoenix initramfs: file I/O works",
                    "Phoenix system: file, console, child and fault checks passed\n",
                    "PHOENIX_SYSTEM_EXIT status=0 fault=false",
                ],
            )
    }

    fn valid_cooperative(&self, output: &[u8]) -> bool {
        let text = String::from_utf8_lossy(output).replace('\r', "");
        if text.contains(PANIC_SENTINEL) || text.contains(EXCEPTION_SENTINEL) {
            return false;
        }
        let mut expected = vec!["PHOENIX_SYSTEM_ENTER pid=0.1 asid=1".to_owned()];
        let fault = &self.faults[3];
        for generation in 1..=257 {
            let faulted = generation <= 256 && (generation - 1) % 4 >= 2;
            if faulted {
                expected.push(format!(
                    "PHOENIX_USER_FAULT pid=1.{generation} kind={} pc={:#x} address={:?} esr={:#x}",
                    fault.kind, fault.pc, fault.address, fault.esr
                ));
            }
            expected.push(format!(
                "child: reaped=1.{generation} fault={faulted} frames=restored asid=2 slots=restored"
            ));
        }
        expected
            .push("Phoenix cooperative: rounds=64 registers=ok stack=ok files=ok waits=ok".into());
        expected.push("system: children=257 faults=128 console-read=0 frame-accounting=ok".into());
        expected.push("PHOENIX_SYSTEM_EXIT status=0 fault=false".into());
        let actual: Vec<_> = text
            .lines()
            .filter(|line| {
                line.starts_with("PHOENIX_SYSTEM_")
                    || line.starts_with(USER_FAULT_SENTINEL)
                    || line.starts_with("child: reaped=")
                    || line.starts_with("Phoenix cooperative:")
                    || line.starts_with("system: children=")
            })
            .collect();
        actual == expected
    }
}

pub(super) fn cooperative_suite(image: &Path, directory: &Path, root: &Path) -> bool {
    if !preflight() {
        return false;
    }
    let Some(contract) = Contract::load(directory) else {
        return false;
    };
    let mut command = command(image, false);
    *command.arguments.last_mut().unwrap() = "phoenix.system-cooperative".into();
    let cooperative = runtime::report(
        runtime::run_guest(
            command,
            directory,
            root,
            "system-cooperative",
            None,
            Duration::from_secs(30),
        )
        .is_some_and(|guest| guest.success() && contract.valid_cooperative(&guest.output)),
    );
    if !cooperative {
        return false;
    }
    if !runtime::report(
        runtime::run_guest(
            command_for_boot(image, "phoenix.system-cooperative-preempt"),
            directory,
            root,
            "system-cooperative-preempt",
            None,
            Duration::from_secs(30),
        )
        .is_some_and(|guest| guest.success() && contract.valid_cooperative(&guest.output)),
    ) {
        return false;
    }
    for (name, enabled) in [("system-preempt", true), ("system-preempt-off", false)] {
        let command = command_for_boot(
            image,
            if enabled {
                "phoenix.system-preempt"
            } else {
                "phoenix.system-preempt-off"
            },
        );
        // Keep the same board and exact image for the negative control.
        let valid = runtime::run_guest(command, directory, root, name, None, Duration::from_secs(30))
            .is_some_and(|guest| {
                guest.success() && if enabled {
                    valid_preemption(&guest.output)
                } else {
                    let text = String::from_utf8_lossy(&guest.output).replace('\r', "");
                    !valid_preemption(&guest.output)
                        && text.lines().any(|line| line == "PHOENIX_SYSTEM_EXIT status=0 fault=false")
                        && text.lines().any(|line| line == "PHOENIX_SCHED root-ticks=0 child-ticks=0 switches=0 child-ready=true")
                }
            });
        if !enabled {
            println!("QEMU_PREEMPTION_NEGATIVE_REJECTED={valid}");
        }
        if !runtime::report(valid) {
            return false;
        }
    }
    true
}

fn valid_preemption(output: &[u8]) -> bool {
    let text = String::from_utf8_lossy(output).replace('\r', "");
    if text.contains(PANIC_SENTINEL) || text.contains(EXCEPTION_SENTINEL) {
        return false;
    }
    let protocol: Vec<_> = text
        .lines()
        .filter(|line| {
            line.starts_with("PHOENIX_SYSTEM_")
                || line.starts_with("PHOENIX_SCHED")
                || line.starts_with(USER_FAULT_SENTINEL)
                || line.starts_with("child: reaped=")
                || line.starts_with("Phoenix preemption:")
                || line.starts_with("system: children=")
        })
        .collect();
    if protocol.len() != 5
        || protocol[0] != "PHOENIX_SYSTEM_ENTER pid=0.1 asid=1"
        || protocol[1] != "Phoenix preemption: non-yielding init progressed registers=ok stack=ok"
        || protocol[3] != "system: children=0 faults=0 console-read=0 frame-accounting=ok"
        || protocol[4] != "PHOENIX_SYSTEM_EXIT status=0 fault=false"
    {
        return false;
    }
    let stats: Vec<_> = protocol[2].split_whitespace().collect();
    if stats.len() != 5 || stats[0] != "PHOENIX_SCHED" || stats[4] != "child-ready=true" {
        return false;
    }
    let number = |token: &str, prefix: &str| token.strip_prefix(prefix)?.parse::<u64>().ok();
    let Some(root) = number(stats[1], "root-ticks=") else {
        return false;
    };
    let Some(child) = number(stats[2], "child-ticks=") else {
        return false;
    };
    let Some(switches) = number(stats[3], "switches=") else {
        return false;
    };
    root >= 2 && child >= 2 && switches >= 4 && switches <= root.saturating_add(child)
}

fn command_for_boot(image: &Path, boot: &str) -> QemuCommand {
    let mut command = command(image, false);
    *command.arguments.last_mut().unwrap() = boot.into();
    command
}

fn command(image: &Path, stress: bool) -> QemuCommand {
    let mut command = QemuCommand::for_kernel(image);
    command.arguments.extend([
        "-semihosting-config".into(),
        "enable=on,target=native".into(),
        "-append".into(),
        if stress {
            "phoenix.system-stress"
        } else {
            "phoenix.system-test"
        }
        .into(),
    ]);
    command
}

pub(super) fn suite(image: &Path, directory: &Path, root: &Path, stress: bool) -> bool {
    if !preflight() {
        return false;
    }
    let Some(contract) = Contract::load(directory) else {
        eprintln!("system oracle requires fault instruction symbols in the exact user ELF");
        return false;
    };
    if stress {
        return runtime::report(
            runtime::run_guest(
                command(image, true),
                directory,
                root,
                "system-stress",
                Some(b"ok\n"),
                Duration::from_secs(30),
            )
            .is_some_and(|guest| guest.success() && contract.valid(&guest.output, 64, true)),
        );
    }
    for (name, input) in [
        ("system-lf", &b"ok\n"[..]),
        ("system-cr", &b"ok\r"[..]),
        ("system-crlf", &b"ok\r\n"[..]),
    ] {
        if !runtime::report(
            runtime::run_guest(
                command(image, false),
                directory,
                root,
                name,
                Some(input),
                TEST_TIMEOUT,
            )
            .is_some_and(|guest| guest.success() && contract.valid(&guest.output, 1, false)),
        ) {
            return false;
        }
    }
    let invalid = runtime::run_guest(
        command(image, false),
        directory,
        root,
        "system-bad-input",
        Some(b"no\n"),
        TEST_TIMEOUT,
    )
    .is_some_and(|guest| {
        let text = String::from_utf8_lossy(&guest.output).replace('\r', "");
        guest.status.code() == Some(1)
            && !text.contains(PANIC_SENTINEL)
            && !text.contains(USER_FAULT_SENTINEL)
            && !text.contains("child: reaped=")
            && text
                .lines()
                .filter(|line| line.starts_with("PHOENIX_SYSTEM_EXIT"))
                .eq(["PHOENIX_SYSTEM_EXIT status=1 fault=false"])
    });
    if !runtime::report(invalid) {
        return false;
    }
    let Some(mutant) = wrong_fault_image(image, directory) else {
        return false;
    };
    // The kernel and init still finish successfully. Only the independent host
    // oracle can reject the nine wrong faults; this is a real QEMU negative control.
    let rejected = runtime::run_guest(
        command(&mutant, false),
        directory,
        root,
        "system-wrong-fault",
        Some(b"ok\n"),
        TEST_TIMEOUT,
    )
    .is_some_and(|guest| {
        guest.success()
            && !contract.valid(&guest.output, 1, false)
            && String::from_utf8_lossy(&guest.output)
                .matches("kind=IllegalInstruction")
                .count()
                == 9
    });
    println!("QEMU_WRONG_FAULT_REJECTED={rejected}");
    runtime::report(rejected)
}

fn wrong_fault_image(image: &Path, directory: &Path) -> Option<std::path::PathBuf> {
    let elf = fs::read(directory.join("phoenix-fault.elf")).ok()?;
    let parsed = phoenix_kernel::elf::ElfImage::parse(&elf).ok()?;
    let segment = parsed.load_segments().next()?.ok()?;
    let start = segment.virtual_range().start().as_usize();
    let offset = (segment.data().as_ptr() as usize)
        .checked_sub(elf.as_ptr() as usize)?
        .checked_add(parsed.entry().as_usize().checked_sub(start)?)?;
    elf.get(offset..offset + 4)?;
    let mut kernel = fs::read(image).ok()?;
    let copies: Vec<_> = kernel
        .windows(elf.len())
        .enumerate()
        .filter_map(|(index, bytes)| (bytes == elf).then_some(index))
        .collect();
    if copies.len() != 10 {
        eprintln!("negative control expected ten embedded copies of the fault ELF");
        return None;
    }
    for base in copies {
        kernel[base + offset..base + offset + 4].fill(0);
    }
    let path = directory.join("phoenix-system-wrong-fault.bin");
    fs::write(&path, kernel).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Independently retained from a real QEMU run and its linked ELF, rather
    // than constructing the fixture with the expected-record generator.
    const LOG: &str = include_str!("fixtures/system.txt");
    const SYMBOLS: &str = include_str!("fixtures/fault.nm");
    fn contract() -> Contract {
        Contract::from_symbols(&parse_nm_symbols(SYMBOLS)).unwrap()
    }

    #[test]
    fn accepts_observed_faults_only_for_the_exact_lifetime_protocol() {
        let contract = contract();
        assert!(contract.valid(LOG.as_bytes(), 1, false));
        assert!(contract.valid(LOG.replace('\n', "\r\n").as_bytes(), 1, false));
        assert!(!contract.valid(LOG.as_bytes(), 64, true));
        for (from, to) in [
            ("kind=Permission", "kind=Translation"),
            ("pc=0x40004c", "pc=0x400048"),
            ("address=Some(0)", "address=Some(4096)"),
            ("esr=0x9200004f", "esr=0x9200000f"), // write changed to read
            ("esr=0x92000007", "esr=0x92000006"), // wrong translation level
            ("pc=0x7fffffe000", "pc=0x400060"),   // NX target was never reached
            ("pid=1.3", "pid=1.4"),
            ("reaped=1.12", "reaped=1.11"),
            ("asid=2", "asid=3"),
            ("frames=restored", "frames=leaked"),
            ("children=12", "children=11"),
            ("console-read=3", "console-read=0"),
            ("status=0 fault=false", "status=1 fault=false"),
        ] {
            assert!(LOG.contains(from), "fixture lacks {from}");
            assert!(
                !contract.valid(LOG.replacen(from, to, 1).as_bytes(), 1, false),
                "accepted {from} -> {to}"
            );
        }
    }

    #[test]
    fn rejects_missing_duplicate_reordered_or_wrong_fault_records() {
        let contract = contract();
        let lines: Vec<_> = LOG.lines().collect();
        let index = lines
            .iter()
            .position(|line| line.starts_with(USER_FAULT_SENTINEL))
            .unwrap();
        let mut missing = lines.clone();
        missing.remove(index);
        assert!(!contract.valid(missing.join("\n").as_bytes(), 1, false));
        let mut duplicate = lines.clone();
        duplicate.insert(index, lines[index]);
        assert!(!contract.valid(duplicate.join("\n").as_bytes(), 1, false));
        let mut reordered = lines;
        reordered.swap(index, index + 1);
        assert!(!contract.valid(reordered.join("\n").as_bytes(), 1, false));
        let wrong: Vec<_> = LOG
            .lines()
            .map(|line| {
                if line.starts_with(USER_FAULT_SENTINEL) {
                    format!(
                        "{}kind=IllegalInstruction pc=0x400000 address=None esr=0x2000000",
                        line.split_once("kind=").unwrap().0
                    )
                } else {
                    line.to_owned()
                }
            })
            .collect();
        assert!(!contract.valid(wrong.join("\n").as_bytes(), 1, false));
        for suffix in [
            "PHOENIX_PANIC",
            "PHOENIX_EXCEPTION",
            "PHOENIX_SYSTEM_EXIT status=0 fault=false",
        ] {
            assert!(!contract.valid(format!("{LOG}{suffix}\n").as_bytes(), 1, false));
        }
    }

    #[test]
    fn refuses_to_guess_when_the_exact_elf_lacks_fault_symbols() {
        let mut symbols = parse_nm_symbols(SYMBOLS);
        symbols.remove("fault_code_write");
        assert!(Contract::from_symbols(&symbols).is_none());
    }

    #[test]
    fn cooperative_oracle_rejects_corrupted_lifetimes_and_context_proof() {
        let observed = include_str!("fixtures/cooperative.txt");
        let contract = contract();
        assert!(contract.valid_cooperative(observed.as_bytes()));
        for (from, to) in [
            ("reaped=1.257", "reaped=1.256"),
            ("fault=true", "fault=false"),
            ("esr=0x2000000", "esr=0x9200000f"),
            ("pc=0x400068", "pc=0x400000"),
            ("registers=ok", "registers=bad"),
            ("faults=128", "faults=127"),
            ("asid=2", "asid=3"),
            ("status=0 fault=false", "status=1 fault=false"),
        ] {
            assert!(observed.contains(from));
            assert!(!contract.valid_cooperative(observed.replacen(from, to, 1).as_bytes()));
        }
        let without_first = observed.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(!contract.valid_cooperative(without_first.as_bytes()));
        assert!(!contract.valid_cooperative(format!("{observed}PHOENIX_PANIC\n").as_bytes()));
    }

    #[test]
    fn preemption_requires_both_contexts_and_a_live_infinite_child() {
        let observed = include_str!("fixtures/preempt.txt");
        assert!(valid_preemption(observed.as_bytes()));
        for (from, to) in [
            ("root-ticks=66", "root-ticks=0"),
            ("child-ticks=65", "child-ticks=0"),
            ("switches=130", "switches=0"),
            ("switches=130", "switches=999999"),
            ("child-ready=true", "child-ready=false"),
            ("registers=ok", "registers=bad"),
            ("status=0 fault=false", "status=1 fault=false"),
        ] {
            assert!(observed.contains(from));
            assert!(!valid_preemption(observed.replacen(from, to, 1).as_bytes()));
        }
        for suffix in [
            "PHOENIX_PANIC",
            "PHOENIX_USER_FAULT",
            "PHOENIX_SCHED",
            "PHOENIX_SYSTEM_EXIT",
        ] {
            assert!(!valid_preemption(
                format!("{observed}{suffix}\n").as_bytes()
            ));
        }
        assert!(!valid_preemption(include_bytes!(
            "fixtures/cooperative.txt"
        )));
    }
}
