//! Independently linked user executables and deterministic minimal-system archive.
use super::*;

pub(super) fn archive(target: &str) -> Option<PathBuf> {
    let root = workspace_root();
    let output = root.join("target/phoenix").join(target).join("debug");
    fs::create_dir_all(&output).ok()?;
    let target_dir = root.join("target/phoenix/build/system-users");
    let objcopy = llvm_tool("llvm-objcopy")?;
    let mut programs = Vec::new();
    for name in ["system", "child", "fault", "cooperative"] {
        let binary = format!("phoenix-{name}");
        let mut cargo = Command::new("cargo");
        cargo.env("CARGO_INCREMENTAL", "0");
        cargo.args([
            "build",
            "--package",
            "phoenix-init",
            "--bin",
            &binary,
            "--target",
            target,
        ]);
        cargo.arg("--target-dir").arg(&target_dir);
        if !run_command(&format!("build {binary}"), &mut cargo) {
            return None;
        }
        let elf = target_dir.join(target).join("debug").join(&binary);
        let stripped = output.join(format!("{binary}.elf"));
        if !run_command(
            "strip user debug data",
            Command::new(&objcopy)
                .arg("--strip-debug")
                .arg(elf)
                .arg(&stripped),
        ) {
            return None;
        }
        let bytes = fs::read(&stripped).ok()?;
        let image = phoenix_kernel::elf::ElfImage::parse(&bytes).ok()?;
        if image.load_segment_count() != 1 || image.entry().as_usize() != INIT_ENTRY {
            return None;
        }
        for segment in image.load_segments() {
            let segment = segment.ok()?;
            if segment.memory_size() > 4096
                || !segment.permissions().executable()
                || segment.permissions().writable()
            {
                return None;
            }
        }
        programs.push(bytes);
    }
    let mut archive = Vec::new();
    for (index, (name, bytes)) in ["init", "child", "fault"]
        .into_iter()
        .zip(programs.iter())
        .enumerate()
    {
        append_newc_record(&mut archive, index as u32 + 1, name, 0o100_755, bytes).ok()?;
    }
    for (index, name) in [
        "fault-p", "fault-t", "fault-n", "fault-i", "fault-s", "fault-w", "fault-l", "fault-r",
        "fault-g",
    ]
    .iter()
    .enumerate()
    {
        append_newc_record(
            &mut archive,
            index as u32 + 4,
            name,
            0o100_755,
            &programs[2],
        )
        .ok()?;
    }
    append_newc_record(&mut archive, 13, "etc", 0o040_755, &[]).ok()?;
    append_newc_record(&mut archive, 14, "etc/motd", 0o100_444, INIT_FILE).ok()?;
    append_newc_record(&mut archive, 15, "stress", 0o100_755, &programs[0]).ok()?;
    append_newc_record(&mut archive, 16, "badelf", 0o100_755, b"not an ELF").ok()?;
    append_newc_record(&mut archive, 17, "cooperate", 0o100_755, &programs[3]).ok()?;
    append_newc_record(&mut archive, 18, "yield-child", 0o100_755, &programs[3]).ok()?;
    append_newc_record(&mut archive, 19, "preempt", 0o100_755, &programs[3]).ok()?;
    append_newc_record(&mut archive, 20, "busy-child", 0o100_755, &programs[3]).ok()?;
    append_newc_record(&mut archive, 21, "sleeptest", 0o100_755, &programs[3]).ok()?;
    append_newc_record(&mut archive, 22, "nap-child", 0o100_755, &programs[3]).ok()?;
    append_newc_record(&mut archive, 0, "TRAILER!!!", 0, &[]).ok()?;
    let parsed = phoenix_kernel::initramfs::Initramfs::from_bytes(&archive).ok()?;
    for (index, name) in ["init", "child", "fault"].iter().enumerate() {
        if parsed.executable_file(name).ok()? != programs[index] {
            return None;
        }
    }
    let path = output.join("phoenix-system.cpio");
    fs::write(&path, archive).ok()?;
    println!(
        "ok: independently linked init/child/fault/cooperative ELF images and system archive inspected"
    );
    Some(path)
}
