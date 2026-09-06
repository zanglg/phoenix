use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

const PROGRAM: &str = "qemu-system-aarch64";
const MACHINE: &str = "virt-9.2,virtualization=on,gic-version=3,highmem=off";
const MACHINE_TYPE: &str = "virt-9.2";
const CPU: &str = "cortex-a72";
const MEMORY: &str = "512M";
const CPUS: &str = "1";
const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const OUTPUT_LIMIT: usize = 1024 * 1024;

pub const BOOT_SUCCESS_SENTINEL: &str = "PHOENIX_BOOT_OK";
pub const PANIC_SENTINEL: &str = "PHOENIX_PANIC";
pub const EXCEPTION_SENTINEL: &str = "PHOENIX_EXCEPTION";
pub const MEMORY_SUCCESS_SENTINEL: &str = "PHOENIX_MEMORY_OK";
pub const EL0_SUCCESS_SENTINEL: &str = "PHOENIX_EL0_OK";
pub const EL0_FAILURE_SENTINEL: &str = "PHOENIX_EL0_FAIL";
pub const INIT_SUCCESS_SENTINEL: &str = "PHOENIX_INIT_OK";
pub const INIT_FAILURE_SENTINEL: &str = "PHOENIX_INIT_FAIL";
pub const INIT_USER_OUTPUT: &str =
    "Phoenix init: hello from EL0\nPhoenix initramfs: file I/O works\n";

struct QemuCommand {
    arguments: Vec<OsString>,
}

impl QemuCommand {
    fn for_kernel(image: &Path) -> Self {
        Self {
            arguments: vec![
                "-machine".into(),
                MACHINE.into(),
                "-cpu".into(),
                CPU.into(),
                "-accel".into(),
                "tcg".into(),
                "-m".into(),
                MEMORY.into(),
                "-smp".into(),
                CPUS.into(),
                "-nodefaults".into(),
                "-nographic".into(),
                "-monitor".into(),
                "none".into(),
                "-serial".into(),
                "stdio".into(),
                "-no-reboot".into(),
                "-kernel".into(),
                image.as_os_str().to_owned(),
            ],
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(PROGRAM);
        command.args(&self.arguments);
        command
    }

    fn display(&self) -> String {
        std::iter::once(OsString::from(PROGRAM))
            .chain(self.arguments.iter().cloned())
            .map(|argument| shell_quote(&argument))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalOutput {
    Success,
    Failure,
    ProtocolFailure,
    Panic,
    Exception,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedOutput {
    Boot,
    Memory,
    El0,
    Init,
}

impl ExpectedOutput {
    const fn sentinel(self) -> &'static str {
        match self {
            Self::Boot => BOOT_SUCCESS_SENTINEL,
            Self::Memory => MEMORY_SUCCESS_SENTINEL,
            Self::El0 => EL0_SUCCESS_SENTINEL,
            Self::Init => INIT_SUCCESS_SENTINEL,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Boot => "AArch64 boot",
            Self::Memory => "AArch64 boot-memory probe",
            Self::El0 => "AArch64 EL0 probe",
            Self::Init => "dynamically loaded AArch64 init probe",
        }
    }

    const fn failure_sentinel(self) -> Option<&'static str> {
        match self {
            Self::El0 => Some(EL0_FAILURE_SENTINEL),
            Self::Init => Some(INIT_FAILURE_SENTINEL),
            Self::Boot | Self::Memory => None,
        }
    }

    const fn required_output(self) -> Option<&'static str> {
        match self {
            Self::Init => Some(INIT_USER_OUTPUT),
            Self::Boot | Self::Memory | Self::El0 => None,
        }
    }
}

#[derive(Debug)]
enum TestOutcome {
    Success,
    Failure,
    ProtocolFailure,
    Panic,
    Exception,
    Timeout,
    OutputLimit,
    Exited(ExitStatus),
    StreamError(String),
}

enum StreamEvent {
    Data(Vec<u8>),
    Error(String),
    Done,
}

pub fn print_command(image: &Path) {
    println!("{}", QemuCommand::for_kernel(image).display());
}

pub fn run_interactive(image: &Path, workspace_root: &Path) -> bool {
    if !preflight() {
        return false;
    }

    let specification = QemuCommand::for_kernel(image);
    println!("==> run AArch64 kernel on QEMU");
    println!("command: {}", specification.display());
    match specification.command().current_dir(workspace_root).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("error: QEMU exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("error: could not start QEMU: {error}");
            false
        }
    }
}

pub fn test_boot(image: &Path, log: &Path, workspace_root: &Path) -> bool {
    test_kernel(image, log, workspace_root, ExpectedOutput::Boot)
}

pub fn test_el0(image: &Path, log: &Path, workspace_root: &Path) -> bool {
    test_kernel(image, log, workspace_root, ExpectedOutput::El0)
}

pub fn test_memory(image: &Path, log: &Path, workspace_root: &Path) -> bool {
    test_kernel(image, log, workspace_root, ExpectedOutput::Memory)
}

pub fn test_init(image: &Path, log: &Path, workspace_root: &Path) -> bool {
    test_kernel(image, log, workspace_root, ExpectedOutput::Init)
}

fn test_kernel(image: &Path, log: &Path, workspace_root: &Path, expected: ExpectedOutput) -> bool {
    if !preflight() {
        return false;
    }
    if let Some(parent) = log.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("error: could not create QEMU log directory: {error}");
        return false;
    }

    let specification = QemuCommand::for_kernel(image);
    println!("==> test {} on QEMU", expected.label());
    println!("command: {}", specification.display());
    println!("timeout: {} seconds", TEST_TIMEOUT.as_secs());

    let mut command = specification.command();
    command
        .current_dir(workspace_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("QEMU_TEST_RESULT=spawn-error");
            eprintln!("error: could not start QEMU: {error}");
            return false;
        }
    };

    let stdout = child.stdout.take().expect("piped QEMU stdout must exist");
    let stderr = child.stderr.take().expect("piped QEMU stderr must exist");
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = spawn_reader(stdout, sender.clone(), "stdout");
    let stderr_reader = spawn_reader(stderr, sender, "stderr");

    let (outcome, mut output) = observe(&mut child, &receiver, expected);
    if matches!(
        outcome,
        TestOutcome::Success
            | TestOutcome::Failure
            | TestOutcome::ProtocolFailure
            | TestOutcome::Panic
            | TestOutcome::Exception
            | TestOutcome::Timeout
            | TestOutcome::OutputLimit
    ) {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    drain_output(&receiver, &mut output);

    if let Err(error) = fs::write(log, &output) {
        eprintln!("error: could not write QEMU log {}: {error}", log.display());
        return false;
    }

    if !output.is_empty() {
        println!("--- QEMU output ---");
        print!("{}", String::from_utf8_lossy(&output));
        if output.last().is_some_and(|byte| *byte != b'\n') {
            println!();
        }
        println!("--- end QEMU output ---");
    }
    println!("QEMU_TEST_LOG={}", log.display());

    match outcome {
        TestOutcome::Success => {
            println!("QEMU_TEST_RESULT=pass");
            println!("QEMU_TEST_SENTINEL={}", expected.sentinel());
            true
        }
        TestOutcome::Failure => {
            eprintln!("QEMU_TEST_RESULT=probe-failure");
            eprintln!(
                "error: observed {}",
                expected
                    .failure_sentinel()
                    .expect("failure outcome requires a failure sentinel")
            );
            false
        }
        TestOutcome::ProtocolFailure => {
            eprintln!("QEMU_TEST_RESULT=protocol-failure");
            eprintln!(
                "error: observed {} without required output {:?}",
                expected.sentinel(),
                expected
                    .required_output()
                    .expect("protocol failure requires prerequisite output")
            );
            false
        }
        TestOutcome::Panic => {
            eprintln!("QEMU_TEST_RESULT=panic");
            eprintln!(
                "error: observed {PANIC_SENTINEL} before {}",
                expected.sentinel()
            );
            false
        }
        TestOutcome::Exception => {
            eprintln!("QEMU_TEST_RESULT=exception");
            eprintln!(
                "error: observed {EXCEPTION_SENTINEL} before {}",
                expected.sentinel()
            );
            false
        }
        TestOutcome::Timeout => {
            eprintln!("QEMU_TEST_RESULT=timeout");
            eprintln!("error: did not observe a terminal sentinel before the timeout");
            false
        }
        TestOutcome::OutputLimit => {
            eprintln!("QEMU_TEST_RESULT=output-limit");
            eprintln!("error: QEMU produced more than {OUTPUT_LIMIT} bytes without a sentinel");
            false
        }
        TestOutcome::Exited(status) => {
            eprintln!("QEMU_TEST_RESULT=early-exit");
            eprintln!("error: QEMU exited with {status} before a terminal sentinel");
            false
        }
        TestOutcome::StreamError(error) => {
            eprintln!("QEMU_TEST_RESULT=stream-error");
            eprintln!("error: failed while reading QEMU output: {error}");
            false
        }
    }
}

fn preflight() -> bool {
    let Some(version) = command_output(&["--version"]) else {
        eprintln!("error: {PROGRAM} is unavailable or could not run");
        eprintln!("hint: install QEMU with AArch64 system emulation, then retry");
        return false;
    };
    let version_line = version.lines().next().unwrap_or("unknown QEMU version");
    println!("ok: {version_line}");

    let Some(machines) = command_output(&["-machine", "help"]) else {
        eprintln!("error: could not query QEMU machine types");
        return false;
    };
    if !has_named_item(&machines, MACHINE_TYPE) {
        eprintln!("error: QEMU does not provide the pinned machine '{MACHINE_TYPE}'");
        eprintln!("hint: install QEMU 9.2 or newer; do not silently substitute another board");
        return false;
    }

    let Some(cpus) = command_output(&["-cpu", "help"]) else {
        eprintln!("error: could not query QEMU CPU types");
        return false;
    };
    if !has_named_item(&cpus, CPU) {
        eprintln!("error: QEMU does not provide the pinned CPU '{CPU}'");
        return false;
    }
    true
}

fn command_output(arguments: &[&str]) -> Option<String> {
    let output = Command::new(PROGRAM).args(arguments).output().ok()?;
    output.status.success().then(|| {
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        text
    })
}

fn has_named_item(output: &str, expected: &str) -> bool {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .any(|item| item == expected)
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    sender: Sender<StreamEvent>,
    name: &'static str,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if sender
                        .send(StreamEvent::Data(buffer[..read].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = sender.send(StreamEvent::Error(format!("{name}: {error}")));
                    break;
                }
            }
        }
        let _ = sender.send(StreamEvent::Done);
    })
}

fn observe(
    child: &mut std::process::Child,
    receiver: &Receiver<StreamEvent>,
    expected: ExpectedOutput,
) -> (TestOutcome, Vec<u8>) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut output = Vec::new();
    let mut completed_streams = 0;

    loop {
        if let Some(terminal) = classify_output(&output, expected) {
            return (
                match terminal {
                    TerminalOutput::Success => TestOutcome::Success,
                    TerminalOutput::Failure => TestOutcome::Failure,
                    TerminalOutput::ProtocolFailure => TestOutcome::ProtocolFailure,
                    TerminalOutput::Panic => TestOutcome::Panic,
                    TerminalOutput::Exception => TestOutcome::Exception,
                },
                output,
            );
        }
        if output.len() > OUTPUT_LIMIT {
            output.truncate(OUTPUT_LIMIT);
            return (TestOutcome::OutputLimit, output);
        }
        match child.try_wait() {
            Ok(Some(status)) if completed_streams == 2 => {
                return (TestOutcome::Exited(status), output);
            }
            Ok(_) => {}
            Err(error) => return (TestOutcome::StreamError(error.to_string()), output),
        }

        let now = Instant::now();
        if now >= deadline {
            return (TestOutcome::Timeout, output);
        }
        let wait = deadline.saturating_duration_since(now).min(POLL_INTERVAL);
        match receiver.recv_timeout(wait) {
            Ok(StreamEvent::Data(bytes)) => output.extend_from_slice(&bytes),
            Ok(StreamEvent::Error(error)) => return (TestOutcome::StreamError(error), output),
            Ok(StreamEvent::Done) => completed_streams += 1,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => completed_streams = 2,
        }
    }
}

fn drain_output(receiver: &Receiver<StreamEvent>, output: &mut Vec<u8>) {
    for event in receiver.try_iter() {
        if let StreamEvent::Data(bytes) = event {
            let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
            output.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        }
    }
}

fn classify_output(output: &[u8], expected: ExpectedOutput) -> Option<TerminalOutput> {
    let terminal_success = find_bytes(output, expected.sentinel().as_bytes());
    let prerequisite_present = expected.required_output().is_none_or(|required| {
        terminal_success
            .is_some_and(|terminal| find_bytes(&output[..terminal], required.as_bytes()).is_some())
    });
    let success = terminal_success.filter(|_| prerequisite_present);
    let protocol_failure = terminal_success.filter(|_| !prerequisite_present);
    let failure = expected
        .failure_sentinel()
        .and_then(|sentinel| find_bytes(output, sentinel.as_bytes()));
    let panic = find_bytes(output, PANIC_SENTINEL.as_bytes());
    let exception = find_bytes(output, EXCEPTION_SENTINEL.as_bytes());
    [
        success.map(|position| (position, TerminalOutput::Success)),
        protocol_failure.map(|position| (position, TerminalOutput::ProtocolFailure)),
        failure.map(|position| (position, TerminalOutput::Failure)),
        panic.map(|position| (position, TerminalOutput::Panic)),
        exception.map(|position| (position, TerminalOutput::Exception)),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|(position, _)| *position)
    .map(|(_, terminal)| terminal)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn shell_quote(argument: &OsStr) -> String {
    let argument = argument.to_string_lossy();
    if !argument.is_empty()
        && argument
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._/:,=".contains(&byte))
    {
        argument.into_owned()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        BOOT_SUCCESS_SENTINEL, CPU, EL0_FAILURE_SENTINEL, EL0_SUCCESS_SENTINEL, EXCEPTION_SENTINEL,
        ExpectedOutput, INIT_FAILURE_SENTINEL, INIT_SUCCESS_SENTINEL, INIT_USER_OUTPUT, MACHINE,
        MEMORY_SUCCESS_SENTINEL, PANIC_SENTINEL, QemuCommand, TerminalOutput, classify_output,
        has_named_item, shell_quote,
    };

    #[test]
    fn command_is_pinned_and_uses_the_raw_image() {
        let command = QemuCommand::for_kernel(Path::new("target/kernel image.bin"));
        let display = command.display();

        assert!(display.contains(MACHINE));
        assert!(display.contains(CPU));
        assert!(display.contains("-accel tcg"));
        assert!(display.contains("-smp 1"));
        assert!(display.contains("-kernel 'target/kernel image.bin'"));
    }

    #[test]
    fn terminal_classifier_handles_split_and_competing_sentinels() {
        assert_eq!(
            classify_output(b"PHOENIX_BOOT_", ExpectedOutput::Boot),
            None
        );
        assert_eq!(
            classify_output(
                format!("boot\n{BOOT_SUCCESS_SENTINEL}\n").as_bytes(),
                ExpectedOutput::Boot
            ),
            Some(TerminalOutput::Success)
        );
        assert_eq!(
            classify_output(
                format!("{PANIC_SENTINEL}\n{BOOT_SUCCESS_SENTINEL}").as_bytes(),
                ExpectedOutput::Boot
            ),
            Some(TerminalOutput::Panic)
        );
        assert_eq!(
            classify_output(
                format!("{BOOT_SUCCESS_SENTINEL}\n{PANIC_SENTINEL}").as_bytes(),
                ExpectedOutput::Boot
            ),
            Some(TerminalOutput::Success)
        );
        assert_eq!(
            classify_output(
                format!("{EXCEPTION_SENTINEL}\n{BOOT_SUCCESS_SENTINEL}").as_bytes(),
                ExpectedOutput::Boot
            ),
            Some(TerminalOutput::Exception)
        );
    }

    #[test]
    fn el0_classifier_ignores_boot_progress_and_requires_el0_result() {
        assert_eq!(
            classify_output(
                format!("{BOOT_SUCCESS_SENTINEL}\nPHOENIX_EL0_ENTER\n").as_bytes(),
                ExpectedOutput::El0
            ),
            None
        );
        assert_eq!(
            classify_output(
                format!("{BOOT_SUCCESS_SENTINEL}\n{EL0_SUCCESS_SENTINEL}\n").as_bytes(),
                ExpectedOutput::El0
            ),
            Some(TerminalOutput::Success)
        );
        assert_eq!(
            classify_output(
                format!("{EL0_FAILURE_SENTINEL}: status=1\n").as_bytes(),
                ExpectedOutput::El0
            ),
            Some(TerminalOutput::Failure)
        );
    }

    #[test]
    fn memory_classifier_ignores_boot_progress_and_requires_memory_result() {
        assert_eq!(
            classify_output(
                format!("{BOOT_SUCCESS_SENTINEL}\nmemory: free-frames=42\n").as_bytes(),
                ExpectedOutput::Memory
            ),
            None
        );
        assert_eq!(
            classify_output(
                format!("{BOOT_SUCCESS_SENTINEL}\n{MEMORY_SUCCESS_SENTINEL}\n").as_bytes(),
                ExpectedOutput::Memory
            ),
            Some(TerminalOutput::Success)
        );
        assert_eq!(
            classify_output(
                format!("{PANIC_SENTINEL}\n{MEMORY_SUCCESS_SENTINEL}\n").as_bytes(),
                ExpectedOutput::Memory
            ),
            Some(TerminalOutput::Panic)
        );
    }

    #[test]
    fn init_classifier_requires_the_loaded_program_result() {
        assert_eq!(
            classify_output(
                format!("{BOOT_SUCCESS_SENTINEL}\n{MEMORY_SUCCESS_SENTINEL}\nPHOENIX_INIT_ENTER\n")
                    .as_bytes(),
                ExpectedOutput::Init
            ),
            None
        );
        assert_eq!(
            classify_output(INIT_SUCCESS_SENTINEL.as_bytes(), ExpectedOutput::Init),
            Some(TerminalOutput::ProtocolFailure)
        );
        assert_eq!(
            classify_output(
                format!("Phoenix init: hello from EL0\n{INIT_SUCCESS_SENTINEL}").as_bytes(),
                ExpectedOutput::Init
            ),
            Some(TerminalOutput::ProtocolFailure)
        );
        assert_eq!(
            classify_output(
                format!("{INIT_USER_OUTPUT}{INIT_SUCCESS_SENTINEL}").as_bytes(),
                ExpectedOutput::Init
            ),
            Some(TerminalOutput::Success)
        );
        assert_eq!(
            classify_output(
                format!("{INIT_FAILURE_SENTINEL}: status=1").as_bytes(),
                ExpectedOutput::Init
            ),
            Some(TerminalOutput::Failure)
        );
    }

    #[test]
    fn named_item_check_does_not_accept_prefixes() {
        let machines = "virt-9.1 old\nvirt-9.2 pinned\nvirt latest";
        assert!(has_named_item(machines, "virt-9.2"));
        assert!(!has_named_item(machines, "virt-9"));
    }

    #[test]
    fn shell_quoting_preserves_simple_values_and_escapes_spaces() {
        assert_eq!(
            shell_quote("virt-9.2,gic-version=3".as_ref()),
            "virt-9.2,gic-version=3"
        );
        assert_eq!(shell_quote("two words".as_ref()), "'two words'");
        assert_eq!(shell_quote("it's".as_ref()), "'it'\\''s'");
    }
}
