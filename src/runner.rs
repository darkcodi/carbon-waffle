use crate::{
    command::{CommandSpec, Operation, Tool, find_tool},
    model::{Interface, Survey, clean_terminal, parse_survey},
    monitor::{self, NetworkRestore},
};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};

use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
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

mod inspection;
mod session;
pub use inspection::Inspection;
pub use session::{Authorization, session_worker_main};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Started,
    Line(String),
    Survey(Survey),
    Inspection(Inspection),
    Candidates { generated: u64, total: String },
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

impl Request {
    fn validate(&self) -> Result<(), String> {
        let spec = self.operation.spec()?;
        self.operation.validate_files()?;
        if !self.executable.is_absolute()
            || self.executable.file_name().and_then(|v| v.to_str()) != Some(spec.tool.name())
        {
            return Err("The worker requires an absolute path to the selected tool.".into());
        }
        if self.nmcli.as_ref().is_some_and(|path| {
            !path.is_absolute() || path.file_name().and_then(|name| name.to_str()) != Some("nmcli")
        }) {
            return Err("The worker requires an absolute path to nmcli.".into());
        }
        Ok(())
    }
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
    session: Option<session::Session>,
    authorization_updates: iced::futures::channel::mpsc::UnboundedSender<Authorization>,
    pub authorization: Option<iced::futures::channel::mpsc::UnboundedReceiver<Authorization>>,
}

impl Runner {
    pub fn new() -> Self {
        let (sender, events) = mpsc::sync_channel(512);
        let (updates, authorization) = iced::futures::channel::mpsc::unbounded();
        Self {
            events,
            sender,
            session: None,
            authorization_updates: updates,
            authorization: Some(authorization),
        }
    }

    pub fn authorize(&mut self) -> Result<(), String> {
        if self
            .session
            .as_ref()
            .is_some_and(|session| !session.is_closed())
        {
            return Err("Session authorization is already active.".into());
        }
        self.session = Some(session::Session::new(
            self.sender.clone(),
            self.authorization_updates.clone(),
        )?);
        Ok(())
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
                if matches!(operation, Operation::Inspect { .. }) && !flag.load(Ordering::Relaxed) {
                    let _ = sender.send((id, Event::Inspection(Inspection::Found)));
                }
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
            let request = Request {
                executable,
                nmcli: if matches!(operation, Operation::Monitor { .. }) {
                    find_tool("nmcli")
                } else {
                    None
                },
                operation: operation.clone(),
            };
            if spec.privileged {
                self.session
                    .as_ref()
                    .ok_or("The privileged session is unavailable. Open Elevate to authorize it.")?
                    .start(id, request, flag)?;
            } else {
                let helper = std::env::current_exe().map_err(|e| e.to_string())?;
                thread::spawn(move || {
                    if let Err(error) = run_helper(id, request, helper, flag, &sender) {
                        let _ = sender.send((id, Event::Error(error)));
                    }
                });
            }
        }
        Ok(Job {
            id,
            operation,
            stopping: false,
            started: false,
            cancel,
        })
    }

    pub fn shutdown(&self) -> bool {
        self.session.as_ref().is_none_or(session::Session::close)
    }
}

fn run_helper(
    id: u64,
    request: Request,
    helper: PathBuf,
    cancel: Arc<AtomicBool>,
    sender: &SyncSender<(u64, Event)>,
) -> Result<(), String> {
    let mut child = Command::new(helper)
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
                // Only Activity text is disposable when its queue is full.
                if matches!(event, Event::Line(_)) {
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
            // Stop a worker that never reached command execution.
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
                "Worker exited ({status}). Check the command output."
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
    request.validate()?;
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
    if let Operation::Inspect { bssid, .. } = &request.operation {
        return inspection::run(&request.executable, &spec.args, bssid, cancel, sink);
    }
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
    let candidates = if let Some(Operation::Crack {
        pattern: Some(pattern),
        wordlist,
        ..
    }) = operation
    {
        sink(Event::Line(format!("Pattern: {pattern}")));
        let parsed = crate::pattern::Pattern::parse(pattern)?;
        if parsed.needs_words() {
            sink(Event::Line(format!(
                "Source wordlist: {}",
                wordlist.display()
            )));
        }
        sink(Event::Line("Preparing candidates for stdin…".into()));
        let prepared = parsed.prepare(wordlist, &cancel);
        if cancel.load(Ordering::Relaxed) {
            return Ok(monitor::Output {
                code: None,
                cancelled: true,
                stdout: String::new(),
            });
        }
        let prepared = prepared?;
        sink(Event::Line(format!(
            "Streaming {} candidate combinations (8–63 bytes); no combined wordlist is written.",
            crate::pattern::format_count(prepared.total)
        )));
        sink(Event::Candidates {
            generated: 0,
            total: crate::pattern::format_count(prepared.total),
        });
        Some(prepared)
    } else {
        None
    };
    let mut child = Command::new(executable)
        .args(args)
        .env("LC_ALL", "C")
        .env("TERM", "dumb")
        .stdin(if candidates.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Cannot run {}: {e}", executable.display()))?;
    let group = Pid::from_raw(child.id() as i32);
    let mut feeder = candidates.map(|candidates| {
        let input = child.stdin.take().unwrap();
        let flag = cancel.clone();
        let progress = sink.clone();
        thread::spawn(move || {
            let total = crate::pattern::format_count(candidates.total);
            let mut output = BufWriter::with_capacity(64 * 1024, input);
            let mut generated = 0u64;
            let mut last_update = Instant::now();
            let result = candidates
                .generate(&flag, |candidate| {
                    output.write_all(candidate)?;
                    output.write_all(b"\n")?;
                    generated = generated.saturating_add(1);
                    if last_update.elapsed() >= Duration::from_secs(1) {
                        progress(Event::Candidates {
                            generated,
                            total: total.clone(),
                        });
                        last_update = Instant::now();
                    }
                    Ok(true)
                })
                .and_then(|()| output.flush());
            progress(Event::Candidates { generated, total });
            result
        })
    });
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
    let snapshot_interval = Duration::from_millis(100);
    let mut last_snapshot = Instant::now() - snapshot_interval;
    let mut last_survey = None;
    let status = loop {
        timed_out |= timeout.is_some_and(|limit| started_at.elapsed() >= limit);
        if (cancel.load(Ordering::Relaxed) || timed_out) && stopped_at.is_none() {
            let _ = killpg(group, Signal::SIGINT);
            stopped_at = Some(Instant::now());
        }
        if stopped_at.is_some_and(|at| at.elapsed() > Duration::from_secs(3)) {
            let _ = killpg(group, Signal::SIGKILL);
        }
        if last_snapshot.elapsed() >= snapshot_interval {
            if let Some(operation) = operation {
                snapshot(operation, &mut last_survey, &*sink);
            }
            last_snapshot = Instant::now();
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                if feeder.is_some() {
                    cancel.store(true, Ordering::Relaxed);
                }
                let _ = killpg(group, Signal::SIGKILL);
                let _ = child.wait();
                if let Some(feeder) = feeder.take() {
                    let _ = feeder.join();
                }
                return Err(error.to_string());
            }
        }
        thread::sleep(Duration::from_millis(50));
    };
    // A script can exit while descendants still own its pipes.
    let _ = killpg(group, Signal::SIGKILL);
    if let Some(feeder) = feeder {
        cancel.store(true, Ordering::Relaxed);
        if let Err(error) = feeder
            .join()
            .map_err(|_| "Pattern generator stopped unexpectedly.")?
            && error.kind() != io::ErrorKind::BrokenPipe
            && stopped_at.is_none()
        {
            return Err(format!("Cannot send generated candidates: {error}"));
        }
    }
    let stdout = reader.join().unwrap_or_default();
    let _ = errors.join();
    if let Some(operation) = operation {
        snapshot(operation, &mut last_survey, &*sink);
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

fn snapshot(operation: &Operation, last_survey: &mut Option<Survey>, sink: &impl Fn(Event)) {
    if let Some(path) = operation.csv_path()
        && let Ok(file) = fs::File::open(path)
    {
        let mut bytes = Vec::new();
        if file.take(4 * 1024 * 1024).read_to_end(&mut bytes).is_ok() {
            let survey = parse_survey(&String::from_utf8_lossy(&bytes));
            // Ignore empty rewrites and avoid redrawing unchanged results.
            if !survey.networks.is_empty() && last_survey.as_ref() != Some(&survey) {
                *last_survey = Some(survey.clone());
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

    const SCAN_CSV: &str = "02:11:22:33:44:55, first, last, 6, 54, WPA2, CCMP, PSK, -40, 20, 0, 0.0.0.0, 7, Lab One, \n";

    fn pattern_operation(wordlist: PathBuf, source: &str) -> Operation {
        Operation::Crack {
            engine: crate::command::Engine::Aircrack,
            input: "/unused-synthetic.cap".into(),
            wordlist,
            pattern: Some(source.into()),
            bssid: "02:00:00:00:00:01".into(),
            output: "/unused-result.txt".into(),
        }
    }

    #[test]
    fn generated_candidates_reach_stdin_with_eof_and_progress() {
        let directory = tempfile::tempdir().unwrap();
        let words = directory.path().join("words.txt");
        fs::write(&words, b"ab\ncd\n").unwrap();
        let operation = pattern_operation(words, "{word}{5}");
        let events = Arc::new(Mutex::new(Vec::new()));
        let received = events.clone();
        let result = run_process(
            Path::new("/bin/sh"),
            &["-c".into(), "exec cat".into()],
            Some(&operation),
            Arc::new(AtomicBool::new(false)),
            Arc::new(move |event| received.lock().unwrap().push(event)),
            Some(Duration::from_secs(5)),
        )
        .unwrap();
        assert_eq!(result.code, Some(0));
        assert!(!result.cancelled);
        let lines: Vec<_> = result.stdout.lines().collect();
        assert_eq!(lines.len(), 32);
        assert_eq!(lines[0], "ababababab");
        assert_eq!(lines[31], "cdcdcdcdcd");
        assert!(events.lock().unwrap().iter().any(
            |event| matches!(event, Event::Candidates { generated: 32, total } if total == "32")
        ));
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            1,
            "no expanded dictionary should be written"
        );
    }

    #[test]
    fn early_tool_exit_stops_a_large_candidate_stream() {
        let operation = pattern_operation(PathBuf::new(), "[a-z]{20}");
        let start = Instant::now();
        let result = run_process(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "IFS= read -r line; printf '%s\\n' \"$line\"".into(),
            ],
            Some(&operation),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {}),
            Some(Duration::from_secs(5)),
        )
        .unwrap();
        assert_eq!(result.code, Some(0));
        assert!(!result.cancelled);
        assert_eq!(result.stdout.trim_end(), "a".repeat(20));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn cancelling_recovery_unblocks_a_full_stdin_pipe() {
        let operation = pattern_operation(PathBuf::new(), "[a-z]{20}");
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (sender, received) = mpsc::channel();
        let handle = thread::spawn(move || {
            run_process(
                Path::new("/bin/sh"),
                &["-c".into(), "printf 'ready\\n'; sleep 30".into()],
                Some(&operation),
                flag,
                Arc::new(move |event| {
                    let _ = sender.send(event);
                }),
                Some(Duration::from_secs(5)),
            )
        });
        loop {
            if matches!(received.recv_timeout(Duration::from_secs(5)).unwrap(), Event::Line(line) if line == "ready")
            {
                break;
            }
        }
        // The child never reads stdin, so the producer fills the pipe and blocks.
        thread::sleep(Duration::from_millis(150));
        cancel.store(true, Ordering::Relaxed);
        let result = handle.join().unwrap().unwrap();
        assert!(result.cancelled);
    }

    #[test]
    fn snapshots_publish_changes_and_preserve_results_during_empty_rewrites() {
        let dir = tempfile::tempdir().unwrap();
        let operation = Operation::Scan {
            interface: "testmon".into(),
            prefix: dir.path().join("survey"),
        };
        let path = operation.csv_path().unwrap();
        let received = Mutex::new(Vec::new());
        let sink = |event| {
            if let Event::Survey(survey) = event {
                received.lock().unwrap().push(survey);
            }
        };
        let mut last_survey = None;
        snapshot(&operation, &mut last_survey, &sink);
        fs::write(&path, SCAN_CSV.trim_end_matches('\n')).unwrap();
        snapshot(&operation, &mut last_survey, &sink);
        assert!(received.lock().unwrap().is_empty());

        fs::write(&path, SCAN_CSV).unwrap();
        snapshot(&operation, &mut last_survey, &sink);
        snapshot(&operation, &mut last_survey, &sink);
        fs::write(&path, "").unwrap();
        snapshot(&operation, &mut last_survey, &sink);
        fs::write(&path, SCAN_CSV).unwrap();
        snapshot(&operation, &mut last_survey, &sink);
        assert_eq!(received.lock().unwrap().len(), 1);

        fs::write(&path, SCAN_CSV.replace(", 20,", ", 21,")).unwrap();
        snapshot(&operation, &mut last_survey, &sink);
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0].networks[0].ssid, "Lab One");
        assert_eq!(received[1].networks[0].beacons, 21);
    }

    #[test]
    fn running_scan_publishes_new_csv_without_waiting_a_second() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-tool");
        fs::write(&fake, "#!/bin/sh\nprintf 'ready\\n'\nsleep 30\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let operation = Operation::Scan {
            interface: "testmon".into(),
            prefix: dir.path().join("survey"),
        };
        let path = operation.csv_path().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (sender, received) = mpsc::channel();
        let handle = thread::spawn(move || {
            run_process(
                &fake,
                &[],
                Some(&operation),
                flag,
                Arc::new(move |event| {
                    let _ = sender.send(event);
                }),
                None,
            )
        });
        let ready = received.recv_timeout(Duration::from_secs(5));
        // Publish after the initial, empty snapshot check has already happened.
        thread::sleep(Duration::from_millis(150));
        fs::write(&path, SCAN_CSV).unwrap();
        let update = received.recv_timeout(Duration::from_millis(650));
        cancel.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
        assert!(matches!(ready, Ok(Event::Line(line)) if line == "ready"));
        assert!(
            matches!(update, Ok(Event::Survey(survey)) if survey.networks[0].ssid == "Lab One"),
            "new networks must arrive promptly while the scan is still running"
        );
    }

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
