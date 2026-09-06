use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

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
    println!("  doctor  Check the local development environment");
    println!("  fmt     Check Rust formatting");
    println!("  check   Check the kernel target and host tooling");
    println!("  lint    Run Clippy for the kernel target and host tooling");
    println!("  test    Run available host-side tests");
    println!("  ci      Run all validation available in version 0.0.0");
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live directly below the workspace root")
        .to_owned()
}

fn run(label: &str, program: &str, args: &[&str]) -> bool {
    println!("==> {label}");
    match Command::new(program)
        .args(args)
        .current_dir(workspace_root())
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("error: {label} exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("error: could not run {program}: {error}");
            false
        }
    }
}

fn capture(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(workspace_root())
        .output()
        .ok()?;

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
    run(
        "check kernel for configured LSP target",
        "cargo",
        &["--config", ".cargo/lsp.toml", "check", "--workspace"],
    ) && run(
        "check xtask for host",
        "cargo",
        &["check", "--manifest-path", "xtask/Cargo.toml"],
    )
}

fn lint() -> bool {
    run(
        "lint kernel for configured LSP target",
        "cargo",
        &[
            "--config",
            ".cargo/lsp.toml",
            "clippy",
            "--workspace",
            "--lib",
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
        "test host tooling",
        "cargo",
        &["test", "--manifest-path", "xtask/Cargo.toml"],
    )
}

fn ci() -> bool {
    doctor() && format() && check() && lint() && test()
}

#[cfg(test)]
mod tests {
    use super::parse_lsp_target;

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
}
