use crate::{
    command::{Engine, Operation, Tool, find_tool},
    model::{self, Interface, Network, Survey, absolute_path},
    monitor::NetworkRestore,
    pattern::{self, Pattern},
    runner::{Authorization, Event, Inspection, Job, Runner},
};
use iced::{Subscription, Task};
use std::{
    collections::{HashSet, VecDeque},
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod appearance;
mod view;

#[cfg(test)]
mod performance;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Page {
    Elevate,
    Monitoring,
    Discover,
    Capture,
    Recover,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::Elevate,
        Self::Monitoring,
        Self::Discover,
        Self::Capture,
        Self::Recover,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Elevate => "Elevate",
            Self::Monitoring => "Monitoring",
            Self::Discover => "Discover",
            Self::Capture => "Capture",
            Self::Recover => "Recover",
        }
    }

    fn next(self) -> Option<Self> {
        Self::ALL.get(self as usize + 1).copied()
    }

    fn previous(self) -> Option<Self> {
        (self as usize)
            .checked_sub(1)
            .and_then(|index| Self::ALL.get(index).copied())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Panel {
    Activity,
    Dependencies,
    HiddenNetworks,
    Reconnect,
    CaptureFile,
    Conversion,
    Session,
    PatternHelp,
    RecoverySettings,
}

#[derive(Default)]
struct Panels {
    activity: bool,
    dependencies: bool,
    hidden_networks: bool,
    reconnect: bool,
    capture_file: bool,
    conversion: bool,
    session: bool,
    pattern_help: bool,
    recovery_settings: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMode {
    Dictionary,
    Pattern,
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    Monitor,
    Restore,
    Scan,
    Capture,
    Deauth,
    Inspect,
    Convert,
    Crack,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Elevate,
    Authorization(Authorization),
    Page(Page),
    Back,
    Next,
    Interface(Interface),
    Refresh,
    Select(String),
    ToggleNetworkGroup(String),
    Filter(String),
    Station(String),
    Count(String),
    CapturePath(String),
    HashPath(String),
    Wordlist(String),
    RecoveryMode(RecoveryMode),
    Pattern(String),
    PreviewPattern,
    PatternPreview(u64, Result<pattern::Preview, String>),
    Engine(Engine),
    Run(Action),
    StopAndCheck,
    Stop(u64),
    StopAll,
    ClearActivity,
    CopyActivity(u64),
    Toggle(Panel),
    Close,
}

struct CommandActivity {
    id: u64,
    label: &'static str,
    command: String,
    output: VecDeque<String>,
    outcome: String,
}

impl CommandActivity {
    fn push(&mut self, line: String) {
        self.output.push_back(line);
        while self.output.len() > 400 {
            self.output.pop_front();
        }
    }

    fn transcript(&self) -> String {
        std::iter::once(format!("$ {}", self.command))
            .chain(self.output.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub struct App {
    page: Page,
    demo: bool,
    interfaces: Vec<Interface>,
    interface: Option<Interface>,
    owned_monitor: Option<Interface>,
    network_restore: Option<NetworkRestore>,
    survey: Survey,
    target: Option<Network>,
    filter: String,
    expanded_networks: HashSet<String>,
    station: String,
    count: String,
    capture_path: String,
    capture_available: bool,
    capture_stamp: Option<(u64, SystemTime)>,
    capture_check: Option<Inspection>,
    check_after_capture: bool,
    capture_started: Option<Instant>,
    hash_path: String,
    wordlist: String,
    recovery_mode: RecoveryMode,
    pattern: String,
    pattern_check: Result<Pattern, String>,
    pattern_preview: Option<Result<pattern::Preview, String>>,
    preview_revision: u64,
    preview_cancel: Option<Arc<AtomicBool>>,
    candidate_progress: Option<(u64, String)>,
    engine: Engine,
    tools: Vec<(Tool, bool)>,
    runner: Runner,
    authorization: Authorization,
    jobs: Vec<Job>,
    pending: Option<Operation>,
    next_id: u64,
    session: PathBuf,
    activity: Vec<CommandActivity>,
    copied_activity: Option<u64>,
    spinner_frame: u8,
    status: String,
    status_error: bool,
    panels: Panels,
    closing: bool,
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        let demo = std::env::args().any(|arg| arg == "--demo");
        let mut app = Self::new(demo);
        // Lifecycle notifications wake the UI directly; no idle polling is
        // needed to detect authorization or an unexpectedly disconnected helper.
        let updates = app.runner.authorization.take().unwrap();
        (app, Task::run(updates, Message::Authorization))
    }

    fn new(demo: bool) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let session = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("captures")
            .join(format!("session-{stamp}-{}", std::process::id()));
        let mut app = Self {
            page: Page::Elevate,
            demo,
            interfaces: vec![],
            interface: None,
            owned_monitor: None,
            network_restore: None,
            survey: Survey::default(),
            target: None,
            filter: String::new(),
            expanded_networks: HashSet::new(),
            station: String::new(),
            count: "5".into(),
            capture_path: String::new(),
            capture_available: false,
            capture_stamp: None,
            capture_check: None,
            check_after_capture: false,
            capture_started: None,
            hash_path: String::new(),
            wordlist: String::new(),
            recovery_mode: RecoveryMode::Dictionary,
            pattern: "{Word}{Word}[0-9]{3}".into(),
            pattern_check: Pattern::parse("{Word}{Word}[0-9]{3}"),
            pattern_preview: None,
            preview_revision: 0,
            preview_cancel: None,
            candidate_progress: None,
            engine: Engine::Aircrack,
            tools: Tool::ALL
                .into_iter()
                .map(|tool| (tool, find_tool(tool.name()).is_some()))
                .collect(),
            runner: Runner::new(),
            authorization: if demo {
                Authorization::Ready
            } else {
                Authorization::Idle
            },
            jobs: vec![],
            pending: None,
            next_id: 0,
            session,
            activity: vec![],
            copied_activity: None,
            spinner_frame: 0,
            status: "Elevate permissions to begin.".into(),
            status_error: false,
            panels: Panels::default(),
            closing: false,
        };
        app.refresh();
        if demo {
            app.load_demo();
        }
        app
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // Poll only while work can produce events (or shutdown needs advancing).
        // Empty ticks otherwise rebuild and lay out every visible widget at 10 Hz.
        let jobs = if !self.jobs.is_empty() || self.pending.is_some() || self.closing {
            iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        };
        Subscription::batch([jobs, iced::window::close_requests().map(|_| Message::Close)])
    }

    fn refresh(&mut self) {
        if self.demo {
            if self.interfaces.is_empty() {
                self.interfaces = vec![Interface {
                    name: "wlan0".into(),
                    phy: "phy0".into(),
                    monitor: false,
                }];
            }
        } else {
            self.interfaces = model::interfaces();
        }
        let old = self.interface.clone();
        self.interface = old
            .as_ref()
            .and_then(|old| self.interfaces.iter().find(|i| i.name == old.name))
            .or_else(|| {
                old.as_ref().and_then(|old| {
                    self.interfaces
                        .iter()
                        .find(|i| i.phy == old.phy && i.monitor)
                })
            })
            .or_else(|| self.interfaces.first())
            .cloned();
        self.tools = Tool::ALL
            .into_iter()
            .map(|tool| (tool, find_tool(tool.name()).is_some()))
            .collect();
    }

    fn load_demo(&mut self) {
        self.survey = model::demo_survey();
        self.target = self.survey.networks.first().cloned();
        self.wordlist = "/demo/wordlist.txt".into();
        self.status = "Demo mode · all jobs are simulated; no radio operations run.".into();
    }

    fn log(&mut self, id: u64, line: String) {
        if let Some(activity) = self.activity.iter_mut().find(|activity| activity.id == id) {
            activity.push(line);
            if self.copied_activity == Some(id) {
                self.copied_activity = None;
            }
        }
    }

    fn radio_busy(&self) -> bool {
        self.jobs
            .iter()
            .any(|j| j.operation.radio() || matches!(j.operation, Operation::Deauth { .. }))
    }

    fn capture_running(&self) -> bool {
        self.jobs
            .iter()
            .any(|j| matches!(j.operation, Operation::Capture { .. }) && j.started && !j.stopping)
    }

    fn offline_busy(&self) -> bool {
        self.jobs.iter().any(|j| {
            matches!(
                j.operation,
                Operation::Inspect { .. } | Operation::Convert { .. } | Operation::Crack { .. }
            )
        })
    }

    fn can_select_page(&self, page: Page) -> bool {
        !self.closing && page <= self.page
    }

    fn can_go_back(&self) -> bool {
        !self.closing && self.page.previous().is_some()
    }

    fn can_advance(&self) -> bool {
        if self.closing || self.pending.is_some() || self.authorization != Authorization::Ready {
            return false;
        }
        let monitor_ready = self.interface.as_ref().is_some_and(|iface| iface.monitor)
            && !self
                .jobs
                .iter()
                .any(|job| matches!(job.operation, Operation::Monitor { .. }));
        match self.page {
            Page::Elevate => true,
            Page::Monitoring => monitor_ready,
            Page::Discover => monitor_ready && self.target.is_some(),
            Page::Capture => {
                self.target.is_some()
                    && self.capture_available
                    && self.capture_check == Some(Inspection::Found)
                    && !self.radio_busy()
                    && !self.offline_busy()
            }
            Page::Recover => false,
        }
    }

    fn show_page(&mut self, page: Page) -> Task<Message> {
        self.page = page;
        iced::widget::operation::scroll_to(
            "current-step",
            iced::widget::operation::AbsoluteOffset { x: 0.0, y: 0.0 },
        )
    }

    fn refresh_capture_available(&mut self) {
        // Cache file checks on input/job events; scrolling must never stat files.
        self.capture_available = if self.demo {
            !self.capture_path.trim().is_empty()
        } else {
            let stamp = absolute_path(&self.capture_path)
                .ok()
                .and_then(|path| fs::metadata(path).ok())
                .filter(|metadata| metadata.is_file() && metadata.len() > 0)
                .and_then(|metadata| Some((metadata.len(), metadata.modified().ok()?)));
            if self.capture_stamp != stamp {
                self.capture_check = None;
            }
            self.capture_stamp = stamp;
            stamp.is_some()
        };
        if !self.capture_available {
            self.capture_check = None;
        }
    }

    fn inspection_matches(&self, operation: &Operation) -> bool {
        matches!(operation, Operation::Inspect { capture, bssid }
            if absolute_path(&self.capture_path).as_ref().ok() == Some(capture)
                && self.target.as_ref().is_some_and(|target| target.bssid.eq_ignore_ascii_case(bssid)))
    }

    fn launch(&mut self, operation: Operation) {
        let spec = match operation.spec() {
            Ok(spec) => spec,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        self.next_id += 1;
        self.activity.push(CommandActivity {
            id: self.next_id,
            label: operation.label(),
            command: spec.preview(),
            output: VecDeque::new(),
            outcome: String::new(),
        });
        if !self.demo {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            if let Err(error) = builder.create(&self.session) {
                self.command_failed(
                    self.next_id,
                    format!("Cannot create session directory: {error}"),
                );
                return;
            }
        }
        match self
            .runner
            .start(self.next_id, operation.clone(), self.demo)
        {
            Ok(job) => {
                if matches!(operation, Operation::Crack { .. }) {
                    self.candidate_progress = None;
                }
                self.status_error = false;
                self.status = format!(
                    "{}{}",
                    operation.label(),
                    if self.demo {
                        " · simulated"
                    } else {
                        " · starting"
                    }
                );
                if let Operation::Capture { prefix, .. } = &operation {
                    self.capture_path = format!("{}-01.cap", prefix.display());
                    self.capture_available = false;
                    self.capture_check = None;
                    self.capture_started = None;
                    self.hash_path.clear();
                }
                self.jobs.push(job);
            }
            Err(error) => self.command_failed(self.next_id, error),
        }
    }

    fn command_failed(&mut self, id: u64, error: String) {
        self.panels.activity = true;
        if let Some(activity) = self.activity.iter_mut().find(|activity| activity.id == id) {
            activity.outcome = "Failed".into();
        }
        self.log(id, format!("Error: {error}"));
        self.fail(error);
    }

    fn fail(&mut self, error: String) {
        self.status_error = true;
        self.status = error;
        self.closing = false;
    }

    fn operation(&self, action: Action) -> Result<Operation, String> {
        let interface = || {
            self.interface
                .as_ref()
                .ok_or_else(|| "Select a wireless adapter first.".to_string())
        };
        let monitor = || {
            interface().and_then(|i| {
                if i.monitor {
                    Ok(i.name.clone())
                } else {
                    Err("Enable monitor mode first.".into())
                }
            })
        };
        let target = || {
            self.target
                .as_ref()
                .ok_or_else(|| "Select a network in Discover first.".to_string())
        };
        let path = |name: &str| self.session.join(format!("{name}-{}", self.next_id + 1));
        match action {
            Action::Monitor => {
                if self.owned_monitor.is_some() || self.network_restore.is_some() {
                    return Err("Restore the current adapter before enabling another.".into());
                }
                if interface()?.monitor {
                    return Err("This adapter is already in monitor mode.".into());
                }
                Ok(Operation::Monitor {
                    interface: interface()?.name.clone(),
                    enable: true,
                    restore: None,
                })
            }
            Action::Restore => self.restoration_operation(),
            Action::Scan => Ok(Operation::Scan {
                interface: monitor()?,
                prefix: path("survey"),
            }),
            Action::Capture => Ok(Operation::Capture {
                interface: monitor()?,
                bssid: target()?.bssid.clone(),
                channel: target()?.channel,
                prefix: path("capture"),
            }),
            Action::Deauth => {
                let capture = self
                    .jobs
                    .iter()
                    .find(|job| {
                        matches!(job.operation, Operation::Capture { .. })
                            && job.started
                            && !job.stopping
                    })
                    .ok_or("Start a target capture first to lock the adapter to its channel.")?;
                let Operation::Capture {
                    interface, bssid, ..
                } = &capture.operation
                else {
                    unreachable!()
                };
                Ok(Operation::Deauth {
                    interface: interface.clone(),
                    bssid: bssid.clone(),
                    station: if self.station.trim().is_empty() {
                        None
                    } else {
                        Some(self.station.trim().into())
                    },
                    count: self
                        .count
                        .parse()
                        .map_err(|_| "Enter a burst count from 1 to 100.")?,
                })
            }
            Action::Inspect => Ok(Operation::Inspect {
                capture: absolute_path(&self.capture_path)?,
                bssid: target()?.bssid.clone(),
            }),
            Action::Convert => Ok(Operation::Convert {
                capture: absolute_path(&self.capture_path)?,
                output: path("handshake").with_extension("hc22000"),
            }),
            Action::Crack => {
                if self.engine == Engine::Aircrack && !target()?.supports_dictionary() {
                    return Err(
                        "This workflow supports WPA/WPA2 PSK recovery. Select a PSK network."
                            .into(),
                    );
                }
                let pattern =
                    (self.recovery_mode == RecoveryMode::Pattern).then(|| self.pattern.clone());
                let needs_words = match &pattern {
                    Some(source) => Pattern::parse(source)?.needs_words(),
                    None => true,
                };
                Ok(Operation::Crack {
                    engine: self.engine,
                    input: absolute_path(if self.engine == Engine::Aircrack {
                        &self.capture_path
                    } else {
                        &self.hash_path
                    })?,
                    wordlist: if needs_words {
                        absolute_path(&self.wordlist)?
                    } else {
                        PathBuf::new()
                    },
                    pattern,
                    bssid: self
                        .target
                        .as_ref()
                        .map(|n| n.bssid.clone())
                        .unwrap_or_default(),
                    output: path("recovered").with_extension("txt"),
                })
            }
        }
    }

    fn restoration_operation(&self) -> Result<Operation, String> {
        let name = self
            .owned_monitor
            .as_ref()
            .map(|interface| interface.name.clone())
            .or_else(|| {
                self.network_restore
                    .as_ref()
                    .map(|restore| restore.interface.clone())
            })
            .or_else(|| {
                self.interface
                    .as_ref()
                    .filter(|interface| interface.monitor)
                    .map(|interface| interface.name.clone())
            })
            .ok_or("No adapter needs restoration.")?;
        Ok(Operation::Monitor {
            interface: name,
            enable: false,
            restore: self.network_restore.clone(),
        })
    }

    fn run(&mut self, action: Action) {
        if self.authorization != Authorization::Ready {
            return;
        }
        if self.closing || self.pending.is_some() {
            return;
        }
        if matches!(action, Action::Monitor | Action::Restore | Action::Scan) && self.radio_busy() {
            self.fail("Stop the active radio job first.".into());
            return;
        }
        if matches!(action, Action::Capture) && self.offline_busy() {
            self.fail("Wait for the current file check or recovery job to finish.".into());
            return;
        }
        if matches!(action, Action::Inspect | Action::Convert | Action::Crack)
            && (self.offline_busy() || self.radio_busy())
        {
            self.fail("Stop capture and wait for the current recovery job to finish.".into());
            return;
        }
        if matches!(action, Action::Deauth)
            && self
                .jobs
                .iter()
                .any(|j| matches!(j.operation, Operation::Deauth { .. }))
        {
            return;
        }
        if matches!(action, Action::Inspect) {
            self.capture_check = None;
        }
        if matches!(action, Action::Crack) {
            self.invalidate_preview();
        }
        let operation = match self.operation(action) {
            Ok(op) => op,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        if let Err(error) = operation.spec() {
            self.fail(error);
            return;
        }
        if matches!(action, Action::Capture) && self.radio_busy() {
            if self
                .jobs
                .iter()
                .any(|j| j.operation.radio() && !matches!(j.operation, Operation::Scan { .. }))
            {
                self.fail("Stop the current radio job before starting another capture.".into());
                return;
            }
            self.pending = Some(operation);
            for job in &mut self.jobs {
                if matches!(job.operation, Operation::Scan { .. }) {
                    job.stop();
                }
            }
            self.status = "Stopping discovery before locking the target channel…".into();
        } else {
            self.launch(operation);
        }
    }

    fn finished(&mut self, id: u64, code: Option<i32>, cancelled: bool, mut error: Option<String>) {
        let Some(index) = self.jobs.iter().position(|j| j.id == id) else {
            return;
        };
        let job = self.jobs.remove(index);
        let ok = code == Some(0) && error.is_none() && !cancelled;
        self.status_error = !ok && !cancelled;
        if self.status_error {
            self.panels.activity = true;
        }
        self.status = format!(
            "{} · {}",
            job.operation.label(),
            if cancelled {
                "stopped".into()
            } else if ok {
                "finished".into()
            } else {
                format!(
                    "exited {} · see tool output",
                    code.map_or("without a code".into(), |v| v.to_string())
                )
            }
        );
        if let Some(activity) = self.activity.iter_mut().find(|activity| activity.id == id) {
            activity.outcome = if cancelled {
                "Stopped".into()
            } else if ok {
                "Finished".into()
            } else {
                code.map_or("Failed".into(), |code| format!("Exit {code}"))
            };
        }
        match &job.operation {
            Operation::Monitor {
                enable, interface, ..
            } => {
                let old_phy = self
                    .interfaces
                    .iter()
                    .find(|i| &i.name == interface)
                    .map(|i| i.phy.clone());
                let old_monitors: Vec<_> = self
                    .interfaces
                    .iter()
                    .filter(|iface| iface.monitor)
                    .map(|iface| iface.name.clone())
                    .collect();
                if self.demo && ok {
                    self.interfaces = vec![Interface {
                        name: if *enable { "wlan0mon" } else { "wlan0" }.into(),
                        phy: "phy0".into(),
                        monitor: *enable,
                    }];
                }
                self.refresh();
                if *enable {
                    if let Some(found) = self
                        .interfaces
                        .iter()
                        .find(|i| {
                            i.monitor
                                && Some(&i.phy) == old_phy.as_ref()
                                && !old_monitors.contains(&i.name)
                        })
                        .cloned()
                    {
                        self.owned_monitor = Some(found.clone());
                        self.interface = Some(found);
                    } else if !cancelled {
                        error.get_or_insert_with(|| "Monitor mode was not detected on the selected adapter. Another network manager or supplicant may still be using it; see Activity.".into());
                    }
                } else if let Some(owned) = &self.owned_monitor {
                    if !self
                        .interfaces
                        .iter()
                        .any(|i| i.monitor && i.name == owned.name)
                    {
                        self.owned_monitor = None;
                    } else {
                        error.get_or_insert_with(|| {
                            "Adapter restoration did not complete; retry Restore managed mode."
                                .into()
                        });
                    }
                }
            }
            Operation::Capture { .. } => {
                if !self.demo || ok || cancelled {
                    self.refresh_capture_available();
                }
            }
            Operation::Inspect { .. } if self.inspection_matches(&job.operation) => {
                self.refresh_capture_available();
                if error.is_some() || self.capture_check.is_none() {
                    self.capture_check = Some(Inspection::Unknown);
                }
                self.status_error = self.capture_check == Some(Inspection::Unknown);
                self.status = match self.capture_check {
                    Some(Inspection::Found) if self.demo => "Demo check complete · simulated handshake.".into(),
                    Some(Inspection::Found) => "Handshake found. Choose Next to open Recover.".into(),
                    Some(Inspection::NotFound) => "No handshake found yet. Record again while a device reconnects.".into(),
                    _ => "Could not verify this capture. See Activity for details, then retry the check.".into(),
                };
                if self.status_error && !cancelled {
                    self.panels.activity = true;
                }
                if cancelled {
                    self.capture_check = None;
                    self.status_error = false;
                    self.status =
                        "Check cancelled. Choose Check handshake when you’re ready.".into();
                }
            }
            Operation::Convert { output, .. } if ok => {
                if self.demo || fs::metadata(output).is_ok_and(|m| m.len() > 0) {
                    self.hash_path = output.to_string_lossy().into_owned();
                    self.status =
                        "Conversion finished. Review converter output for handshake quality."
                            .into();
                } else {
                    self.status =
                        "No usable hashes were written. Capture more traffic and retry.".into();
                }
            }
            Operation::Crack { output, .. } if ok => {
                self.status = if self.demo {
                    "Demo recovery finished; no password was recovered.".into()
                } else if fs::metadata(output).is_ok_and(|m| m.len() > 0) {
                    format!("Recovered result saved to {}", output.display())
                } else {
                    "Recovery exited successfully; check the tool output for its result.".into()
                };
            }
            _ => {}
        }
        if let Some(error) = error {
            self.pending = None;
            self.command_failed(id, error);
        } else {
            self.log(id, self.status.clone());
        }
    }

    fn invalidate_preview(&mut self) {
        if let Some(cancel) = self.preview_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.preview_revision += 1;
        self.pattern_preview = None;
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Elevate => {
                if self.closing
                    || !self.jobs.is_empty()
                    || matches!(
                        self.authorization,
                        Authorization::Pending | Authorization::Ready
                    )
                {
                    return Task::none();
                }
                if self.demo {
                    return self.update(Message::Authorization(Authorization::Ready));
                }
                match self.runner.authorize() {
                    Ok(()) => {
                        self.authorization = Authorization::Pending;
                        self.status = "Authorize this session in the desktop prompt…".into();
                        self.status_error = false;
                    }
                    Err(error) => {
                        return self.update(Message::Authorization(Authorization::Failed(error)));
                    }
                }
            }
            Message::Authorization(authorization) => {
                match &authorization {
                    Authorization::Ready if !self.closing => {
                        self.status = "Permissions ready for this session.".into();
                        self.status_error = false;
                    }
                    Authorization::Failed(error) if !self.closing => self.fail(error.clone()),
                    _ => {}
                }
                self.authorization = authorization;
            }
            Message::Tick => {
                if (self.page == Page::Monitoring || self.page == Page::Capture)
                    && (!self.jobs.is_empty() || self.pending.is_some())
                {
                    self.spinner_frame = (self.spinner_frame + 1) % 8;
                }
                let events: Vec<_> = self.runner.events.try_iter().take(512).collect();
                for (id, event) in events {
                    match event {
                        Event::Started => {
                            if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                                job.started = true;
                                if matches!(job.operation, Operation::Capture { .. }) {
                                    self.capture_started = Some(Instant::now());
                                }
                            }
                        }
                        Event::Line(line) => self.log(id, line),
                        Event::Inspection(result) => {
                            if self
                                .jobs
                                .iter()
                                .any(|job| job.id == id && self.inspection_matches(&job.operation))
                            {
                                self.capture_check = Some(result);
                            }
                        }
                        Event::Candidates { generated, total } => {
                            if self.jobs.iter().any(|job| {
                                job.id == id
                                    && matches!(
                                        job.operation,
                                        Operation::Crack {
                                            pattern: Some(_),
                                            ..
                                        }
                                    )
                            }) {
                                self.candidate_progress = Some((generated, total));
                            }
                        }
                        Event::NetworkRestore(restore) => self.network_restore = restore,
                        Event::MonitorReady(interface) => {
                            self.owned_monitor = Some(interface.clone());
                            self.interface = Some(interface);
                        }
                        Event::Survey(survey) => {
                            if self.jobs.iter().any(|j| {
                                j.id == id && matches!(j.operation, Operation::Scan { .. })
                            }) {
                                self.survey = survey;
                            } else {
                                self.survey.stations = survey.stations;
                            }
                        }
                        Event::Finished { code, cancelled } => {
                            self.finished(id, code, cancelled, None)
                        }
                        Event::Error(error) => self.finished(id, None, false, Some(error)),
                    }
                }
                if !self.radio_busy()
                    && let Some(operation) = self.pending.take()
                {
                    self.launch(operation);
                }
                if self.check_after_capture && !self.radio_busy() && !self.offline_busy() {
                    self.check_after_capture = false;
                    self.refresh_capture_available();
                    if !self.closing && self.authorization == Authorization::Ready {
                        if self.capture_available {
                            self.run(Action::Inspect);
                        } else {
                            self.capture_check = Some(Inspection::Unknown);
                            self.fail("No capture data was saved. Start recording again.".into());
                        }
                    }
                }
                if self.closing && self.jobs.is_empty() {
                    if self.owned_monitor.is_some() || self.network_restore.is_some() {
                        match self.restoration_operation() {
                            Ok(operation) => self.launch(operation),
                            Err(error) => self.fail(error),
                        }
                    } else if self.runner.shutdown() {
                        return iced::exit();
                    }
                }
            }
            Message::Page(page) => {
                if self.can_select_page(page) {
                    return self.show_page(page);
                }
            }
            Message::Back => {
                if self.can_go_back() {
                    return self.show_page(self.page.previous().unwrap());
                }
            }
            Message::Next => {
                if self.page == Page::Capture && !self.demo {
                    self.refresh_capture_available();
                }
                if self.can_advance() {
                    return self.show_page(self.page.next().unwrap());
                }
            }
            Message::Interface(interface) => {
                if !self.radio_busy() {
                    self.interface = Some(interface);
                }
            }
            Message::Refresh => {
                if !self.radio_busy() {
                    self.refresh();
                    self.refresh_capture_available();
                }
            }
            Message::Select(bssid) => {
                if (!self.radio_busy()
                    || self.jobs.iter().all(|j| {
                        !j.operation.radio() || matches!(j.operation, Operation::Scan { .. })
                    }))
                    && let Some(network) = self.survey.networks.iter().find(|n| n.bssid == bssid)
                {
                    if self
                        .target
                        .as_ref()
                        .is_none_or(|target| target.bssid != bssid)
                    {
                        self.capture_path.clear();
                        self.capture_available = false;
                        self.capture_check = None;
                        self.capture_started = None;
                        self.hash_path.clear();
                    }
                    self.target = Some(network.clone());
                    self.station.clear();
                }
            }
            Message::ToggleNetworkGroup(ssid) => {
                if !self.expanded_networks.remove(&ssid) {
                    self.expanded_networks.insert(ssid);
                }
            }
            Message::Filter(value) => self.filter = value,
            Message::Station(value) => self.station = value,
            Message::Count(value) => self.count = value,
            Message::CapturePath(value) => {
                if !self.radio_busy() && !self.offline_busy() && self.pending.is_none() {
                    self.capture_path = value;
                    self.capture_check = None;
                    self.refresh_capture_available();
                }
            }
            Message::HashPath(value) => self.hash_path = value,
            Message::Wordlist(value) => {
                if !self.offline_busy() {
                    self.wordlist = value;
                    self.invalidate_preview();
                }
            }
            Message::RecoveryMode(mode) => {
                if !self.offline_busy() {
                    self.recovery_mode = mode;
                    self.invalidate_preview();
                    self.candidate_progress = None;
                }
            }
            Message::Pattern(value) => {
                if !self.offline_busy() {
                    self.pattern_check = Pattern::parse(&value);
                    self.pattern = value;
                    self.invalidate_preview();
                }
            }
            Message::PreviewPattern => {
                if self.offline_busy() || self.preview_cancel.is_some() || self.closing {
                    return Task::none();
                }
                let pattern = match &self.pattern_check {
                    Ok(pattern) => pattern.clone(),
                    Err(error) => {
                        self.pattern_preview = Some(Err(error.clone()));
                        return Task::none();
                    }
                };
                let path = if pattern.needs_words() {
                    match absolute_path(&self.wordlist) {
                        Ok(path) => path,
                        Err(error) => {
                            self.pattern_preview = Some(Err(error));
                            return Task::none();
                        }
                    }
                } else {
                    PathBuf::new()
                };
                self.invalidate_preview();
                let revision = self.preview_revision;
                let cancel = Arc::new(AtomicBool::new(false));
                self.preview_cancel = Some(cancel.clone());
                let (sender, receiver) = iced::futures::channel::oneshot::channel();
                std::thread::spawn(move || {
                    let result = pattern
                        .prepare(&path, &cancel)
                        .and_then(|prepared| prepared.preview(&cancel));
                    let _ = sender.send(result);
                });
                return Task::perform(
                    async move {
                        receiver
                            .await
                            .unwrap_or_else(|_| Err("Pattern preview was interrupted.".into()))
                    },
                    move |result| Message::PatternPreview(revision, result),
                );
            }
            Message::PatternPreview(revision, result) => {
                if revision == self.preview_revision && !self.closing {
                    self.preview_cancel = None;
                    self.pattern_preview = Some(result);
                    if self.page == Page::Recover {
                        return iced::widget::operation::scroll_to(
                            "current-step",
                            iced::widget::operation::AbsoluteOffset { x: 0.0, y: 240.0 },
                        );
                    }
                }
            }
            Message::Engine(value) => self.engine = value,
            Message::Run(action) => self.run(action),
            Message::StopAndCheck => {
                if self.capture_running()
                    && !self.offline_busy()
                    && !self.closing
                    && self.authorization == Authorization::Ready
                {
                    self.check_after_capture = true;
                    for job in &mut self.jobs {
                        if matches!(
                            job.operation,
                            Operation::Capture { .. } | Operation::Deauth { .. }
                        ) {
                            job.stop();
                        }
                    }
                    self.status = "Saving the recording before checking for a handshake…".into();
                }
            }
            Message::Stop(id) => {
                self.check_after_capture = false;
                let is_capture = self
                    .jobs
                    .iter()
                    .any(|j| j.id == id && matches!(j.operation, Operation::Capture { .. }));
                for job in &mut self.jobs {
                    if job.id == id
                        || (is_capture && matches!(job.operation, Operation::Deauth { .. }))
                    {
                        job.stop();
                    }
                }
                self.pending = None;
            }
            Message::StopAll => {
                self.check_after_capture = false;
                self.pending = None;
                for job in &mut self.jobs {
                    job.stop();
                }
            }
            Message::ClearActivity => {
                self.activity
                    .retain(|activity| self.jobs.iter().any(|job| job.id == activity.id));
                self.copied_activity = None;
            }
            Message::CopyActivity(id) => {
                if let Some(activity) = self.activity.iter().find(|activity| activity.id == id) {
                    self.copied_activity = Some(id);
                    return iced::clipboard::write(activity.transcript());
                }
            }
            Message::Toggle(panel) => {
                let open = match panel {
                    Panel::Activity => &mut self.panels.activity,
                    Panel::Dependencies => &mut self.panels.dependencies,
                    Panel::HiddenNetworks => &mut self.panels.hidden_networks,
                    Panel::Reconnect => &mut self.panels.reconnect,
                    Panel::CaptureFile => &mut self.panels.capture_file,
                    Panel::Conversion => &mut self.panels.conversion,
                    Panel::Session => &mut self.panels.session,
                    Panel::PatternHelp => &mut self.panels.pattern_help,
                    Panel::RecoverySettings => &mut self.panels.recovery_settings,
                };
                *open = !*open;
            }
            Message::Close => {
                self.invalidate_preview();
                self.check_after_capture = false;
                self.closing = true;
                self.pending = None;
                for job in &mut self.jobs {
                    job.stop();
                }
                self.status = "Stopping jobs and restoring this session’s adapter…".into();
            }
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, thread, time::Instant};

    fn demo_app() -> App {
        let mut app = App::new(true);
        app.interfaces = vec![Interface {
            name: "wlan0mon".into(),
            phy: "phy0".into(),
            monitor: true,
        }];
        app.interface = app.interfaces.first().cloned();
        app.load_demo();
        app
    }

    fn pump_until(app: &mut App, condition: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while !condition(app) {
            assert!(start.elapsed() < Duration::from_secs(5), "{}", app.status);
            let _ = app.update(Message::Tick);
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn recovery_modes_keep_dictionary_and_pattern_inputs_separate() {
        let mut app = demo_app();
        app.capture_path = "/synthetic.cap".into();
        app.wordlist = "/words.txt".into();
        assert!(matches!(
            app.operation(Action::Crack).unwrap(),
            Operation::Crack { pattern: None, .. }
        ));
        let _ = app.update(Message::RecoveryMode(RecoveryMode::Pattern));
        let _ = app.update(Message::Pattern("{word}{5}".into()));
        assert!(
            matches!(app.operation(Action::Crack).unwrap(), Operation::Crack { pattern: Some(source), wordlist, .. } if source == "{word}{5}" && wordlist == Path::new("/words.txt"))
        );
        let _ = app.update(Message::Wordlist(String::new()));
        assert!(app.operation(Action::Crack).is_err());
        let _ = app.update(Message::Pattern("[0-9]{8}".into()));
        assert!(
            matches!(app.operation(Action::Crack).unwrap(), Operation::Crack { wordlist, .. } if wordlist.as_os_str().is_empty())
        );
        let _ = app.update(Message::Pattern("[0-9]+".into()));
        assert!(app.pattern_check.is_err());
        assert!(app.operation(Action::Crack).is_err());
        let _ = app.update(Message::RecoveryMode(RecoveryMode::Dictionary));
        let _ = app.update(Message::Wordlist("/words.txt".into()));
        assert!(matches!(
            app.operation(Action::Crack).unwrap(),
            Operation::Crack { pattern: None, .. }
        ));
    }

    #[test]
    fn edits_cancel_preview_and_ignore_late_results() {
        let mut app = demo_app();
        let _ = app.update(Message::RecoveryMode(RecoveryMode::Pattern));
        let old_revision = app.preview_revision;
        let flag = Arc::new(AtomicBool::new(false));
        app.preview_cancel = Some(flag.clone());
        let _ = app.update(Message::Pattern("[0-9]{8}".into()));
        assert!(flag.load(Ordering::Relaxed));
        assert!(app.preview_cancel.is_none());
        let _ = app.update(Message::PatternPreview(old_revision, Err("stale".into())));
        assert!(app.pattern_preview.is_none());
        let _ = app.update(Message::PatternPreview(
            app.preview_revision,
            Err("current".into()),
        ));
        assert!(matches!(&app.pattern_preview, Some(Err(error)) if error == "current"));
        let _ = app.update(Message::Wordlist("/changed.txt".into()));
        assert!(app.pattern_preview.is_none());
        let old_revision = app.preview_revision;
        let _ = app.update(Message::Close);
        let _ = app.update(Message::PatternPreview(
            old_revision,
            Err("after close".into()),
        ));
        assert!(app.pattern_preview.is_none());
    }

    #[test]
    fn preview_completes_as_a_task_without_starting_a_tool() {
        use iced::futures::{FutureExt, StreamExt};
        let mut app = demo_app();
        let directory = tempfile::tempdir().unwrap();
        let words = directory.path().join("words.txt");
        fs::write(&words, b"alpha\nbeta\n").unwrap();
        let _ = app.update(Message::RecoveryMode(RecoveryMode::Pattern));
        let _ = app.update(Message::Wordlist(words.to_string_lossy().into_owned()));
        let task = app.update(Message::PreviewPattern);
        assert!(app.preview_cancel.is_some());
        assert!(app.pattern_preview.is_none());
        assert!(app.jobs.is_empty());
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let started = Instant::now();
        loop {
            if let Some(Some(iced_runtime::Action::Output(message))) = stream.next().now_or_never()
            {
                let _ = app.update(message);
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(5));
        }
        assert!(app.preview_cancel.is_none());
        let preview = app.pattern_preview.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(preview.total, 4_000);
        assert_eq!(preview.samples[0], "AlphaAlpha000");
        assert!(app.activity.is_empty());
        assert!(app.jobs.is_empty());
    }

    #[test]
    fn expanding_ssid_groups_keeps_selection_on_an_explicit_bssid() {
        let mut app = demo_app();
        app.page = Page::Discover;
        app.target = None;
        let ssid = app.survey.networks[0].ssid.clone();
        app.survey.networks[1].ssid = ssid.clone();
        let target = app.survey.networks[1].clone();
        assert!(app.expanded_networks.is_empty());
        let _ = app.update(Message::ToggleNetworkGroup(ssid.clone()));
        assert!(app.target.is_none());
        assert!(!app.can_advance());
        let _ = app.update(Message::Select(target.bssid.clone()));
        assert!(app.can_advance());
        let _ = app.update(Message::ToggleNetworkGroup(ssid.clone()));
        assert!(!app.expanded_networks.contains(&ssid));
        assert_eq!(app.target.as_ref(), Some(&target));
        let _ = app.update(Message::ToggleNetworkGroup(ssid.clone()));
        app.survey.networks.reverse();
        let _ = app.update(Message::Filter(target.bssid.clone()));
        assert!(app.expanded_networks.contains(&ssid));
        assert!(matches!(
            app.operation(Action::Capture).unwrap(),
            Operation::Capture { bssid, channel, .. } if bssid == target.bssid && channel == target.channel
        ));
    }

    #[test]
    fn capture_waits_for_scan_and_keeps_its_target_locked() {
        let mut app = demo_app();
        app.run(Action::Scan);
        assert_eq!(app.jobs.len(), 1);
        app.run(Action::Capture);
        assert!(app.pending.is_some());
        assert!(app.jobs[0].stopping);
        pump_until(&mut app, App::capture_running);
        assert_eq!(app.jobs.len(), 1);
        let bssid = app.target.as_ref().unwrap().bssid.clone();
        let _ = app.update(Message::Select("02:00:00:00:01:02".into()));
        assert_eq!(app.target.as_ref().unwrap().bssid, bssid);
        app.run(Action::Deauth);
        assert_eq!(app.jobs.len(), 2);
        let capture = app
            .jobs
            .iter()
            .find(|j| matches!(j.operation, Operation::Capture { .. }))
            .unwrap()
            .id;
        let _ = app.update(Message::Stop(capture));
        assert!(app.jobs.iter().all(|j| j.stopping));
        pump_until(&mut app, |a| a.jobs.is_empty());
    }

    #[test]
    fn deauth_requires_capture() {
        let mut app = demo_app();
        app.run(Action::Deauth);
        assert!(app.jobs.is_empty());
        assert!(app.status.contains("Start a target capture"));
    }

    #[test]
    fn idle_polling_sleeps_but_jobs_and_window_closure_still_advance() {
        let mut app = demo_app();
        assert_eq!(app.subscription().units(), 1); // Window close listener only.
        app.run(Action::Scan);
        assert_eq!(app.subscription().units(), 2);
        let _ = app.update(Message::StopAll);
        pump_until(&mut app, |a| a.jobs.is_empty());
        assert_eq!(app.subscription().units(), 1);
        let _ = app.update(Message::Close);
        assert_eq!(app.subscription().units(), 2);
    }

    #[test]
    fn live_startup_waits_for_explicit_elevation() {
        let mut app = App::new(false);
        assert_eq!(app.page, Page::Elevate);
        assert_eq!(Page::ALL[0], Page::Elevate);
        assert_eq!(app.authorization, Authorization::Idle);
        let _ = app.update(Message::Refresh);
        let _ = app.update(Message::Page(Page::Monitoring));
        app.run(Action::Monitor);
        assert!(app.jobs.is_empty());
        assert!(app.activity.is_empty());
        assert_eq!(app.authorization, Authorization::Idle);
        assert_eq!(app.subscription().units(), 1);
        assert!(matches!(
            app.runner.authorization.as_mut().unwrap().try_recv(),
            Err(iced::futures::channel::mpsc::TryRecvError::Empty)
        ));
        assert!(app.runner.shutdown());
    }

    #[test]
    fn elevation_ignores_duplicate_clicks_and_can_retry_failure() {
        let mut app = demo_app();
        app.authorization = Authorization::Pending;
        let _ = app.update(Message::Elevate);
        assert_eq!(app.authorization, Authorization::Pending);
        let _ = app.update(Message::Authorization(Authorization::Failed(
            "Authorization was cancelled".into(),
        )));
        assert!(app.status_error);
        // Demo retry follows the same UI transition without invoking polkit.
        let _ = app.update(Message::Elevate);
        assert_eq!(app.authorization, Authorization::Ready);
        assert!(!app.status_error);
        let _ = app.update(Message::Elevate);
        assert_eq!(app.authorization, Authorization::Ready);
        let _ = app.update(Message::Close);
        app.authorization = Authorization::Idle;
        let _ = app.update(Message::Elevate);
        assert_eq!(app.authorization, Authorization::Idle);
    }

    #[test]
    fn tools_wait_for_authorization_and_stop_accepting_jobs_on_failure() {
        let mut app = demo_app();
        app.authorization = Authorization::Pending;
        app.run(Action::Scan);
        assert!(app.jobs.is_empty());
        let _ = app.update(Message::Authorization(Authorization::Ready));
        app.run(Action::Scan);
        assert_eq!(app.jobs.len(), 1);
        let _ = app.update(Message::StopAll);
        pump_until(&mut app, |app| app.jobs.is_empty());
        let _ = app.update(Message::Authorization(Authorization::Failed(
            "Session ended".into(),
        )));
        assert!(app.status_error);
        app.run(Action::Scan);
        assert!(app.jobs.is_empty());
        assert_eq!(app.subscription().units(), 1);
    }

    #[test]
    fn navigation_requires_completion_and_step_shortcuts_only_go_back() {
        let mut app = demo_app();
        app.authorization = Authorization::Idle;
        assert!(!app.can_go_back());
        let _ = app.update(Message::Back);
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Elevate);
        let _ = app.update(Message::Authorization(Authorization::Ready));
        assert!(app.can_advance());
        // Even completed prerequisites never make a future step clickable.
        for page in Page::ALL.into_iter().skip(1) {
            assert!(!app.can_select_page(page));
            let _ = app.update(Message::Page(page));
            assert_eq!(app.page, Page::Elevate);
        }
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Monitoring);
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Discover);
        app.target = None;
        assert!(!app.can_advance());
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Discover);
        let bssid = app.survey.networks[0].bssid.clone();
        let _ = app.update(Message::Select(bssid));
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Capture);
        assert!(!app.can_advance());
        let _ = app.update(Message::Authorization(Authorization::Failed(
            "Session ended".into(),
        )));
        // Losing prerequisites does not prevent returning to fix them.
        let _ = app.update(Message::Back);
        assert_eq!(app.page, Page::Discover);
        assert!(!app.can_advance());
        let _ = app.update(Message::Page(Page::Elevate));
        assert_eq!(app.page, Page::Elevate);
        let _ = app.update(Message::Page(Page::Discover));
        assert_eq!(app.page, Page::Elevate);
        let _ = app.update(Message::Authorization(Authorization::Ready));
        let _ = app.update(Message::Next);
        app.interface.as_mut().unwrap().monitor = false;
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Monitoring);
        let _ = app.update(Message::Close);
        assert!(!app.can_go_back());
        let _ = app.update(Message::Back);
        let _ = app.update(Message::Page(Page::Elevate));
        assert_eq!(app.page, Page::Monitoring);
    }

    #[test]
    fn navigation_waits_for_monitor_setup_and_capture_shutdown() {
        let mut app = App::new(true);
        app.target = None;
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Monitoring);
        assert!(!app.can_advance());
        app.run(Action::Monitor);
        // A monitor interface can appear before the setup job has completed.
        app.interface.as_mut().unwrap().monitor = true;
        assert!(!app.can_advance());
        pump_until(&mut app, |app| app.jobs.is_empty());
        assert!(app.can_advance());
        let _ = app.update(Message::Next);
        app.run(Action::Scan);
        assert!(!app.can_advance());
        let bssid = app.survey.networks[0].bssid.clone();
        let _ = app.update(Message::Select(bssid));
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Capture);
        app.run(Action::Capture);
        assert!(app.pending.is_some());
        assert!(!app.can_advance());
        pump_until(&mut app, App::capture_running);
        assert!(!app.capture_path.is_empty());
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Capture);
        let _ = app.update(Message::StopAndCheck);
        assert!(!app.can_advance());
        pump_until(&mut app, |app| app.jobs.is_empty());
        assert!(app.can_advance());
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Recover);
        assert!(!app.can_advance());
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Recover);
        let _ = app.update(Message::Back);
        assert_eq!(app.page, Page::Capture);
        assert!(app.can_advance());
        let _ = app.update(Message::Page(Page::Discover));
        let bssid = app.survey.networks[1].bssid.clone();
        let _ = app.update(Message::Select(bssid));
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Capture);
        assert!(!app.can_advance());
        assert!(app.capture_path.is_empty());
    }

    #[test]
    fn stop_and_check_waits_for_radio_shutdown_and_can_be_cancelled() {
        let mut app = demo_app();
        app.page = Page::Capture;
        app.run(Action::Capture);
        pump_until(&mut app, App::capture_running);
        assert_eq!(
            app.jobs.len(),
            1,
            "recording never sends disconnect requests automatically"
        );
        app.run(Action::Deauth);
        let _ = app.update(Message::StopAndCheck);
        assert!(app.check_after_capture);
        assert!(app.jobs.iter().all(|job| job.stopping));
        pump_until(&mut app, |app| {
            app.jobs
                .iter()
                .any(|job| matches!(job.operation, Operation::Inspect { .. }))
        });
        assert!(!app.radio_busy());
        assert!(!app.can_advance());
        assert!(!app.check_after_capture);
        let path = app.capture_path.clone();
        let _ = app.update(Message::CapturePath("/different.cap".into()));
        assert_eq!(
            app.capture_path, path,
            "files cannot change during inspection"
        );
        let _ = app.update(Message::StopAll);
        pump_until(&mut app, |app| app.jobs.is_empty());
        assert_eq!(app.capture_check, None);
        assert!(!app.can_advance());
        assert!(!app.status_error);
    }

    #[test]
    fn successful_checks_are_invalidated_by_file_or_target_changes() {
        let mut app = demo_app();
        app.page = Page::Capture;
        let _ = app.update(Message::CapturePath("/first.cap".into()));
        app.capture_check = Some(Inspection::Found);
        assert!(app.can_advance());
        let _ = app.update(Message::CapturePath("/second.cap".into()));
        assert_eq!(app.capture_check, None);
        assert!(!app.can_advance());
        app.capture_check = Some(Inspection::Found);
        let other = app.survey.networks[1].bssid.clone();
        let _ = app.update(Message::Select(other));
        assert_eq!(app.capture_check, None);
        assert!(app.capture_path.is_empty());
        assert!(!app.can_advance());
    }

    #[test]
    fn recovery_requires_a_nonempty_file_and_rechecks_it_on_next() {
        let mut app = demo_app();
        app.demo = false; // Validate real files without starting any subprocess.
        app.page = Page::Capture;
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("test.cap");
        for path in [&capture, dir.path()] {
            let _ = app.update(Message::CapturePath(path.to_string_lossy().into_owned()));
            assert!(!app.can_advance());
        }
        fs::write(&capture, []).unwrap();
        let _ = app.update(Message::CapturePath(capture.to_string_lossy().into_owned()));
        assert!(!app.can_advance());
        fs::write(&capture, b"synthetic capture data").unwrap();
        let _ = app.update(Message::CapturePath(capture.to_string_lossy().into_owned()));
        assert!(
            !app.can_advance(),
            "a nonempty file alone is not a handshake"
        );
        app.capture_check = Some(Inspection::NotFound);
        assert!(!app.can_advance());
        app.capture_check = Some(Inspection::Found);
        assert!(app.can_advance());
        fs::write(&capture, b"changed capture data with a different size").unwrap();
        let _ = app.update(Message::Next);
        assert_eq!(
            app.page,
            Page::Capture,
            "changed data invalidates the previous check"
        );
        assert_eq!(app.capture_check, None);
        app.capture_check = Some(Inspection::Found);
        fs::remove_file(&capture).unwrap();
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Capture);
        assert!(!app.can_advance());
        fs::write(&capture, b"synthetic capture data").unwrap();
        let _ = app.update(Message::Next);
        assert_eq!(
            app.page,
            Page::Capture,
            "a replaced file needs another check"
        );
        app.capture_check = Some(Inspection::Found);
        let _ = app.update(Message::Next);
        assert_eq!(app.page, Page::Recover);
    }
}
