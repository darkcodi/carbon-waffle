//! One authenticated helper per GUI session, connected only through its pipes.
use super::{Event, Request, execute, find_tool, read_output};
use iced::futures::channel::mpsc as asynchronous;
use nix::unistd::geteuid;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authorization {
    Idle,
    Pending,
    Ready,
    Failed(String),
}

#[derive(Serialize, Deserialize)]
enum Control {
    Start { id: u64, request: Request },
    Cancel { id: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
enum Reply {
    Ready,
    Job { id: u64, event: Event },
}

struct Submission {
    id: u64,
    request: Request,
    cancel: Arc<AtomicBool>,
}

pub(super) struct Session {
    submissions: Sender<Submission>,
    ready: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Session {
    pub fn new(
        events: SyncSender<(u64, Event)>,
        updates: asynchronous::UnboundedSender<Authorization>,
    ) -> Result<Self, String> {
        let helper = std::env::current_exe().map_err(|error| error.to_string())?;
        let pkexec = if geteuid().is_root() {
            None
        } else {
            Some(find_tool("pkexec").ok_or(
                "Install polkit (pkexec) and a desktop authentication agent to authorize this session.",
            )?)
        };
        Ok(Self::launch(helper, pkexec, events, updates))
    }

    fn launch(
        helper: PathBuf,
        pkexec: Option<PathBuf>,
        events: SyncSender<(u64, Event)>,
        updates: asynchronous::UnboundedSender<Authorization>,
    ) -> Self {
        let (submissions, incoming) = mpsc::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let connection = Connection {
            incoming,
            ready: ready.clone(),
            shutdown: shutdown.clone(),
            done: done.clone(),
            events,
            updates,
        };
        thread::spawn(move || connection.run(helper, pkexec));
        Self {
            submissions,
            ready,
            shutdown,
            done,
        }
    }

    pub fn start(&self, id: u64, request: Request, cancel: Arc<AtomicBool>) -> Result<(), String> {
        if !self.ready.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
            return Err("The privileged session is not authorized or has ended. Open Elevate to authorize it.".into());
        }
        self.submissions
            .send(Submission {
                id,
                request,
                cancel,
            })
            .map_err(|_| "The privileged session has ended. Open Elevate to authorize it.".into())
    }

    pub fn close(&self) -> bool {
        self.shutdown.store(true, Ordering::Release);
        self.is_closed()
    }

    pub fn is_closed(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

struct Connection {
    incoming: Receiver<Submission>,
    ready: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    events: SyncSender<(u64, Event)>,
    updates: asynchronous::UnboundedSender<Authorization>,
}

impl Connection {
    fn run(self, helper: PathBuf, pkexec: Option<PathBuf>) {
        let mut active = HashMap::new();
        let result = self.communicate(helper, pkexec, &mut active);
        self.ready.store(false, Ordering::Release);
        let error = result.err().unwrap_or_else(|| {
            "The privileged session ended. Open Elevate to authorize it.".into()
        });
        // Include submissions racing with helper failure, so no GUI job is left
        // waiting forever for a completion event.
        for pending in self.incoming.try_iter() {
            active.insert(pending.id, (pending.cancel, false));
        }
        for id in active.keys() {
            let _ = self.events.send((*id, Event::Error(error.clone())));
        }
        // The helper and its readers have exited. Make retry available before
        // notifying the UI, so an immediate click can start a fresh session.
        self.done.store(true, Ordering::Release);
        if !self.shutdown.load(Ordering::Acquire) {
            let _ = self.updates.unbounded_send(Authorization::Failed(error));
        }
    }

    fn communicate(
        &self,
        helper: PathBuf,
        pkexec: Option<PathBuf>,
        active: &mut HashMap<u64, (Arc<AtomicBool>, bool)>,
    ) -> Result<(), String> {
        let mut command = if let Some(pkexec) = pkexec {
            let mut command = Command::new(pkexec);
            command.arg(helper);
            command
        } else {
            Command::new(helper)
        };
        let mut child = command
            .arg("--session-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start authorization: {error}"))?;
        let mut input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let errors = child.stderr.take().unwrap();
        let ready = self.ready.clone();
        let events = self.events.clone();
        let updates = self.updates.clone();
        let (completed, completions) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                match serde_json::from_str::<Reply>(&line) {
                    Ok(Reply::Ready) => {
                        ready.store(true, Ordering::Release);
                        let _ = updates.unbounded_send(Authorization::Ready);
                    }
                    Ok(Reply::Job { id, event }) => {
                        if matches!(event, Event::Finished { .. } | Event::Error(_)) {
                            let _ = completed.send(id);
                        }
                        if matches!(event, Event::Line(_) | Event::Survey(_)) {
                            let _ = events.try_send((id, event));
                        } else if events.send((id, event)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut diagnostic = String::new();
            read_output(errors, |line| {
                if diagnostic.len() + line.len() < 4096 {
                    diagnostic.push_str(&line);
                    diagnostic.push('\n');
                }
            });
            diagnostic
        });
        let mut closing_at = None;
        let mut failure = None;
        let status = loop {
            for id in completions.try_iter() {
                active.remove(&id);
            }
            if self.shutdown.load(Ordering::Acquire) || failure.is_some() || reader.is_finished() {
                if input.take().is_some() {
                    closing_at = Some(Instant::now());
                }
            } else if let Some(pipe) = input.as_mut() {
                for submission in self.incoming.try_iter() {
                    let Submission {
                        id,
                        request,
                        cancel,
                    } = submission;
                    active.insert(id, (cancel, false));
                    if let Err(error) = write_packet(pipe, &Control::Start { id, request }) {
                        failure = Some(format!("Cannot contact the privileged session: {error}"));
                        break;
                    }
                }
                for (id, (cancel, sent)) in active.iter_mut() {
                    if cancel.load(Ordering::Relaxed) && !*sent {
                        if let Err(error) = write_packet(pipe, &Control::Cancel { id: *id }) {
                            failure = Some(format!("Cannot stop the privileged job: {error}"));
                            break;
                        }
                        *sent = true;
                    }
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(error) => {
                    failure = Some(error.to_string());
                    drop(input.take());
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
            if !self.ready.load(Ordering::Acquire)
                && closing_at.is_some_and(|at| at.elapsed() > Duration::from_secs(2))
            {
                // Dismiss authorization if the GUI closes before a helper starts.
                let _ = child.kill();
            }
            thread::sleep(Duration::from_millis(20));
        };
        drop(input);
        let _ = reader.join();
        let diagnostic = stderr.join().unwrap_or_default();
        for id in completions.try_iter() {
            active.remove(&id);
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if self.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        if status.is_some_and(|status| status.code() == Some(126)) {
            return Err("Authorization was cancelled. Try again in Elevate.".into());
        }
        Err(format!(
            "The privileged session exited ({}). {} Open Elevate to authorize a new session.",
            status.map_or_else(|| "unknown status".into(), |status| status.to_string()),
            diagnostic.trim()
        ))
    }
}

fn write_packet(writer: &mut impl Write, packet: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, packet)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn read_control(reader: &mut impl BufRead) -> Result<Option<Control>, String> {
    let mut line = String::new();
    let count = Read::by_ref(reader)
        .take(65537)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if count == 0 {
        return Ok(None);
    }
    if count > 65536 || !line.ends_with('\n') {
        return Err("The session request is too large or incomplete.".into());
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|error| format!("Invalid session request: {error}"))
}

struct WorkerJob {
    cancel: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

fn serve(
    reader: &mut impl BufRead,
    sink: impl Fn(Reply) + Send + Sync + 'static,
) -> Result<(), String> {
    let sink = Arc::new(sink);
    let mut jobs: HashMap<u64, WorkerJob> = HashMap::new();
    sink(Reply::Ready);
    let result = (|| {
        while let Some(control) = read_control(reader)? {
            jobs.retain(|_, job| !job.thread.is_finished());
            match control {
                Control::Cancel { id } => {
                    if let Some(job) = jobs.get(&id) {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                }
                Control::Start { id, request } => {
                    if jobs.contains_key(&id) {
                        return Err("Duplicate session job ID.".into());
                    }
                    if let Err(error) = request.validate() {
                        sink(Reply::Job {
                            id,
                            event: Event::Error(error),
                        });
                        continue;
                    }
                    if jobs.len() >= 16 {
                        sink(Reply::Job {
                            id,
                            event: Event::Error("Too many active session jobs.".into()),
                        });
                        continue;
                    }
                    let cancel = Arc::new(AtomicBool::new(false));
                    let flag = cancel.clone();
                    let output = sink.clone();
                    let worker = thread::spawn(move || {
                        let events = output.clone();
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            execute(request, flag, move |event| events(Reply::Job { id, event }))
                        }))
                        .unwrap_or_else(|_| Err("The session job panicked.".into()));
                        if let Err(error) = result {
                            output(Reply::Job {
                                id,
                                event: Event::Error(error),
                            });
                        }
                    });
                    jobs.insert(
                        id,
                        WorkerJob {
                            cancel,
                            thread: worker,
                        },
                    );
                }
            }
        }
        Ok(())
    })();
    // EOF revokes this session: stop tool process groups and allow monitor setup
    // rollback to finish before the elevated helper exits.
    for job in jobs.values() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    for (_, job) in jobs {
        let _ = job.thread.join();
    }
    result
}

pub fn session_worker_main() -> Result<(), String> {
    if !geteuid().is_root() {
        return Err("The session helper must be started through desktop authorization.".into());
    }
    serve(&mut BufReader::new(io::stdin()), |reply| {
        let _ = write_packet(&mut io::stdout().lock(), &reply);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Operation;
    use std::{
        fs,
        os::unix::{fs::PermissionsExt, net::UnixStream},
    };

    fn script(path: &std::path::Path, body: &str) -> PathBuf {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        path.to_owned()
    }

    fn request(executable: PathBuf) -> Request {
        Request {
            executable,
            nmcli: None,
            operation: Operation::Check,
        }
    }

    fn wait_for<T>(mut condition: impl FnMut() -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(result) = condition() {
                return result;
            }
            assert!(
                start.elapsed() < Duration::from_secs(6),
                "session check timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn next_reply(events: &Receiver<Reply>, condition: impl Fn(&Reply) -> bool) -> Reply {
        let start = Instant::now();
        loop {
            let reply = events.recv_timeout(Duration::from_secs(5)).unwrap();
            if condition(&reply) {
                return reply;
            }
            assert!(start.elapsed() < Duration::from_secs(6));
        }
    }

    #[test]
    fn one_authorization_serves_multiple_jobs_and_closes_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let pkexec = script(
            &dir.path().join("pkexec"),
            r#"
base=${0%/*}
printf 'authorized\n' >> "$base/calls"
printf '"Ready"\n'
id=0
while IFS= read -r request; do
    id=$((id + 1))
    printf '{"Job":{"id":%s,"event":{"Finished":{"code":0,"cancelled":false}}}}\n' "$id"
done
printf 'closed\n' > "$base/closed"
"#,
        );
        let (events, received) = mpsc::sync_channel(32);
        let (updates, mut authorization) = asynchronous::unbounded();
        let session = Session::launch(
            PathBuf::from("/unused/helper"),
            Some(pkexec),
            events,
            updates,
        );
        assert_eq!(
            wait_for(|| authorization.try_recv().ok()),
            Authorization::Ready
        );
        for id in 1..=3 {
            session
                .start(
                    id,
                    request(PathBuf::from("/unused/airmon-ng")),
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap();
            let (job, event) = received.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(job, id);
            assert!(matches!(
                event,
                Event::Finished {
                    code: Some(0),
                    cancelled: false
                }
            ));
        }
        wait_for(|| session.close().then_some(()));
        assert_eq!(
            fs::read_to_string(dir.path().join("calls")).unwrap(),
            "authorized\n"
        );
        assert!(dir.path().join("closed").exists());
    }

    #[test]
    fn cancelled_authorization_does_not_retry_on_job_submission() {
        let dir = tempfile::tempdir().unwrap();
        let pkexec = script(
            &dir.path().join("pkexec"),
            "printf 'called\\n' >> \"${0%/*}/calls\"\nexit 126",
        );
        let (events, _received) = mpsc::sync_channel(32);
        let (updates, mut authorization) = asynchronous::unbounded();
        let session = Session::launch(
            PathBuf::from("/unused/helper"),
            Some(pkexec),
            events,
            updates,
        );
        assert!(
            matches!(wait_for(|| authorization.try_recv().ok()), Authorization::Failed(error) if error.contains("cancelled"))
        );
        assert!(session.is_closed(), "failure must immediately allow retry");
        assert!(
            session
                .start(
                    1,
                    request(PathBuf::from("/unused/airmon-ng")),
                    Arc::new(AtomicBool::new(false))
                )
                .is_err()
        );
        wait_for(|| session.close().then_some(()));
        assert_eq!(
            fs::read_to_string(dir.path().join("calls")).unwrap(),
            "called\n"
        );
    }

    #[test]
    fn helper_failure_finishes_active_job_and_revokes_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let pkexec = script(
            &dir.path().join("pkexec"),
            "printf '\"Ready\"\\n'\nIFS= read -r request\nexit 7",
        );
        let (events, received) = mpsc::sync_channel(32);
        let (updates, mut authorization) = asynchronous::unbounded();
        let session = Session::launch(
            PathBuf::from("/unused/helper"),
            Some(pkexec),
            events,
            updates,
        );
        assert_eq!(
            wait_for(|| authorization.try_recv().ok()),
            Authorization::Ready
        );
        session
            .start(
                1,
                request(PathBuf::from("/unused/airmon-ng")),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        let (id, event) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(id, 1);
        assert!(matches!(event, Event::Error(_)));
        assert!(matches!(
            wait_for(|| authorization.try_recv().ok()),
            Authorization::Failed(_)
        ));
        assert!(session.is_closed(), "failure must immediately allow retry");
        assert!(
            session
                .start(
                    2,
                    request(PathBuf::from("/unused/airmon-ng")),
                    Arc::new(AtomicBool::new(false))
                )
                .is_err()
        );
        wait_for(|| session.close().then_some(()));
    }

    #[test]
    fn closing_during_authorization_stops_the_pending_process() {
        let dir = tempfile::tempdir().unwrap();
        let pkexec = script(
            &dir.path().join("pkexec"),
            "printf '%s\\n' \"$$\" > \"${0%/*}/pid\"\nexec sleep 30",
        );
        let (events, _received) = mpsc::sync_channel(32);
        let (updates, _authorization) = asynchronous::unbounded();
        let session = Session::launch(
            PathBuf::from("/unused/helper"),
            Some(pkexec),
            events,
            updates,
        );
        let pid: i32 = wait_for(|| {
            fs::read_to_string(dir.path().join("pid"))
                .ok()
                .and_then(|v| v.trim().parse().ok())
        });
        wait_for(|| session.close().then_some(()));
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
    }

    #[test]
    fn server_supports_concurrency_cancellation_reuse_and_disconnect_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let long_dir = dir.path().join("long");
        let quick_dir = dir.path().join("quick");
        fs::create_dir(&long_dir).unwrap();
        fs::create_dir(&quick_dir).unwrap();
        let long = script(
            &long_dir.join("airmon-ng"),
            "printf 'PID:%s\\n' \"$$\"\nsleep 30 &\nwait",
        );
        let quick = script(&quick_dir.join("airmon-ng"), "printf 'done\\n'");
        let (mut client, server) = UnixStream::pair().unwrap();
        let (events, received) = mpsc::channel();
        let server = thread::spawn(move || {
            serve(&mut BufReader::new(server), move |reply| {
                let _ = events.send(reply);
            })
        });
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            Reply::Ready
        ));
        // Invalid commands do not execute and do not poison subsequent requests.
        write_packet(
            &mut client,
            &Control::Start {
                id: 10,
                request: request(PathBuf::from("/bin/sh")),
            },
        )
        .unwrap();
        next_reply(&received, |r| {
            matches!(
                r,
                Reply::Job {
                    id: 10,
                    event: Event::Error(_)
                }
            )
        });
        write_packet(
            &mut client,
            &Control::Start {
                id: 1,
                request: request(long.clone()),
            },
        )
        .unwrap();
        next_reply(
            &received,
            |r| matches!(r, Reply::Job { id: 1, event: Event::Line(line) } if line.starts_with("PID:")),
        );
        write_packet(
            &mut client,
            &Control::Start {
                id: 2,
                request: request(quick.clone()),
            },
        )
        .unwrap();
        next_reply(&received, |r| {
            matches!(
                r,
                Reply::Job {
                    id: 2,
                    event: Event::Finished {
                        code: Some(0),
                        cancelled: false
                    }
                }
            )
        });
        write_packet(&mut client, &Control::Cancel { id: 1 }).unwrap();
        next_reply(&received, |r| {
            matches!(
                r,
                Reply::Job {
                    id: 1,
                    event: Event::Finished {
                        cancelled: true,
                        ..
                    }
                }
            )
        });
        write_packet(
            &mut client,
            &Control::Start {
                id: 3,
                request: request(quick),
            },
        )
        .unwrap();
        next_reply(&received, |r| {
            matches!(
                r,
                Reply::Job {
                    id: 3,
                    event: Event::Finished {
                        code: Some(0),
                        cancelled: false
                    }
                }
            )
        });
        write_packet(
            &mut client,
            &Control::Start {
                id: 4,
                request: request(long),
            },
        )
        .unwrap();
        let reply = next_reply(
            &received,
            |r| matches!(r, Reply::Job { id: 4, event: Event::Line(line) } if line.starts_with("PID:")),
        );
        let Reply::Job {
            event: Event::Line(line),
            ..
        } = reply
        else {
            unreachable!()
        };
        let pid: i32 = line.strip_prefix("PID:").unwrap().parse().unwrap();
        drop(client);
        wait_for(|| server.is_finished().then_some(()));
        server.join().unwrap().unwrap();
        next_reply(&received, |r| {
            matches!(
                r,
                Reply::Job {
                    id: 4,
                    event: Event::Finished {
                        cancelled: true,
                        ..
                    }
                }
            )
        });
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
    }
}
