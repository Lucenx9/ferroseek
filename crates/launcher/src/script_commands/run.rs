// SPDX-License-Identifier: GPL-3.0-or-later
//! Own the process group and drain its combined output away from the UI.

use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Plan, Script};

const MAX_OUTPUT: usize = 64 * 1024;
const TICK: Duration = Duration::from_millis(25);
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Exited(ExitStatus),
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub completion: Result<Completion, String>,
    pub failed_step: Option<String>,
    pub output: String,
    pub truncated: bool,
}

#[derive(Default)]
struct Process {
    child: Option<Child>,
    cancelled: bool,
}

pub struct Job(Arc<Mutex<Process>>);

impl Job {
    pub fn start(
        plan: Arc<Plan>,
        inputs: Vec<String>,
        progress: impl FnMut(usize) + Send + 'static,
    ) -> (Self, impl Future<Output = Report>) {
        let process = Arc::new(Mutex::new(Process::default()));
        let job = Self(Arc::clone(&process));
        let work = zruntime::unblock(move || run_plan(&plan, &inputs, &process, progress));
        (job, work)
    }

    pub fn cancel(&self) {
        let mut process = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        process.cancelled = true;
        process.signal();
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Process {
    fn signal(&self) {
        if let Some(child) = &self.child {
            // The group was created for this job. Keep the direct child owned
            // and unreaped under this lock until after signalling its group.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        }
    }

    fn completion(&mut self, deadline: Instant) -> io::Result<Option<Completion>> {
        if self.cancelled {
            return Ok(Some(Completion::Cancelled));
        }
        if let Some(child) = &mut self.child
            && let Some(status) = child.try_wait()?
        {
            self.child = None;
            return Ok(Some(Completion::Exited(status)));
        }
        if Instant::now() >= deadline {
            self.signal();
            return Ok(Some(Completion::TimedOut));
        }
        Ok(None)
    }
}

fn spawn(script: &Script, inputs: &[String], process: &Mutex<Process>) -> io::Result<UnixStream> {
    let (output, writer) = UnixStream::pair()?;
    output.set_read_timeout(Some(TICK))?;
    let stderr = writer.try_clone()?;
    let mut command = Command::new(&script.executable);
    command
        .args(&script.args)
        .args(inputs)
        .current_dir(script.executable.parent().expect("validated absolute executable path"))
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::from(OwnedFd::from(stderr)));

    let mut process = process.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !process.cancelled {
        process.child = Some(command.spawn()?);
    }
    Ok(output)
}

fn run_step(
    script: &Script,
    inputs: &[String],
    process: &Mutex<Process>,
    capture: &mut Capture,
) -> io::Result<Completion> {
    let output = spawn(script, inputs, process)?;
    let result = collect(output, process, Instant::now() + TIMEOUT, capture);
    let child = {
        let mut state = process.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.signal();
        state.child.take()
    };
    if let Some(mut child) = child {
        child.wait()?;
    }
    result
}

fn run_plan(plan: &Plan, inputs: &[String], process: &Mutex<Process>, mut progress: impl FnMut(usize)) -> Report {
    let mut capture = Capture::default();
    if inputs.len() != plan.parameters.len() {
        return capture.report(Err("Enter a value for every argument.".into()), None);
    }
    for (index, step) in plan.steps.iter().enumerate() {
        progress(index);
        if plan.is_workflow() {
            capture.section(index, &step.script.title);
        }
        let result = run_step(&step.script, &step.inputs(inputs), process, &mut capture)
            .map_err(|error| format!("Could not run command. {error}"));
        let failed = !matches!(result, Ok(Completion::Exited(status)) if status.success());
        if failed || index + 1 == plan.steps.len() {
            let failed_step =
                (failed && plan.is_workflow()).then(|| format!("Step {} · {}", index + 1, step.script.title));
            return capture.report(result, failed_step);
        }
    }
    capture.report(Err("This workflow has no steps.".into()), None)
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(PartialEq, Eq)]
enum Stream {
    Read,
    Pending,
    Closed,
}

impl Capture {
    fn append(&mut self, bytes: &[u8]) {
        let count = bytes.len().min(MAX_OUTPUT - self.bytes.len());
        self.bytes.extend_from_slice(&bytes[..count]);
        self.truncated |= count < bytes.len();
    }

    fn read_chunk(&mut self, output: &mut UnixStream) -> io::Result<Stream> {
        let mut buffer = [0; 4096];
        match output.read(&mut buffer) {
            Ok(0) => Ok(Stream::Closed),
            Ok(size) => {
                self.append(&buffer[..size]);
                Ok(Stream::Read)
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(Stream::Pending)
            }
            Err(err) => Err(err),
        }
    }

    fn section(&mut self, index: usize, title: &str) {
        if index > 0 {
            self.append(b"\n");
        }
        self.append(format!("[{} · {title}]\n", index + 1).as_bytes());
    }

    fn report(self, completion: Result<Completion, String>, failed_step: Option<String>) -> Report {
        let output = String::from_utf8_lossy(&self.bytes)
            .chars()
            .filter(|&c| matches!(c, '\n' | '\t') || !c.is_control())
            .collect();
        Report { completion, failed_step, output, truncated: self.truncated }
    }

    fn drain(&mut self, output: &mut UnixStream) -> io::Result<()> {
        // A detached descendant may still hold the stream after its parent exits.
        output.set_nonblocking(true)?;
        while !self.truncated && self.read_chunk(output)? == Stream::Read {}
        Ok(())
    }
}

fn collect(
    mut output: UnixStream,
    process: &Mutex<Process>,
    deadline: Instant,
    capture: &mut Capture,
) -> io::Result<Completion> {
    let mut open = true;
    let completion = loop {
        if let Some(end) = process.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).completion(deadline)? {
            break end;
        }
        if open {
            open = capture.read_chunk(&mut output)? != Stream::Closed;
        } else {
            std::thread::sleep(TICK);
        }
    };
    if open {
        capture.drain(&mut output)?;
    }
    Ok(completion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn shell(source: &str, args: &[&str]) -> Script {
        Script {
            title: "Synthetic command".into(),
            description: String::new(),
            executable: PathBuf::from("/bin/sh"),
            args: ["-c", source, "fixture"].into_iter().chain(args.iter().copied()).map(str::to_string).collect(),
            parameters: vec![],
            confirm: false,
        }
    }

    fn run(script: Script, inputs: &[String]) -> Report {
        run_plan(&Plan::single(Arc::new(script)), inputs, &Mutex::new(Process::default()), |_| {})
    }

    #[test]
    fn captures_literal_arguments_and_stderr_even_on_failure() {
        let script = Script {
            parameters: vec!["Literal argument".into()],
            ..shell("printf '%s\\n' \"$1\" \"$2\"; printf 'fixture error\\n' >&2; exit 7", &["--fixed"])
        };
        let report = run(script, &["a path $HOME ; *.txt".into()]);
        assert_eq!(report.output, "--fixed\na path $HOME ; *.txt\nfixture error\n");
        assert!(matches!(report.completion, Ok(Completion::Exited(status)) if status.code() == Some(7)));
    }

    #[test]
    fn keeps_a_bounded_output_prefix_while_draining_the_child() {
        let chunk = "x".repeat(4096);
        let script = shell("n=0; while [ \"$n\" -lt 32 ]; do printf '%s' \"$1\"; n=$((n+1)); done", &[&chunk]);
        let report = run(script, &[]);
        assert_eq!((report.output.len(), report.truncated), (MAX_OUTPUT, true));
        assert!(matches!(report.completion, Ok(Completion::Exited(status)) if status.success()));
    }

    #[test]
    fn a_failed_step_keeps_earlier_output_and_stops_the_workflow() {
        let mut commands = Vec::new();
        for (title, source) in [
            ("Build", "printf 'Build complete\\n'"),
            ("Check", "printf 'Check failed\\n' >&2; exit 7"),
            ("Publish", "printf 'Must not run\\n'"),
        ] {
            commands.push(Arc::new(Script { title: title.into(), ..shell(source, &[]) }));
        }
        let workflow = super::super::Workflow {
            title: "Prepare".into(),
            description: String::new(),
            steps: commands.iter().map(|script| script.title.clone()).collect(),
        };
        let plan = Plan::workflow(&workflow, &commands).unwrap();
        let mut started = Vec::new();
        let report = run_plan(&plan, &[], &Mutex::new(Process::default()), |index| started.push(index));
        assert_eq!(started, [0, 1]);
        assert_eq!(report.failed_step.as_deref(), Some("Step 2 · Check"));
        assert_eq!(report.output, "[1 · Build]\nBuild complete\n\n[2 · Check]\nCheck failed\n");
    }

    #[test]
    fn cancellation_and_timeout_terminate_the_active_process() {
        let script = shell("exec sleep 60", &[]);
        for cancel in [false, true] {
            let job = Job(Arc::new(Mutex::new(Process::default())));
            let output = spawn(&script, &[], &job.0).unwrap();
            if cancel {
                job.cancel();
            }
            let completion = collect(output, &job.0, Instant::now(), &mut Capture::default()).unwrap();
            let expected = if cancel { Completion::Cancelled } else { Completion::TimedOut };
            assert_eq!(completion, expected);
            let mut child = job.0.lock().unwrap().child.take().unwrap();
            assert!(!child.wait().unwrap().success());
        }
    }
}
