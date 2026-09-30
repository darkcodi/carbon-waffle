use crate::{
    command::{Engine, Operation, Tool, find_tool},
    model::{self, Interface, Network, Survey, absolute_path},
    monitor::NetworkRestore,
    runner::{Event, Job, Runner},
};
use iced::{Subscription, Task};
use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod appearance;
mod view;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Monitoring,
    Discover,
    Capture,
    Recover,
}

impl Page {
    const ALL: [Self; 4] = [
        Self::Monitoring,
        Self::Discover,
        Self::Capture,
        Self::Recover,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Monitoring => "Monitoring",
            Self::Discover => "Discover",
            Self::Capture => "Capture",
            Self::Recover => "Recover",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Panel {
    Activity,
    Tools,
    Reconnect,
    CaptureFile,
    Conversion,
    Session,
}

#[derive(Default)]
struct Panels {
    activity: bool,
    tools: bool,
    reconnect: bool,
    capture_file: bool,
    conversion: bool,
    session: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    Monitor,
    Restore,
    Check,
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
    Page(Page),
    Interface(Interface),
    Refresh,
    Demo,
    Select(String),
    Filter(String),
    Station(String),
    Count(String),
    CapturePath(String),
    HashPath(String),
    Wordlist(String),
    Engine(Engine),
    Run(Action),
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
    station: String,
    count: String,
    capture_path: String,
    hash_path: String,
    wordlist: String,
    engine: Engine,
    tools: Vec<(Tool, bool)>,
    runner: Runner,
    jobs: Vec<Job>,
    pending: Option<Operation>,
    next_id: u64,
    session: PathBuf,
    activity: Vec<CommandActivity>,
    copied_activity: Option<u64>,
    status: String,
    status_error: bool,
    panels: Panels,
    closing: bool,
}

impl App {
    pub fn new() -> Self {
        let demo = std::env::args().any(|arg| arg == "--demo");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let session = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("captures")
            .join(format!("session-{stamp}-{}", std::process::id()));
        let mut app = Self {
            page: Page::Monitoring,
            demo,
            interfaces: vec![],
            interface: None,
            owned_monitor: None,
            network_restore: None,
            survey: Survey::default(),
            target: None,
            filter: String::new(),
            station: String::new(),
            count: "5".into(),
            capture_path: String::new(),
            hash_path: String::new(),
            wordlist: String::new(),
            engine: Engine::Aircrack,
            tools: Tool::ALL
                .into_iter()
                .map(|tool| (tool, find_tool(tool.name()).is_some()))
                .collect(),
            runner: Runner::new(),
            jobs: vec![],
            pending: None,
            next_id: 0,
            session,
            activity: vec![],
            copied_activity: None,
            status: "Ready when you are.".into(),
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
        Subscription::batch([
            iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            iced::window::close_requests().map(|_| Message::Close),
        ])
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
                self.status_error = false;
                if matches!(operation, Operation::Inspect { .. }) {
                    self.panels.activity = true;
                }
                self.status = format!(
                    "{}{}",
                    operation.label(),
                    if self.demo {
                        " · simulated"
                    } else if spec.privileged {
                        " · waiting for worker / desktop authorization"
                    } else {
                        " · starting"
                    }
                );
                if let Operation::Capture { prefix, .. } = &operation {
                    self.capture_path = format!("{}-01.cap", prefix.display());
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
            Action::Check => Ok(Operation::Check),
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
                    return Err("This workflow supports WPA/WPA2 PSK dictionary recovery. Select a PSK network.".into());
                }
                Ok(Operation::Crack {
                    engine: self.engine,
                    input: absolute_path(if self.engine == Engine::Aircrack {
                        &self.capture_path
                    } else {
                        &self.hash_path
                    })?,
                    wordlist: absolute_path(&self.wordlist)?,
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
        if self.closing || self.pending.is_some() {
            return;
        }
        if matches!(action, Action::Monitor | Action::Restore | Action::Scan) && self.radio_busy() {
            self.fail("Stop the active radio job first.".into());
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
        if matches!(action, Action::Check)
            && self
                .jobs
                .iter()
                .any(|j| matches!(j.operation, Operation::Check))
        {
            return;
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

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let events: Vec<_> = self.runner.events.try_iter().take(512).collect();
                for (id, event) in events {
                    match event {
                        Event::Started => {
                            if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                                job.started = true;
                            }
                        }
                        Event::Line(line) => self.log(id, line),
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
                if self.closing && self.jobs.is_empty() {
                    if self.owned_monitor.is_some() || self.network_restore.is_some() {
                        match self.restoration_operation() {
                            Ok(operation) => self.launch(operation),
                            Err(error) => self.fail(error),
                        }
                    } else {
                        return iced::exit();
                    }
                }
            }
            Message::Page(page) => {
                self.page = page;
                return iced::widget::operation::scroll_to(
                    "current-step",
                    iced::widget::operation::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
            }
            Message::Interface(interface) => {
                if !self.radio_busy() {
                    self.interface = Some(interface);
                }
            }
            Message::Refresh => {
                if !self.radio_busy() {
                    self.refresh();
                }
            }
            Message::Demo => {
                if self.jobs.is_empty()
                    && self.owned_monitor.is_none()
                    && self.network_restore.is_none()
                {
                    self.demo = !self.demo;
                    self.page = Page::Monitoring;
                    self.panels = Panels::default();
                    self.status_error = false;
                    self.interfaces.clear();
                    self.interface = None;
                    self.survey = Survey::default();
                    self.target = None;
                    self.capture_path.clear();
                    self.hash_path.clear();
                    self.wordlist.clear();
                    self.station.clear();
                    self.pending = None;
                    self.activity.clear();
                    self.copied_activity = None;
                    self.refresh();
                    if self.demo {
                        self.load_demo();
                    } else {
                        self.status = "Live mode · select your research adapter.".into();
                    }
                }
            }
            Message::Select(bssid) => {
                if (!self.radio_busy()
                    || self.jobs.iter().all(|j| {
                        !j.operation.radio() || matches!(j.operation, Operation::Scan { .. })
                    }))
                    && let Some(network) = self.survey.networks.iter().find(|n| n.bssid == bssid)
                {
                    self.target = Some(network.clone());
                    self.station.clear();
                }
            }
            Message::Filter(value) => self.filter = value,
            Message::Station(value) => self.station = value,
            Message::Count(value) => self.count = value,
            Message::CapturePath(value) => self.capture_path = value,
            Message::HashPath(value) => self.hash_path = value,
            Message::Wordlist(value) => self.wordlist = value,
            Message::Engine(value) => self.engine = value,
            Message::Run(action) => self.run(action),
            Message::Stop(id) => {
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
                    Panel::Tools => &mut self.panels.tools,
                    Panel::Reconnect => &mut self.panels.reconnect,
                    Panel::CaptureFile => &mut self.panels.capture_file,
                    Panel::Conversion => &mut self.panels.conversion,
                    Panel::Session => &mut self.panels.session,
                };
                *open = !*open;
            }
            Message::Close => {
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
    use std::{thread, time::Instant};

    fn demo_app() -> App {
        let mut app = App::new();
        app.demo = true;
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
    fn deauth_requires_capture_and_demo_cannot_switch_during_a_job() {
        let mut app = demo_app();
        app.run(Action::Deauth);
        assert!(app.jobs.is_empty());
        assert!(app.status.contains("Start a target capture"));
        app.run(Action::Scan);
        let _ = app.update(Message::Demo);
        assert!(app.demo);
        let _ = app.update(Message::StopAll);
        pump_until(&mut app, |a| a.jobs.is_empty());
    }
}
