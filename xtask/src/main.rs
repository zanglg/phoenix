use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod qemu;

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";
const KERNEL_PHYS_BASE: u64 = 0x0000_0000_4008_0000;
const KERNEL_VIRT_BASE: u64 = 0xffff_ff80_4008_0000;
const BOOT_STACK_SIZE: u64 = 64 * 1024;
const EXCEPTION_VECTOR_TABLE_SIZE: u64 = 2048;
const INIT_ENTRY: usize = 0x0040_0000;
const INIT_MESSAGE: &[u8] = b"Phoenix init: hello from EL0\n";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print_help();
        return ExitCode::FAILURE;
    };

    if args.next().is_some() {
        eprintln!("error: command '{command}' does not accept arguments");
        return ExitCode::FAILURE;
    }

    let ok = match command.as_str() {
        "doctor" => doctor(),
        "fmt" => format(),
        "check" => check(),
        "lint" => lint(),
        "test" => test(),
        "build" => build_kernel(),
        "inspect" => inspect_kernel(),
        "build-init" => build_init(),
        "inspect-init" => inspect_init(),
        "qemu-command" => print_qemu_command(),
        "run" => run_kernel(),
        "test-boot" => test_boot(),
        "test-memory" => test_memory(),
        "test-el0" => test_el0(),
        "test-init" => test_init(),
        "ci" => ci(),
        "help" | "--help" | "-h" => {
            print_help();
            true
        }
        _ => {
            eprintln!("error: unknown command '{command}'");
            print_help();
            false
        }
    };

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_help() {
    println!("Phoenix development tasks");
    println!();
    println!("Usage: cargo xtask <command>");
    println!();
    println!("Commands:");
    println!("  doctor   Check the local development environment");
    println!("  fmt      Check Rust formatting");
    println!("  check    Check kernel and host tooling");
    println!("  lint     Run Clippy for kernel and host tooling");
    println!("  test     Run host-side unit tests");
    println!("  build    Build the configured kernel ELF and raw image");
    println!("  inspect  Validate the built AArch64 ELF and raw image");
    println!("  build-init  Build the first userspace ELF and deterministic initramfs");
    println!("  inspect-init  Validate the userspace ELF and initramfs");
    println!("  qemu-command  Print the pinned QEMU command without running it");
    println!("  run      Build and run Phoenix interactively on QEMU");
    println!("  test-boot  Run the bounded QEMU boot integration test");
    println!("  test-memory  Run the bounded QEMU boot-memory integration probe");
    println!("  test-el0  Run the bounded QEMU EL0 conformance probe");
    println!("  test-init  Run the bounded dynamically loaded init probe");
    println!("  ci       Run every validation available without an emulator");
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live directly below the workspace root")
        .to_owned()
}

fn run(label: &str, program: &str, args: &[&str]) -> bool {
    let mut command = Command::new(program);
    command.args(args);
    run_command(label, &mut command)
}

fn run_command(label: &str, command: &mut Command) -> bool {
    println!("==> {label}");
    match command.current_dir(workspace_root()).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("error: {label} exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("error: could not run {label}: {error}");
            false
        }
    }
}

fn capture(program: &str, args: &[&str]) -> Option<String> {
    capture_command(Command::new(program).args(args))
}

fn capture_command(command: &mut Command) -> Option<String> {
    let output = command.current_dir(workspace_root()).output().ok()?;

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn probe(label: &str, program: &str, args: &[&str], required: bool) -> bool {
    match capture(program, args) {
        Some(version) => {
            println!("ok: {label}: {version}");
            true
        }
        None if required => {
            eprintln!("missing: {label} (required)");
            false
        }
        None => {
            println!("optional: {label} is not installed");
            true
        }
    }
}

fn installed_item(label: &str, args: &[&str], needle: &str) -> bool {
    match capture("rustup", args) {
        Some(output) if output.lines().any(|line| line.starts_with(needle)) => {
            println!("ok: {label}: {needle}");
            true
        }
        _ => {
            eprintln!("missing: {label}: {needle}");
            false
        }
    }
}

fn parse_lsp_target(contents: &str) -> Option<String> {
    let mut in_build_section = false;

    for raw_line in contents.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_build_section = line == "[build]";
            continue;
        }
        if !in_build_section {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "target" {
            continue;
        }

        let value = value.trim();
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            return Some(value[1..value.len() - 1].to_owned());
        }
    }

    None
}

fn configured_target() -> Option<String> {
    let contents = fs::read_to_string(workspace_root().join(".cargo/lsp.toml")).ok()?;
    parse_lsp_target(&contents)
}

fn rustc_host() -> Option<String> {
    capture("rustc", &["-vV"])?
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
}

fn llvm_tool(name: &str) -> Option<PathBuf> {
    let sysroot = capture("rustc", &["--print", "sysroot"])?;
    let host = rustc_host()?;
    let path = Path::new(&sysroot)
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(name);
    path.is_file().then_some(path)
}

fn doctor() -> bool {
    println!("Phoenix 0.0.0 environment");

    let target_check = match configured_target() {
        Some(target) => installed_item(
            "configured kernel target",
            &["target", "list", "--installed"],
            &target,
        ),
        None => {
            eprintln!("missing: [build].target in .cargo/lsp.toml");
            false
        }
    };

    let checks = [
        probe("rustc", "rustc", &["--version"], true),
        probe("cargo", "cargo", &["--version"], true),
        probe("rustfmt", "cargo", &["fmt", "--version"], true),
        probe("clippy", "cargo", &["clippy", "--version"], true),
        probe("rust-analyzer", "rust-analyzer", &["--version"], true),
        probe("git", "git", &["--version"], true),
        target_check,
        installed_item(
            "Rust component",
            &["component", "list", "--installed"],
            "rust-src",
        ),
        installed_item(
            "Rust component",
            &["component", "list", "--installed"],
            "llvm-tools",
        ),
        probe(
            "qemu-system-aarch64",
            "qemu-system-aarch64",
            &["--version"],
            false,
        ),
    ];

    checks.into_iter().all(|check| check)
}

fn format() -> bool {
    run(
        "format kernel workspace",
        "cargo",
        &["fmt", "--all", "--", "--check"],
    ) && run(
        "format xtask",
        "cargo",
        &[
            "fmt",
            "--manifest-path",
            "xtask/Cargo.toml",
            "--",
            "--check",
        ],
    )
}

fn check() -> bool {
    let Some(target) = configured_target() else {
        eprintln!("error: no kernel target is configured in .cargo/lsp.toml");
        return false;
    };
    let Some(initramfs) = prepare_initramfs(&target) else {
        return false;
    };
    let mut target_check = Command::new("cargo");
    target_check.args([
        "--config",
        ".cargo/lsp.toml",
        "check",
        "--workspace",
        "--all-features",
        "--target",
        &target,
    ]);
    target_check.env("PHOENIX_INITRAMFS", initramfs);
    run_command(
        "check kernel and init for configured LSP target",
        &mut target_check,
    ) && run(
        "check kernel library for host tests",
        "cargo",
        &["check", "--workspace", "--lib"],
    ) && run(
        "check xtask for host",
        "cargo",
        &["check", "--manifest-path", "xtask/Cargo.toml"],
    )
}

fn lint() -> bool {
    let Some(target) = configured_target() else {
        eprintln!("error: no kernel target is configured in .cargo/lsp.toml");
        return false;
    };
    let Some(initramfs) = prepare_initramfs(&target) else {
        return false;
    };
    let mut kernel_lint = Command::new("cargo");
    kernel_lint.args([
        "--config",
        ".cargo/lsp.toml",
        "clippy",
        "--package",
        "phoenix-kernel",
        "--lib",
        "--bin",
        "phoenix-kernel",
        "--all-features",
        "--target",
        &target,
        "--",
        "-D",
        "warnings",
    ]);
    kernel_lint.env("PHOENIX_INITRAMFS", initramfs);
    run_command("lint kernel for configured LSP target", &mut kernel_lint)
        && run(
            "lint init for configured LSP target",
            "cargo",
            &[
                "clippy",
                "--package",
                "phoenix-init",
                "--bin",
                "phoenix-init",
                "--target",
                &target,
                "--",
                "-D",
                "warnings",
            ],
        )
        && run(
            "lint kernel library for host tests",
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--lib",
                "--tests",
                "--",
                "-D",
                "warnings",
            ],
        )
        && run(
            "lint xtask for host",
            "cargo",
            &[
                "clippy",
                "--manifest-path",
                "xtask/Cargo.toml",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        )
}

fn test() -> bool {
    run(
        "test kernel library on host",
        "cargo",
        &["test", "--workspace", "--lib", "--all-features"],
    ) && run(
        "test host tooling",
        "cargo",
        &["test", "--manifest-path", "xtask/Cargo.toml"],
    )
}

struct Artifacts {
    cargo_target_dir: PathBuf,
    elf: PathBuf,
    image: PathBuf,
    map: PathBuf,
    qemu_log: PathBuf,
    qemu_memory_log: PathBuf,
    qemu_el0_log: PathBuf,
    qemu_init_log: PathBuf,
}

struct InitArtifacts {
    cargo_target_dir: PathBuf,
    elf: PathBuf,
    embedded_elf: PathBuf,
    initramfs: PathBuf,
    map: PathBuf,
}

#[derive(Clone, Copy)]
enum KernelVariant {
    Default,
    BootMemoryProbe,
    El0Probe,
    LoadedInitProbe,
}

impl KernelVariant {
    const fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::BootMemoryProbe => "boot-memory-probe",
            Self::El0Probe => "el0-probe",
            Self::LoadedInitProbe => "loaded-init-probe",
        }
    }

    const fn features(self) -> &'static [&'static str] {
        match self {
            Self::Default => &[],
            Self::BootMemoryProbe => &["boot-memory-probe"],
            Self::El0Probe => &["el0-probe"],
            Self::LoadedInitProbe => &["loaded-init-probe"],
        }
    }

    const fn expects_el0_probe(self) -> bool {
        matches!(self, Self::El0Probe)
    }

    const fn expects_loaded_init(self) -> bool {
        matches!(self, Self::LoadedInitProbe)
    }

    const fn expected_sentinel(self) -> &'static str {
        match self {
            Self::Default => qemu::BOOT_SUCCESS_SENTINEL,
            Self::BootMemoryProbe => qemu::MEMORY_SUCCESS_SENTINEL,
            Self::El0Probe => qemu::EL0_SUCCESS_SENTINEL,
            Self::LoadedInitProbe => qemu::INIT_SUCCESS_SENTINEL,
        }
    }
}

fn kernel_artifacts(target: &str, variant: KernelVariant) -> Artifacts {
    let root = workspace_root();
    let output = root.join("target/phoenix").join(target).join("debug");
    let cargo_target_dir = root.join("target/phoenix/build").join(variant.name());
    let artifact_stem = match variant {
        KernelVariant::Default => "phoenix-kernel",
        KernelVariant::BootMemoryProbe => "phoenix-kernel-memory",
        KernelVariant::El0Probe => "phoenix-kernel-el0",
        KernelVariant::LoadedInitProbe => "phoenix-kernel-init",
    };
    Artifacts {
        elf: cargo_target_dir.join(target).join("debug/phoenix-kernel"),
        image: output.join(format!("{artifact_stem}.bin")),
        map: output.join(format!("{artifact_stem}.map")),
        qemu_log: output.join("qemu-boot.log"),
        qemu_memory_log: output.join("qemu-memory.log"),
        qemu_el0_log: output.join("qemu-el0.log"),
        qemu_init_log: output.join("qemu-init.log"),
        cargo_target_dir,
    }
}

fn init_artifacts(target: &str) -> InitArtifacts {
    let root = workspace_root();
    let output = root.join("target/phoenix").join(target).join("debug");
    let cargo_target_dir = root.join("target/phoenix/build/init");
    InitArtifacts {
        elf: cargo_target_dir.join(target).join("debug/phoenix-init"),
        embedded_elf: output.join("phoenix-init.elf"),
        initramfs: output.join("phoenix-initramfs.cpio"),
        map: output.join("phoenix-init.map"),
        cargo_target_dir,
    }
}

fn append_newc_field(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(format!("{value:08x}").as_bytes());
}

fn append_newc_record(
    output: &mut Vec<u8>,
    inode: u32,
    path: &str,
    mode: u32,
    data: &[u8],
) -> Result<(), String> {
    let file_size = u32::try_from(data.len())
        .map_err(|_| "init ELF is too large for a newc file-size field".to_owned())?;
    let name_size = path
        .len()
        .checked_add(1)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| "initramfs path is too large for a newc name-size field".to_owned())?;
    if path.as_bytes().contains(&0) {
        return Err("initramfs path contains an interior NUL".to_owned());
    }

    output.extend_from_slice(b"070701");
    for value in [inode, mode, 0, 0, 1, 0, file_size, 0, 0, 0, 0, name_size, 0] {
        append_newc_field(output, value);
    }
    output.extend_from_slice(path.as_bytes());
    output.push(0);
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
    output.extend_from_slice(data);
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
    Ok(())
}

fn make_initramfs(init_elf: &[u8]) -> Result<Vec<u8>, String> {
    const REGULAR_EXECUTABLE: u32 = 0o100_755;

    let mut output = Vec::new();
    append_newc_record(&mut output, 1, "init", REGULAR_EXECUTABLE, init_elf)?;
    append_newc_record(&mut output, 0, "TRAILER!!!", 0, &[])?;
    Ok(output)
}

fn require_aarch64_target() -> Option<String> {
    let target = configured_target()?;
    if target != AARCH64_TARGET {
        eprintln!("error: kernel artifacts are not implemented for configured target '{target}'");
        eprintln!("hint: LSP target switching is supported before that architecture's boot port");
        return None;
    }
    Some(target)
}

fn build_kernel() -> bool {
    build_kernel_variant(KernelVariant::Default)
}

fn build_init() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = init_artifacts(&target);
    let Some(output_dir) = artifacts.embedded_elf.parent() else {
        eprintln!("error: invalid init artifact path");
        return false;
    };
    if let Err(error) = fs::create_dir_all(output_dir) {
        eprintln!("error: could not create {}: {error}", output_dir.display());
        return false;
    }

    let link_arg = format!("-Clink-arg=-Map={}", artifacts.map.display());
    let mut cargo = Command::new("cargo");
    cargo.args([
        "rustc",
        "--package",
        "phoenix-init",
        "--bin",
        "phoenix-init",
        "--target",
        &target,
    ]);
    cargo.arg("--target-dir").arg(&artifacts.cargo_target_dir);
    cargo.args(["--", &link_arg]);
    if !run_command("build standalone AArch64 init ELF", &mut cargo) {
        return false;
    }

    let Some(objcopy) = llvm_tool("llvm-objcopy") else {
        eprintln!("error: llvm-objcopy is unavailable; install the llvm-tools component");
        return false;
    };
    let mut command = Command::new(objcopy);
    command
        .arg("--strip-debug")
        .arg(&artifacts.elf)
        .arg(&artifacts.embedded_elf);
    if !run_command("create embeddable init ELF", &mut command) {
        return false;
    }

    let init_elf = match fs::read(&artifacts.embedded_elf) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "error: could not read embeddable init ELF {}: {error}",
                artifacts.embedded_elf.display()
            );
            return false;
        }
    };
    let initramfs = match make_initramfs(&init_elf) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: could not construct initramfs: {error}");
            return false;
        }
    };
    if let Err(error) = fs::write(&artifacts.initramfs, initramfs) {
        eprintln!(
            "error: could not write initramfs {}: {error}",
            artifacts.initramfs.display()
        );
        return false;
    }

    println!("artifact: init ELF: {}", artifacts.elf.display());
    println!(
        "artifact: embeddable init ELF: {}",
        artifacts.embedded_elf.display()
    );
    println!("artifact: initramfs: {}", artifacts.initramfs.display());
    println!("artifact: init linker map: {}", artifacts.map.display());
    true
}

fn prepare_initramfs(target: &str) -> Option<PathBuf> {
    if target != AARCH64_TARGET || !build_init() || !inspect_init() {
        return None;
    }
    Some(init_artifacts(target).initramfs)
}

fn inspect_init() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = init_artifacts(&target);
    for path in [
        &artifacts.elf,
        &artifacts.embedded_elf,
        &artifacts.initramfs,
        &artifacts.map,
    ] {
        if !path.is_file() {
            eprintln!(
                "error: missing init artifact {}; run cargo xtask build-init",
                path.display()
            );
            return false;
        }
    }

    let bytes = match fs::read(&artifacts.embedded_elf) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "error: could not read init ELF {}: {error}",
                artifacts.embedded_elf.display()
            );
            return false;
        }
    };
    let image = match phoenix_kernel::elf::ElfImage::parse(&bytes) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("error: Phoenix loader rejects init ELF: {error:?}");
            return false;
        }
    };
    let mut errors = Vec::new();
    match fs::read(&artifacts.initramfs) {
        Ok(archive_bytes) => {
            match phoenix_kernel::initramfs::Initramfs::from_bytes(&archive_bytes) {
                Ok(archive) => {
                    if archive.len() != 1 {
                        errors.push(format!(
                            "initramfs has {} entries, expected exactly one",
                            archive.len()
                        ));
                    }
                    match archive.find("init") {
                    Some(entry)
                        if entry.kind()
                            == phoenix_kernel::initramfs::InitramfsEntryKind::RegularFile
                            && entry.permissions() == 0o755
                            && entry.data() == bytes =>
                    {
                        println!("ok: deterministic initramfs contains the exact init ELF");
                    }
                    Some(_) => errors.push(
                        "initramfs init is not an executable regular file with the exact ELF bytes"
                            .to_owned(),
                    ),
                    None => errors.push("initramfs does not contain canonical path 'init'".to_owned()),
                }
                }
                Err(error) => errors.push(format!(
                    "Phoenix initramfs parser rejects generated archive: {error:?}"
                )),
            }
        }
        Err(error) => errors.push(format!(
            "could not read initramfs {}: {error}",
            artifacts.initramfs.display()
        )),
    }
    if image.entry().as_usize() != INIT_ENTRY {
        errors.push(format!(
            "init entry is {:#x}, expected {INIT_ENTRY:#x}",
            image.entry().as_usize()
        ));
    }
    if image.load_segment_count() != 1 {
        errors.push(format!(
            "init has {} load segments, expected exactly one",
            image.load_segment_count()
        ));
    }
    let segments: Vec<_> = image.load_segments().collect();
    match segments.as_slice() {
        [Ok(segment)] => {
            if segment.virtual_range().start().as_usize() != INIT_ENTRY
                || segment.virtual_range().byte_len() != 4096
                || segment.memory_size() > 4096
                || !segment.permissions().readable()
                || !segment.permissions().executable()
                || segment.permissions().writable()
            {
                errors.push("init load segment is not one bounded RX page".to_owned());
            }
        }
        _ => errors.push("init load-segment iteration is inconsistent".to_owned()),
    }
    match fs::metadata(&artifacts.embedded_elf) {
        Ok(metadata) if metadata.len() <= 128 * 1024 => {}
        Ok(metadata) => errors.push(format!(
            "embeddable init ELF is unexpectedly large: {} bytes",
            metadata.len()
        )),
        Err(error) => errors.push(format!("could not stat embeddable init ELF: {error}")),
    }
    match fs::metadata(&artifacts.map) {
        Ok(metadata) if metadata.len() > 0 => {}
        Ok(_) => errors.push("init linker map is empty".to_owned()),
        Err(error) => errors.push(format!("could not stat init linker map: {error}")),
    }

    let Some(nm) = llvm_tool("llvm-nm") else {
        eprintln!("error: llvm-nm is unavailable; install the llvm-tools component");
        return false;
    };
    let Some(symbol_output) = capture_command(
        Command::new(nm)
            .arg("--defined-only")
            .arg("--numeric-sort")
            .arg(&artifacts.embedded_elf),
    ) else {
        eprintln!("error: llvm-nm could not inspect the init ELF");
        return false;
    };
    let symbols = parse_nm_symbols(&symbol_output);
    for symbol in [
        "_start",
        "__init_start",
        "__init_end",
        "__init_message_start",
        "__init_message_end",
    ] {
        if !symbols.contains_key(symbol) {
            errors.push(format!("required init symbol '{symbol}' is missing"));
        }
    }
    if symbols.get("_start") != Some(&(INIT_ENTRY as u64))
        || symbols.get("__init_start") != Some(&(INIT_ENTRY as u64))
    {
        errors.push("init start symbols do not match the fixed entry".to_owned());
    }
    if symbols
        .get("__init_end")
        .and_then(|end| end.checked_sub(INIT_ENTRY as u64))
        .is_none_or(|size| size == 0 || size > 4096)
    {
        errors.push("init linked text is empty or exceeds one page".to_owned());
    }
    match (
        symbols.get("__init_message_start"),
        symbols.get("__init_message_end"),
    ) {
        (Some(start), Some(end)) if end.checked_sub(*start) == Some(INIT_MESSAGE.len() as u64) => {}
        _ => errors.push("init message symbols do not describe the expected bytes".to_owned()),
    }
    if !bytes
        .windows(INIT_MESSAGE.len())
        .any(|window| window == INIT_MESSAGE)
    {
        errors.push("init ELF does not contain the expected userspace message".to_owned());
    }

    if errors.is_empty() {
        println!("ok: init ELF is accepted by the Phoenix loader");
        println!("ok: init entry, RX page, message, symbols, size bound, and map");
        true
    } else {
        for error in errors {
            eprintln!("error: {error}");
        }
        false
    }
}

fn build_kernel_variant(variant: KernelVariant) -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target, variant);
    let initramfs = if matches!(variant, KernelVariant::LoadedInitProbe) {
        let Some(path) = prepare_initramfs(&target) else {
            return false;
        };
        Some(path)
    } else {
        None
    };
    let Some(output_dir) = artifacts.image.parent() else {
        eprintln!("error: invalid kernel artifact path");
        return false;
    };
    if let Err(error) = fs::create_dir_all(output_dir) {
        eprintln!("error: could not create {}: {error}", output_dir.display());
        return false;
    }

    let link_arg = format!("-Clink-arg=-Map={}", artifacts.map.display());
    let mut cargo = Command::new("cargo");
    cargo.args([
        "--config",
        ".cargo/lsp.toml",
        "rustc",
        "--package",
        "phoenix-kernel",
        "--bin",
        "phoenix-kernel",
        "--target",
        &target,
    ]);
    cargo.arg("--target-dir").arg(&artifacts.cargo_target_dir);
    let features = variant.features();
    if !features.is_empty() {
        cargo.arg("--features").arg(features.join(","));
    }
    if let Some(initramfs) = initramfs {
        cargo.env("PHOENIX_INITRAMFS", initramfs);
    }
    cargo.args(["--", &link_arg]);
    if !run_command(
        &format!("build AArch64 kernel ELF ({})", variant.name()),
        &mut cargo,
    ) {
        return false;
    }

    let Some(objcopy) = llvm_tool("llvm-objcopy") else {
        eprintln!("error: llvm-objcopy is unavailable; install the llvm-tools component");
        return false;
    };
    let mut command = Command::new(objcopy);
    command
        .arg("-O")
        .arg("binary")
        .arg(&artifacts.elf)
        .arg(&artifacts.image);
    if !run_command("create raw AArch64 kernel image", &mut command) {
        return false;
    }

    println!("artifact: ELF: {}", artifacts.elf.display());
    println!("artifact: raw image: {}", artifacts.image.display());
    println!("artifact: linker map: {}", artifacts.map.display());
    true
}

fn parse_nm_symbols(output: &str) -> BTreeMap<String, u64> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let address = u64::from_str_radix(fields.next()?, 16).ok()?;
            let _kind = fields.next()?;
            let name = fields.next()?;
            Some((name.to_owned(), address))
        })
        .collect()
}

fn validate_symbol_layout(symbols: &BTreeMap<String, u64>, expect_el0_probe: bool) -> Vec<String> {
    let required = [
        "_start",
        "kernel_main",
        "__kernel_start",
        "__kernel_end",
        "__bss_start",
        "__bss_end",
        "__boot_l1_page_table",
        "__boot_stack_bottom",
        "__boot_stack_top",
        "__exception_vectors",
        "__exception_vectors_end",
    ];
    let mut errors = Vec::new();
    for symbol in required {
        if !symbols.contains_key(symbol) {
            errors.push(format!("required symbol '{symbol}' is missing"));
        }
    }
    if !errors.is_empty() {
        return errors;
    }

    let start = symbols["_start"];
    let kernel_start = symbols["__kernel_start"];
    let kernel_end = symbols["__kernel_end"];
    let bss_start = symbols["__bss_start"];
    let bss_end = symbols["__bss_end"];
    let table = symbols["__boot_l1_page_table"];
    let stack_bottom = symbols["__boot_stack_bottom"];
    let stack_top = symbols["__boot_stack_top"];
    let vectors = symbols["__exception_vectors"];
    let vectors_end = symbols["__exception_vectors_end"];

    if start != KERNEL_VIRT_BASE {
        errors.push(format!(
            "_start is {start:#018x}, expected {KERNEL_VIRT_BASE:#018x}"
        ));
    }
    if kernel_start != start {
        errors.push("__kernel_start and _start differ".to_owned());
    }
    if kernel_end <= kernel_start || kernel_end - kernel_start >= 1024 * 1024 * 1024 {
        errors.push("kernel does not fit the temporary 1 GiB mapping".to_owned());
    }
    if table & 0xfff != 0 {
        errors.push("bootstrap L1 table is not 4 KiB aligned".to_owned());
    }
    if bss_end < bss_start {
        errors.push("BSS end precedes BSS start".to_owned());
    }
    if stack_bottom < bss_end || stack_top - stack_bottom != BOOT_STACK_SIZE {
        errors.push("bootstrap stack range is invalid".to_owned());
    }
    if stack_top & 0xf != 0 {
        errors.push("bootstrap stack top is not 16-byte aligned".to_owned());
    }
    if vectors & (EXCEPTION_VECTOR_TABLE_SIZE - 1) != 0 {
        errors.push("exception vector table is not 2 KiB aligned".to_owned());
    }
    if vectors_end.checked_sub(vectors) != Some(EXCEPTION_VECTOR_TABLE_SIZE) {
        errors.push("exception vector table is not exactly 2 KiB".to_owned());
    }
    if expect_el0_probe {
        let required_probe = [
            "__user_probe_entry",
            "__user_probe_start",
            "__user_probe_end",
            "__user_probe_stack_bottom",
            "__user_probe_stack_top",
        ];
        for symbol in required_probe {
            if !symbols.contains_key(symbol) {
                errors.push(format!("required EL0 probe symbol '{symbol}' is missing"));
            }
        }
        if required_probe
            .iter()
            .all(|symbol| symbols.contains_key(*symbol))
        {
            let probe_start = symbols["__user_probe_start"];
            let probe_end = symbols["__user_probe_end"];
            let probe_entry = symbols["__user_probe_entry"];
            let user_stack_bottom = symbols["__user_probe_stack_bottom"];
            let user_stack_top = symbols["__user_probe_stack_top"];
            if probe_start & 0xfff != 0
                || probe_entry != probe_start
                || probe_end.checked_sub(probe_start) != Some(4096)
            {
                errors
                    .push("EL0 probe text is not one aligned page with entry at start".to_owned());
            }
            if user_stack_bottom & 0xfff != 0
                || user_stack_top.checked_sub(user_stack_bottom) != Some(4096)
            {
                errors.push("EL0 probe stack is not one aligned page".to_owned());
            }
        }
    }
    errors
}

fn inspect_kernel() -> bool {
    inspect_kernel_variant(KernelVariant::Default)
}

fn inspect_kernel_variant(variant: KernelVariant) -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target, variant);
    for path in [&artifacts.elf, &artifacts.image, &artifacts.map] {
        if !path.is_file() {
            eprintln!(
                "error: missing artifact {}; run cargo xtask build",
                path.display()
            );
            return false;
        }
    }

    let Some(nm) = llvm_tool("llvm-nm") else {
        eprintln!("error: llvm-nm is unavailable; install the llvm-tools component");
        return false;
    };
    let Some(readobj) = llvm_tool("llvm-readobj") else {
        eprintln!("error: llvm-readobj is unavailable; install the llvm-tools component");
        return false;
    };

    let Some(nm_output) = capture_command(
        Command::new(nm)
            .arg("--defined-only")
            .arg("--numeric-sort")
            .arg(&artifacts.elf),
    ) else {
        eprintln!(
            "error: llvm-nm could not inspect {}",
            artifacts.elf.display()
        );
        return false;
    };
    let symbols = parse_nm_symbols(&nm_output);
    let mut errors = validate_symbol_layout(&symbols, variant.expects_el0_probe());

    let Some(header) = capture_command(
        Command::new(readobj)
            .arg("--file-headers")
            .arg("--program-headers")
            .arg(&artifacts.elf),
    ) else {
        eprintln!(
            "error: llvm-readobj could not inspect {}",
            artifacts.elf.display()
        );
        return false;
    };

    let expected_entry = format!("Entry: {KERNEL_VIRT_BASE:#X}");
    let expected_virtual = format!("VirtualAddress: {KERNEL_VIRT_BASE:#X}");
    let expected_physical = format!("PhysicalAddress: {KERNEL_PHYS_BASE:#X}");
    for (description, expected) in [
        ("AArch64 machine type", "Machine: EM_AARCH64".to_owned()),
        ("higher-half entry", expected_entry),
        ("higher-half load address", expected_virtual),
        ("physical load address", expected_physical),
    ] {
        if !header.contains(&expected) {
            errors.push(format!("ELF is missing {description}: {expected}"));
        }
    }

    for (description, path) in [
        ("raw image", &artifacts.image),
        ("linker map", &artifacts.map),
    ] {
        match fs::metadata(path) {
            Ok(metadata) if metadata.len() > 0 => {}
            Ok(_) => errors.push(format!("{description} is empty: {}", path.display())),
            Err(error) => errors.push(format!(
                "could not read {description} {}: {error}",
                path.display()
            )),
        }
    }

    let sentinel = variant.expected_sentinel();
    let kernel_bytes = match fs::read(&artifacts.elf) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            errors.push(format!(
                "could not read ELF {} for content inspection: {error}",
                artifacts.elf.display()
            ));
            None
        }
    };
    if kernel_bytes.as_ref().is_some_and(|bytes| {
        bytes
            .windows(sentinel.len())
            .any(|window| window == sentinel.as_bytes())
    }) {
        println!("ok: expected terminal sentinel is embedded: {sentinel}");
    } else if kernel_bytes.is_some() {
        errors.push(format!(
            "ELF does not contain expected terminal sentinel: {sentinel}"
        ));
    }

    if variant.expects_loaded_init() {
        let init = init_artifacts(&target);
        match (kernel_bytes.as_deref(), fs::read(&init.initramfs)) {
            (Some(kernel), Ok(archive))
                if !archive.is_empty()
                    && kernel
                        .windows(archive.len())
                        .any(|window| window == archive.as_slice()) =>
            {
                println!("ok: exact inspected initramfs is embedded in the kernel");
            }
            (Some(_), Ok(_)) => errors
                .push("kernel does not contain the exact inspected initramfs archive".to_owned()),
            (_, Err(error)) => errors.push(format!(
                "could not read initramfs {}: {error}",
                init.initramfs.display()
            )),
            (None, Ok(_)) => {}
        }
    }

    if errors.is_empty() {
        println!("ok: AArch64 ELF machine, entry, load addresses, and symbols");
        println!("ok: bootstrap page table, exception vectors, BSS, stack, image, and map");
        if variant.expects_el0_probe() {
            println!("ok: EL0 probe text, entry, and stack static layout");
        }
        if variant.expects_loaded_init() {
            println!("ok: loaded-init archive content and terminal protocol");
        }
        true
    } else {
        for error in errors {
            eprintln!("error: {error}");
        }
        false
    }
}

fn print_qemu_command() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    qemu::print_command(&kernel_artifacts(&target, KernelVariant::Default).image);
    true
}

fn run_kernel() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    build_kernel()
        && inspect_kernel()
        && qemu::run_interactive(
            &kernel_artifacts(&target, KernelVariant::Default).image,
            &workspace_root(),
        )
}

fn test_boot() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target, KernelVariant::Default);
    build_kernel()
        && inspect_kernel()
        && qemu::test_boot(&artifacts.image, &artifacts.qemu_log, &workspace_root())
}

fn test_el0() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target, KernelVariant::El0Probe);
    build_kernel_variant(KernelVariant::El0Probe)
        && inspect_kernel_variant(KernelVariant::El0Probe)
        && qemu::test_el0(&artifacts.image, &artifacts.qemu_el0_log, &workspace_root())
}

fn test_memory() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target, KernelVariant::BootMemoryProbe);
    build_kernel_variant(KernelVariant::BootMemoryProbe)
        && inspect_kernel_variant(KernelVariant::BootMemoryProbe)
        && qemu::test_memory(
            &artifacts.image,
            &artifacts.qemu_memory_log,
            &workspace_root(),
        )
}

fn test_init() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target, KernelVariant::LoadedInitProbe);
    build_kernel_variant(KernelVariant::LoadedInitProbe)
        && inspect_kernel_variant(KernelVariant::LoadedInitProbe)
        && qemu::test_init(
            &artifacts.image,
            &artifacts.qemu_init_log,
            &workspace_root(),
        )
}

fn ci() -> bool {
    doctor()
        && format()
        && check()
        && lint()
        && test()
        && build_init()
        && inspect_init()
        && build_kernel_variant(KernelVariant::BootMemoryProbe)
        && inspect_kernel_variant(KernelVariant::BootMemoryProbe)
        && build_kernel_variant(KernelVariant::El0Probe)
        && inspect_kernel_variant(KernelVariant::El0Probe)
        && build_kernel_variant(KernelVariant::LoadedInitProbe)
        && inspect_kernel_variant(KernelVariant::LoadedInitProbe)
        && build_kernel()
        && inspect_kernel()
}

#[cfg(test)]
mod tests {
    use super::{
        KERNEL_VIRT_BASE, make_initramfs, parse_lsp_target, parse_nm_symbols,
        validate_symbol_layout,
    };

    #[test]
    fn deterministic_initramfs_round_trips_through_kernel_parser() {
        let first = make_initramfs(b"ELF fixture").unwrap();
        let second = make_initramfs(b"ELF fixture").unwrap();
        assert_eq!(first, second);

        let archive = phoenix_kernel::initramfs::Initramfs::from_bytes(&first).unwrap();
        assert_eq!(archive.len(), 1);
        let init = archive.find("init").unwrap();
        assert_eq!(init.data(), b"ELF fixture");
        assert_eq!(init.permissions(), 0o755);
    }

    #[test]
    fn parses_target_from_build_section() {
        let config = r#"
            [build]
            target = "riscv64gc-unknown-none-elf"
        "#;

        assert_eq!(
            parse_lsp_target(config).as_deref(),
            Some("riscv64gc-unknown-none-elf")
        );
    }

    #[test]
    fn ignores_target_outside_build_section() {
        let config = r#"
            target = "wrong"
            [target.aarch64-unknown-none-softfloat]
            linker = "rust-lld"
        "#;

        assert_eq!(parse_lsp_target(config), None);
    }

    #[test]
    fn validates_expected_boot_symbol_layout() {
        let symbols = parse_nm_symbols(
            "ffffff8040080000 T __kernel_start\n\
             ffffff8040080000 T _start\n\
             ffffff8040080650 T kernel_main\n\
             ffffff8040084000 D __boot_l1_page_table\n\
             ffffff8040084800 T __exception_vectors\n\
             ffffff8040085000 T __exception_vectors_end\n\
             ffffff8040085000 D __bss_start\n\
             ffffff8040085000 B __bss_end\n\
             ffffff8040085000 B __boot_stack_bottom\n\
             ffffff8040095000 B __boot_stack_top\n\
             ffffff8040095000 B __kernel_end",
        );

        assert_eq!(symbols["_start"], KERNEL_VIRT_BASE);
        assert!(validate_symbol_layout(&symbols, false).is_empty());
    }

    #[test]
    fn validates_expected_el0_probe_symbol_layout() {
        let symbols = parse_nm_symbols(
            "ffffff8040080000 T __kernel_start\n\
             ffffff8040080000 T _start\n\
             ffffff8040080650 T kernel_main\n\
             ffffff8040084000 D __boot_l1_page_table\n\
             ffffff8040084800 T __exception_vectors\n\
             ffffff8040085000 T __exception_vectors_end\n\
             ffffff8040085000 D __bss_start\n\
             ffffff8040085000 B __bss_end\n\
             ffffff8040085000 B __boot_stack_bottom\n\
             ffffff8040095000 B __boot_stack_top\n\
             ffffff8040096000 T __user_probe_entry\n\
             ffffff8040096000 T __user_probe_start\n\
             ffffff8040097000 T __user_probe_end\n\
             ffffff8040098000 B __user_probe_stack_bottom\n\
             ffffff8040099000 B __user_probe_stack_top\n\
             ffffff8040099000 B __kernel_end",
        );

        assert!(validate_symbol_layout(&symbols, true).is_empty());
    }

    #[test]
    fn rejects_missing_or_misaligned_boot_symbols() {
        let symbols = parse_nm_symbols(
            "ffffff8040080000 T __kernel_start\n\
             ffffff8040080000 T _start\n\
             ffffff8040080650 T kernel_main\n\
             ffffff8040084001 D __boot_l1_page_table\n\
             ffffff8040085000 D __bss_start\n\
             ffffff8040085000 B __bss_end\n\
             ffffff8040085000 B __boot_stack_bottom\n\
             ffffff8040095000 B __boot_stack_top",
        );

        let errors = validate_symbol_layout(&symbols, false);
        assert!(errors.iter().any(|error| error.contains("__kernel_end")));
    }
}
