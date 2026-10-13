//! Reviewable artifact directory with exact tool/source provenance and checksums.
use super::*;

pub(super) fn run() -> bool {
    match create() {
        Ok(path) => {
            println!("package: {}", path.display());
            true
        }
        Err(error) => {
            eprintln!("package failed: {error}");
            false
        }
    }
}
fn capture(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(workspace_root())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn create() -> Result<PathBuf, String> {
    let target = require_aarch64_target().ok_or("target missing")?;
    if !build_kernel_variant(KernelVariant::System)
        || !inspect_kernel_variant(KernelVariant::System)
    {
        return Err("system build/inspection failed".into());
    }
    let artifacts = kernel_artifacts(&target, KernelVariant::System);
    let directory = artifacts.image.parent().ok_or("image directory missing")?;
    let package = workspace_root()
        .join("target/phoenix/packages")
        .join(phoenix_kernel::VERSION);
    fs::create_dir_all(&package).map_err(|error| error.to_string())?;
    let mut files = vec![
        (artifacts.elf.clone(), "phoenix-system.kernel.elf"),
        (artifacts.image.clone(), "phoenix-system.bin"),
        (artifacts.map.clone(), "phoenix-system.map"),
    ];
    for name in [
        "phoenix-system.cpio",
        "phoenix-system.elf",
        "phoenix-child.elf",
        "phoenix-fault.elf",
    ] {
        files.push((directory.join(name), name));
    }
    for (source, name) in &files {
        fs::copy(source, package.join(name)).map_err(|error| error.to_string())?;
    }
    let provenance = format!(
        "Phoenix {}\nsource={}\nworktree={}\ncheckout={}\ntarget={target}\nprofile=debug\nCARGO_INCREMENTAL=0\ncommand=cargo xtask package\nboard=virt-9.1,virtualization=on,gic-version=3,highmem=off\ncpu=cortex-a72\naccel=tcg\nram=512M\ncpus=1\n\n{}\n\n{}\n\nQEMU: {}\n",
        phoenix_kernel::VERSION,
        capture("git", &["rev-parse", "HEAD"])?,
        if capture("git", &["status", "--porcelain"])?.is_empty() {
            "clean"
        } else {
            "dirty"
        },
        workspace_root().display(),
        capture("rustc", &["-Vv"])?,
        capture("cargo", &["-V"])?,
        capture("qemu-system-aarch64", &["--version"])
            .unwrap_or_else(|_| "not installed (package not runtime verified here)".into()),
    );
    fs::write(package.join("PROVENANCE.txt"), provenance).map_err(|error| error.to_string())?;
    let output = Command::new("sha256sum")
        .current_dir(&package)
        .args(files.iter().map(|(_, name)| *name))
        .arg("PROVENANCE.txt")
        .output()
        .map_err(|error| format!("sha256sum is required: {error}"))?;
    if !output.status.success() {
        return Err("artifact hashing failed".into());
    }
    fs::write(package.join("SHA256SUMS"), output.stdout).map_err(|error| error.to_string())?;
    Ok(package)
}
