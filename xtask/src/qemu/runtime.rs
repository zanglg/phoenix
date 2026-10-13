//! Runtime probes require guest-driven success AND a clean emulator exit.
use super::*;

pub(super) fn suite(image: &Path, directory: &Path, root: &Path) -> bool {
    if !preflight() {
        return false;
    }
    [
        "state",
        "state-el1",
        "panic",
        "registers",
        "guard",
        "readonly",
        "nx",
        "timer",
        "threads",
        "heap-low",
        "heap-high",
        "stack-guards",
        "fatal",
    ]
    .into_iter()
    .all(|name| run_probe(image, directory, root, name))
}

pub(super) fn system(image: &Path, directory: &Path, root: &Path) -> bool {
    preflight() && run_probe(image, directory, root, "system")
}

fn valid_transcript(output: &[u8], name: &str) -> bool {
    let normalized = String::from_utf8_lossy(output).replace('\r', "");
    if name == "state-el1" {
        return normalized
            .lines()
            .any(|line| line == "entry exception level: EL1")
            && valid_transcript(output, "state");
    }
    if name == "system" {
        return !normalized.contains("PHOENIX_PANIC")
            && !normalized.contains("PHOENIX_SYSTEM_FAIL")
            && normalized
                .lines()
                .filter(|line| *line == "PHOENIX_SYSTEM_OK")
                .count()
                == 1
            && contains_ordered(
                normalized.as_bytes(),
                &[
                    "PHOENIX_SYSTEM_ENTER",
                    "Phoenix system: init",
                    "Phoenix initramfs: file I/O works",
                    "child: reaped=1.1 fault=false frames=restored",
                    "child: reaped=1.2 fault=false frames=restored",
                    "PHOENIX_USER_FAULT",
                    "child: reaped=1.3 fault=true frames=restored",
                    "child: reaped=1.4 fault=true frames=restored",
                    "child: reaped=1.5 fault=true frames=restored",
                    "child: reaped=1.6 fault=true frames=restored",
                    "child: reaped=1.7 fault=true frames=restored",
                    "child: reaped=1.8 fault=true frames=restored",
                    "child: reaped=1.9 fault=true frames=restored",
                    "child: reaped=1.10 fault=true frames=restored",
                    "child: reaped=1.11 fault=true frames=restored",
                    "Phoenix system: file, console, child and fault checks passed",
                    "system: children=11 faults=9 console-read=3 frame-accounting=ok",
                    "PHOENIX_SYSTEM_OK",
                ],
            );
    }
    let begin = format!("PHOENIX_TEST_BEGIN {name}");
    let lines: Vec<_> = normalized.lines().collect();
    if lines
        .iter()
        .filter(|line| **line == "PHOENIX_TEST_OK")
        .count()
        != 1
        || (name != "fatal" && lines.iter().any(|line| line.contains(EXCEPTION_SENTINEL)))
        || (name != "panic" && lines.iter().any(|line| line.contains(PANIC_SENTINEL)))
    {
        return false;
    }
    let expected = match name {
        "state" => "state: EL=1 ",
        "panic" => "PHOENIX_PANIC",
        "registers" => "registers: saved=31 restored=31 eret=ok",
        "guard" => "fault: guard ",
        "readonly" => "fault: readonly ",
        "nx" => "fault: nx ",
        "heap-low" => "fault: heap-low ",
        "heap-high" => "fault: heap-high ",
        "stack-guards" => "stack: guards=2 recovered=2 while-mapped=yes",
        "fatal" => "PHOENIX_EXCEPTION",
        "threads" => "threads: workers=2 preemption=ok sleep=ok exit=ok reclaimed=16 ",
        "timer" => "timer: intid=30 event=20552 irq=1 eoi=ok return=ok lock=ok gates=closed",
        _ => return false,
    };
    let Some(start) = lines.iter().position(|line| *line == begin) else {
        return false;
    };
    let Some(end) = lines.iter().position(|line| *line == "PHOENIX_TEST_OK") else {
        return false;
    };
    (name != "panic"
        || (normalized.contains("deliberate runtime probe")
            && normalized.contains("trace: return=0x")))
        && (name != "fatal"
            || (normalized.contains("esr: 0x00000000f2000456")
                && normalized.contains("x30: 0x")
                && normalized.contains("trace: return=0x")))
        && start < end
        && lines[start + 1..end]
            .iter()
            .any(|line| line.starts_with(expected))
}

fn run_probe(image: &Path, directory: &Path, root: &Path, name: &str) -> bool {
    let mut specification = QemuCommand::for_kernel(image);
    if name == "state-el1" {
        for argument in &mut specification.arguments {
            if argument.to_string_lossy().contains("virtualization=on") {
                *argument = argument
                    .to_string_lossy()
                    .replace("virtualization=on", "virtualization=off")
                    .into();
            }
        }
    }
    specification.arguments.extend([
        "-semihosting-config".into(),
        "enable=on,target=native".into(),
        "-append".into(),
        if name == "system" {
            "phoenix.system-test".into()
        } else if name == "state-el1" {
            "phoenix.probe=state".into()
        } else {
            format!("phoenix.probe={name}").into()
        },
    ]);
    println!(
        "==> runtime probe {name}\ncommand: {}",
        specification.display()
    );
    let mut child = match specification
        .command()
        .current_dir(root)
        .stdin(if name == "system" {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("could not spawn QEMU: {error}");
            return false;
        }
    };
    if name == "system" {
        use std::io::Write;
        if child.stdin.take().unwrap().write_all(b"ok\n").is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    }
    let (tx, rx) = mpsc::channel();
    let stdout = spawn_reader(child.stdout.take().unwrap(), tx.clone(), "stdout");
    let stderr = spawn_reader(child.stderr.take().unwrap(), tx, "stderr");
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut output = Vec::new();
    let mut failure = None;
    let status = loop {
        if Instant::now() >= deadline {
            failure = Some("timeout".to_owned());
            break None;
        }
        if output.len() > OUTPUT_LIMIT {
            failure = Some("output-limit".to_owned());
            break None;
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(StreamEvent::Data(bytes)) => output.extend_from_slice(&bytes),
            Ok(StreamEvent::Error(error)) => {
                failure = Some(error);
                break None;
            }
            _ => (),
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => (),
            Err(error) => {
                failure = Some(error.to_string());
                break None;
            }
        }
    };
    if status.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = stdout.join();
    let _ = stderr.join();
    drain_output(&rx, &mut output);
    let log = directory.join(format!("qemu-{name}.log"));
    if let Err(error) = fs::write(&log, &output) {
        eprintln!("could not save {}: {error}", log.display());
        return false;
    }
    print!("{}", String::from_utf8_lossy(&output));
    let passed = failure.is_none()
        && output.len() <= OUTPUT_LIMIT
        && status.is_some_and(|status| status.success())
        && valid_transcript(&output, name);
    println!(
        "QEMU_TEST_LOG={}\nQEMU_TEST_RESULT={}",
        log.display(),
        if passed { "pass" } else { "fail" }
    );
    if !passed {
        eprintln!("probe={name} status={status:?} failure={failure:?}");
    }
    passed
}

#[cfg(test)]
mod tests {
    use super::valid_transcript;
    #[test]
    fn requires_exact_complete_terminal_and_prior_evidence() {
        let good = b"PHOENIX_TEST_BEGIN registers\r\nregisters: saved=31 restored=31 eret=ok\r\nPHOENIX_TEST_OK\r\n";
        assert!(valid_transcript(good, "registers"));
        assert!(!valid_transcript(good, "timer"));
        assert!(!valid_transcript(
            b"PHOENIX_TEST_BEGIN state\nPHOENIX_TEST_OK\n",
            "state"
        ));
        assert!(!valid_transcript(
            b"PHOENIX_TEST_BEGIN state\nstate: EL=1 SP=0\nPHOENIX_TEST_OKAY\n",
            "state"
        ));
        let mut failure = good.to_vec();
        failure.extend_from_slice(b"PHOENIX_PANIC\n");
        assert!(!valid_transcript(&failure, "registers"));
    }
}
