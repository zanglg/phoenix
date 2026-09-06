use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";
const KERNEL_PHYS_BASE: u64 = 0x0000_0000_4008_0000;
const KERNEL_VIRT_BASE: u64 = 0xffff_ff80_4008_0000;
const BOOT_STACK_SIZE: u64 = 64 * 1024;

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
    run(
        "check kernel for configured LSP target",
        "cargo",
        &[
            "--config",
            ".cargo/lsp.toml",
            "check",
            "--workspace",
            "--target",
            &target,
        ],
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
    run(
        "lint kernel for configured LSP target",
        "cargo",
        &[
            "--config",
            ".cargo/lsp.toml",
            "clippy",
            "--workspace",
            "--lib",
            "--bin",
            "phoenix-kernel",
            "--target",
            &target,
            "--",
            "-D",
            "warnings",
        ],
    ) && run(
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
    ) && run(
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
        &["test", "--workspace", "--lib"],
    ) && run(
        "test host tooling",
        "cargo",
        &["test", "--manifest-path", "xtask/Cargo.toml"],
    )
}

struct Artifacts {
    elf: PathBuf,
    image: PathBuf,
    map: PathBuf,
}

fn kernel_artifacts(target: &str) -> Artifacts {
    let root = workspace_root();
    let output = root.join("target/phoenix").join(target).join("debug");
    Artifacts {
        elf: root
            .join("target")
            .join(target)
            .join("debug/phoenix-kernel"),
        image: output.join("phoenix-kernel.bin"),
        map: output.join("phoenix-kernel.map"),
    }
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
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target);
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
        "--",
        &link_arg,
    ]);
    if !run_command("build AArch64 kernel ELF", &mut cargo) {
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

fn validate_symbol_layout(symbols: &BTreeMap<String, u64>) -> Vec<String> {
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
    errors
}

fn inspect_kernel() -> bool {
    let Some(target) = require_aarch64_target() else {
        return false;
    };
    let artifacts = kernel_artifacts(&target);
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
    let mut errors = validate_symbol_layout(&symbols);

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

    if errors.is_empty() {
        println!("ok: AArch64 ELF machine, entry, load addresses, and symbols");
        println!("ok: bootstrap page table, BSS, stack, raw image, and linker map");
        true
    } else {
        for error in errors {
            eprintln!("error: {error}");
        }
        false
    }
}

fn ci() -> bool {
    doctor() && format() && check() && lint() && test() && build_kernel() && inspect_kernel()
}

#[cfg(test)]
mod tests {
    use super::{KERNEL_VIRT_BASE, parse_lsp_target, parse_nm_symbols, validate_symbol_layout};

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
             ffffff8040085000 D __bss_start\n\
             ffffff8040085000 B __bss_end\n\
             ffffff8040085000 B __boot_stack_bottom\n\
             ffffff8040095000 B __boot_stack_top\n\
             ffffff8040095000 B __kernel_end",
        );

        assert_eq!(symbols["_start"], KERNEL_VIRT_BASE);
        assert!(validate_symbol_layout(&symbols).is_empty());
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

        let errors = validate_symbol_layout(&symbols);
        assert!(errors.iter().any(|error| error.contains("__kernel_end")));
    }
}
