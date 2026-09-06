use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );
    let workspace_root = manifest_dir
        .parent()
        .expect("kernel must live directly below the workspace root");

    println!("cargo:rerun-if-changed=linker/aarch64-qemu-virt.ld");
    println!("cargo:rerun-if-changed=src/arch/aarch64/boot.S");
    println!("cargo:rerun-if-changed=src/arch/aarch64/vectors.S");
    println!("cargo:rerun-if-changed=src/arch/aarch64/user_probe.S");
    println!("cargo:rerun-if-env-changed=PHOENIX_INIT_ELF");
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join(".git/index").display()
    );

    export_build_identity(workspace_root);
    let target = env::var("TARGET").expect("Cargo must provide TARGET");

    if target == AARCH64_TARGET && env::var_os("CARGO_FEATURE_LOADED_INIT_PROBE").is_some() {
        let init_elf = env::var_os("PHOENIX_INIT_ELF")
            .expect("loaded-init-probe requires PHOENIX_INIT_ELF; use cargo xtask");
        println!("cargo:rerun-if-changed={}", Path::new(&init_elf).display());
        println!(
            "cargo:rustc-env=PHOENIX_INIT_ELF={}",
            Path::new(&init_elf).display()
        );
    }

    if target == AARCH64_TARGET {
        let linker_script = manifest_dir.join("linker/aarch64-qemu-virt.ld");
        println!(
            "cargo:rustc-link-arg-bin=phoenix-kernel=-T{}",
            linker_script.display()
        );
        println!("cargo:rustc-link-arg-bin=phoenix-kernel=--build-id=none");
    }
}

fn export_build_identity(workspace_root: &Path) {
    let commit = git_output(workspace_root, &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_owned());
    let dirty = git_output(workspace_root, &["status", "--porcelain"])
        .is_some_and(|output| !output.is_empty());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".to_owned());
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());

    println!("cargo:rustc-env=PHOENIX_GIT_COMMIT={commit}");
    println!(
        "cargo:rustc-env=PHOENIX_GIT_DIRTY={}",
        if dirty { "dirty" } else { "clean" }
    );
    println!("cargo:rustc-env=PHOENIX_BUILD_PROFILE={profile}");
    println!("cargo:rustc-env=PHOENIX_BUILD_TARGET={target}");
}

fn git_output(workspace_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
