use crate::{
    command::{CommandSpec, Operation, Tool, find_tool},
    model::{Interface, Survey, clean_terminal, parse_survey},
    monitor::{self, NetworkRestore},
};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid, geteuid},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Started,
    Line(String),
    Survey(Survey),
    NetworkRestore(Option<NetworkRestore>),
    MonitorReady(Interface),
    Finished { code: Option<i32>, cancelled: bool },
    Error(String),
}

#[derive(Serialize, Deserialize)]
struct Request {
    executable: PathBuf,
    #[serde(default)]
    nmcli: Option<PathBuf>,
    operation: Operation,
}

pub struct Job {
    pub id: u64,
    pub operation: Operation,
    pub stopping: bool,
    pub started: bool,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn stop(&mut self) {
        self.stopping = true;
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct Runner {
    pub events: Receiver<(u64, Event)>,
    sender: SyncSender<(u64, Event)>,
}

impl Runner {
    pub fn new() -> Self {
        let (sender, events) = mpsc::sync_channel(512);
        Self { events, sender }
    }

    pub fn start(&self, id: u64, operation: Operation, demo: bool) -> Result<Job, String> {
        let spec = operation.spec()?;
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let sender = self.sender.clone();
        if demo {
            let operation = operation.clone();
            thread::spawn(move || {
                let _ = sender.send((id, Event::Started));
                let _ = sender.send((id, Event::Line(format!("[DEMO] {}", spec.preview()))));
                if operation.csv_path().is_some() {
                    let _ = sender.send((id, Event::Survey(crate::model::demo_survey())));
                }
                let until = Instant::now() + Duration::from_secs(2);
                while !flag.load(Ordering::Relaxed) {
                    if operation.csv_path().is_none() && Instant::now() >= until {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                let _ = sender.send((
                    id,
                    Event::Line(
                        "[DEMO] Simulated job; no command executed or capture written.".into(),
                    ),
                ));
                let _ = sender.send((
                    id,
                    Event::Finished {
                        code: Some(0),
                        cancelled: flag.load(Ordering::Relaxed),
                    },
                ));
            });
        } else {
            operation.validate_files()?;
            let executable = find_tool(spec.tool.name())
                .ok_or_else(|| format!("{} is missing from PATH.", spec.tool.name()))?;
            let helper = std::env::current_exe().map_err(|e| e.to_string())?;
            let pkexec = if spec.privileged && !geteuid().is_root() {
                Some(find_tool("pkexec").ok_or("Install polkit (pkexec) and a desktop authentication agent for radio operations.")?)
            } else {
                None
            };
            let request = Request {
                executable,
                nmcli: if matches!(operation, Operation::Monitor { .. }) {
                    find_tool("nmcli")
                } else {
                    None
                },
                operation: operation.clone(),
            };
            thread::spawn(move || {
                if let Err(error) = run_helper(id, request, helper, pkexec, flag, &sender) {
                    let _ = sender.send((id, Event::Error(error)));
                }
            });
        }
        Ok(Job {
            id,
            operation,
            stopping: false,
            started: false,
            cancel,
        })
    }
}

fn run_helper(
    id: u64,
    request: Request,
    helper: PathBuf,
    pkexec: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    sender: &SyncSender<(u64, Event)>,
) -> Result<(), String> {
    let mut command = if let Some(pkexec) = pkexec {
        let mut cmd = Command::new(pkexec);
        cmd.arg(helper);
        cmd
    } else {
        Command::new(helper)
    };
    let mut child = command
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start worker: {e}"))?;
    let mut input = child.stdin.take();
    let write_result = (|| -> io::Result<()> {
        let pipe = input.as_mut().unwrap();
        serde_json::to_writer(&mut *pipe, &request)?;
        pipe.write_all(b"\n")?;
        pipe.flush()
    })();
    if let Err(e) = write_result {
        drop(input);
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    let output = child.stdout.take().unwrap();
    let tx = sender.clone();
    let saw_finish = Arc::new(AtomicBool::new(false));
    let seen = saw_finish.clone();
    let worker_started = Arc::new(AtomicBool::new(false));
    let started = worker_started.clone();
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if let Ok(event) = serde_json::from_str::<Event>(&line) {
                if matches!(event, Event::Started) {
                    started.store(true, Ordering::Relaxed);
                }
                if matches!(event, Event::Finished { .. } | Event::Error(_)) {
                    seen.store(true, Ordering::Relaxed);
                }
                if matches!(event, Event::Line(_) | Event::Survey(_)) {
                    let _ = tx.try_send((id, event));
                } else if tx.send((id, event)).is_err() {
                    break;
                }
            }
        }
    });
    let error = child.stderr.take().unwrap();
    let tx = sender.clone();
    let errors = thread::spawn(move || {
        read_output(error, |line| {
            let _ = tx.try_send((id, Event::Line(line)));
        })
    });
    let mut stopping = None;
    let status = loop {
        if cancel.load(Ordering::Relaxed) && input.is_some() {
            // EOF is handled inside the elevated worker, including on GUI exit.
            drop(input.take());
            stopping = Some(Instant::now());
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if !worker_started.load(Ordering::Relaxed)
            && stopping.is_some_and(|at| at.elapsed() > Duration::from_secs(8))
        {
            // Close a pending pkexec authorization dialog as well.
            let _ = child.kill();
        }
        thread::sleep(Duration::from_millis(50));
    };
    drop(input);
    let _ = reader.join();
    let _ = errors.join();
    if !saw_finish.load(Ordering::Relaxed) {
        if cancel.load(Ordering::Relaxed) {
            let _ = sender.send((
                id,
                Event::Finished {
                    code: status.code(),
                    cancelled: true,
                },
            ));
        } else {
            return Err(format!(
                "Worker exited ({status}). Check the log and the polkit authentication agent."
            ));
        }
    }
    Ok(())
}

fn emit(event: Event) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if serde_json::to_writer(&mut out, &event).is_ok() {
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

pub fn worker_main() -> Result<(), String> {
    let mut input = BufReader::new(io::stdin());
    let mut line = String::new();
    Read::by_ref(&mut input)
        .take(65536)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let request: Request =
        serde_json::from_str(&line).map_err(|e| format!("Invalid worker request: {e}"))?;
    let spec = request.operation.spec()?;
    request.operation.validate_files()?;
    if !request.executable.is_absolute()
        || request.executable.file_name().and_then(|v| v.to_str()) != Some(spec.tool.name())
    {
        return Err("The worker requires an absolute path to the selected tool.".into());
    }
    if request.nmcli.as_ref().is_some_and(|path| {
        !path.is_absolute() || path.file_name().and_then(|name| name.to_str()) != Some("nmcli")
    }) {
        return Err("The worker requires an absolute path to nmcli.".into());
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    thread::spawn(move || {
        let mut control = String::new();
        let _ = input.read_line(&mut control);
        flag.store(true, Ordering::Relaxed);
    });
    let result = execute(request, cancelled, emit);
    if let Err(error) = &result {
        emit(Event::Error(error.clone()));
    }
    result
}

/// Each worker owns its tool's process group and can signal it even when root.
fn execute(
    request: Request,
    cancel: Arc<AtomicBool>,
    sink: impl Fn(Event) + Send + Sync + 'static,
) -> Result<(), String> {
    let spec = request.operation.spec()?;
    let sink = Arc::new(sink);
    if cancel.load(Ordering::Relaxed) {
        sink(Event::Finished {
            code: None,
            cancelled: true,
        });
        return Ok(());
    }
    sink(Event::Started);
    if let Operation::Monitor {
        interface,
        enable,
        restore,
    } = &request.operation
    {
        let host = MonitorHost {
            request: &request,
            cancel,
            sink: sink.clone(),
        };
        if *enable {
            monitor::enable(&host, interface)?;
        } else {
            monitor::disable(&host, interface, restore.as_ref())?;
        }
        sink(Event::Finished {
            code: Some(0),
            cancelled: *enable && host.cancel.load(Ordering::Relaxed),
        });
        return Ok(());
    }
    let result = run_process(
        &request.executable,
        &spec.args,
        Some(&request.operation),
        cancel,
        sink.clone(),
        None,
    )?;
    sink(Event::Finished {
        code: result.code,
        cancelled: result.cancelled,
    });
    Ok(())
}

struct MonitorHost<'a, F> {
    request: &'a Request,
    cancel: Arc<AtomicBool>,
    sink: Arc<F>,
}

impl<F: Fn(Event) + Send + Sync + 'static> monitor::Host for MonitorHost<'_, F> {
    fn interfaces(&self) -> Vec<Interface> {
        crate::model::interfaces()
    }
    fn has_nmcli(&self) -> bool {
        self.request.nmcli.is_some()
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn emit(&self, event: Event) {
        (self.sink)(event);
    }
    fn run(&self, tool: Tool, args: &[&str], cleanup: bool) -> Result<monitor::Output, String> {
        let executable = if tool == Tool::Nmcli {
            self.request.nmcli.as_ref().ok_or("nmcli is unavailable.")?
        } else {
            &self.request.executable
        };
        let spec = CommandSpec {
            tool,
            args: args.iter().map(|arg| arg.to_string()).collect(),
            privileged: true,
        };
        self.emit(Event::Line(format!("$ {}", spec.preview())));
        let cancel = if cleanup {
            Arc::new(AtomicBool::new(false))
        } else {
            self.cancel.clone()
        };
        run_process(
            executable,
            &spec.args,
            None,
            cancel,
            self.sink.clone(),
            Some(Duration::from_secs(30)),
        )
    }
}

fn run_process(
    executable: &Path,
    args: &[String],
    operation: Option<&Operation>,
    cancel: Arc<AtomicBool>,
    sink: Arc<impl Fn(Event) + Send + Sync + 'static>,
    timeout: Option<Duration>,
) -> Result<monitor::Output, String> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(monitor::Output {
            code: None,
            cancelled: true,
            stdout: String::new(),
        });
    }
    let mut child = Command::new(executable)
        .args(args)
        .env("LC_ALL", "C")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Cannot run {}: {e}", executable.display()))?;
    let group = Pid::from_raw(child.id() as i32);
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let output_sink = sink.clone();
    let reader = thread::spawn(move || {
        let mut captured = String::new();
        read_output(out, |s| {
            if captured.len() + s.len() < 65536 {
                captured.push_str(&s);
                captured.push('\n');
            }
            output_sink(Event::Line(s));
        });
        captured
    });
    let error_sink = sink.clone();
    let errors = thread::spawn(move || read_output(err, |s| error_sink(Event::Line(s))));
    let mut stopped_at = None;
    let started_at = Instant::now();
    let mut timed_out = false;
    let mut last_snapshot = Instant::now() - Duration::from_secs(2);
    let status = loop {
        timed_out |= timeout.is_some_and(|limit| started_at.elapsed() >= limit);
        if (cancel.load(Ordering::Relaxed) || timed_out) && stopped_at.is_none() {
            let _ = killpg(group, Signal::SIGINT);
            stopped_at = Some(Instant::now());
        }
        if stopped_at.is_some_and(|at| at.elapsed() > Duration::from_secs(3)) {
            let _ = killpg(group, Signal::SIGKILL);
        }
        if last_snapshot.elapsed() >= Duration::from_secs(1) {
            if let Some(operation) = operation {
                snapshot(operation, &*sink);
            }
            last_snapshot = Instant::now();
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = killpg(group, Signal::SIGKILL);
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
        thread::sleep(Duration::from_millis(50));
    };
    // A script can exit while descendants still own its pipes.
    let _ = killpg(group, Signal::SIGKILL);
    let stdout = reader.join().unwrap_or_default();
    let _ = errors.join();
    if let Some(operation) = operation {
        snapshot(operation, &*sink);
    }
    if timed_out {
        return Err(format!(
            "{} timed out; its process group was stopped.",
            executable.display()
        ));
    }
    Ok(monitor::Output {
        code: status.code(),
        cancelled: stopped_at.is_some(),
        stdout,
    })
}

fn snapshot(operation: &Operation, sink: &impl Fn(Event)) {
    if let Some(path) = operation.csv_path()
        && let Ok(file) = fs::File::open(path)
    {
        let mut bytes = Vec::new();
        if file.take(4 * 1024 * 1024).read_to_end(&mut bytes).is_ok() {
            let survey = parse_survey(&String::from_utf8_lossy(&bytes));
            // Airodump truncates the snapshot before rewriting it.
            if !survey.networks.is_empty() {
                sink(Event::Survey(survey));
            }
        }
    }
}

fn read_output(reader: impl Read, mut sink: impl FnMut(String)) {
    let mut reader = BufReader::new(reader);
    let mut buffer = [0u8; 4096];
    let mut line = Vec::new();
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for &byte in &buffer[..n] {
                    if byte == b'\n' || byte == b'\r' || line.len() >= 8192 {
                        let text = clean_terminal(&String::from_utf8_lossy(&line));
                        if !text.trim().is_empty() {
                            sink(text);
                        }
                        line.clear();
                    }
                    if byte != b'\n' && byte != b'\r' {
                        line.push(byte);
                    }
                }
            }
        }
    }
    if !line.is_empty() {
        sink(clean_terminal(&String::from_utf8_lossy(&line)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::Mutex};

    #[test]
    fn cancellation_reaps_process_group_and_handles_carriage_returns() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-tool");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf 'first\\rsecond\\n'\nsleep 30 &\nwait\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let received = events.clone();
        let start = Instant::now();
        let handle = thread::spawn(move || {
            execute(
                Request {
                    executable: fake,
                    nmcli: None,
                    operation: Operation::Check,
                },
                flag,
                move |e| received.lock().unwrap().push(e),
            )
        });
        while !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Line(s) if s == "second"))
        {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(10));
        }
        cancel.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
        assert!(start.elapsed() < Duration::from_secs(6));
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Line(s) if s == "first"))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Finished {
                cancelled: true,
                ..
            }
        )));
    }
}
