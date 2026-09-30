use crate::{
    command::{Engine, Operation, Tool, find_tool},
    model::{self, Interface, Network, Survey, absolute_path},
    runner::{Event, Job, Runner},
};
use iced::{
    Element, Fill, Font, Subscription, Task, Theme,
    widget::{self, button, column, container, pick_list, row, scrollable, text, text_input},
};
use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Discover,
    Capture,
    Recover,
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
    ClearLog,
    Close,
}

pub struct App {
    page: Page,
    demo: bool,
    interfaces: Vec<Interface>,
    interface: Option<Interface>,
    owned_monitor: Option<Interface>,
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
    log: VecDeque<String>,
    status: String,
    preview: String,
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
            page: Page::Discover,
            demo,
            interfaces: vec![],
            interface: None,
            owned_monitor: None,
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
            log: VecDeque::new(),
            status: "Choose an adapter, enable monitor mode, then start discovery.".into(),
            preview: "Commands appear here when a job starts.".into(),
            closing: false,
        };
        app.refresh();
        if demo {
            app.load_demo();
        }
        app
    }

    pub fn theme(&self) -> Theme {
        Theme::TokyoNight
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

    fn log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > 400 {
            self.log.pop_front();
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
        if !self.demo {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            if let Err(error) = builder.create(&self.session) {
                self.fail(format!("Cannot create session directory: {error}"));
                return;
            }
        }
        self.next_id += 1;
        match self
            .runner
            .start(self.next_id, operation.clone(), self.demo)
        {
            Ok(job) => {
                self.preview = operation.spec().unwrap().preview();
                self.log(format!("[{}] $ {}", job.id, self.preview));
                self.status = format!(
                    "{}{}",
                    operation.label(),
                    if self.demo {
                        " · simulated"
                    } else if operation.spec().unwrap().privileged {
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
            Err(error) => self.fail(error),
        }
    }

    fn fail(&mut self, error: String) {
        self.log(format!("Error: {error}"));
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
                if self.owned_monitor.is_some() {
                    return Err("Restore the current adapter before enabling another.".into());
                }
                if interface()?.monitor {
                    return Err("This adapter is already in monitor mode.".into());
                }
                Ok(Operation::Monitor {
                    interface: interface()?.name.clone(),
                    enable: true,
                })
            }
            Action::Restore => Ok(Operation::Monitor {
                interface: monitor()?,
                enable: false,
            }),
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

    fn finished(&mut self, id: u64, code: Option<i32>, cancelled: bool, error: Option<String>) {
        let Some(index) = self.jobs.iter().position(|j| j.id == id) else {
            return;
        };
        let job = self.jobs.remove(index);
        let ok = code == Some(0) && error.is_none() && !cancelled;
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
        self.log(format!("[{id}] {}", self.status));
        match &job.operation {
            Operation::Monitor { enable, interface } => {
                let old_phy = self
                    .interfaces
                    .iter()
                    .find(|i| &i.name == interface)
                    .map(|i| i.phy.clone());
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
                        .find(|i| i.monitor && Some(&i.phy) == old_phy.as_ref())
                        .cloned()
                    {
                        self.owned_monitor = Some(found.clone());
                        self.interface = Some(found);
                    } else {
                        self.status = "Monitor mode was not detected. Check tool output and refresh adapters.".into();
                    }
                } else if let Some(owned) = &self.owned_monitor {
                    if !self
                        .interfaces
                        .iter()
                        .any(|i| i.monitor && i.phy == owned.phy)
                    {
                        self.owned_monitor = None;
                    } else if self.closing {
                        self.fail(
                            "Adapter restoration did not complete; retry Restore managed mode."
                                .into(),
                        );
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
            self.fail(error);
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
                        Event::Line(line) => self.log(format!("[{id}] {line}")),
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
                    if let Some(interface) = self.owned_monitor.clone() {
                        self.launch(Operation::Monitor {
                            interface: interface.name,
                            enable: false,
                        });
                    } else {
                        return iced::exit();
                    }
                }
            }
            Message::Page(page) => self.page = page,
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
                if self.jobs.is_empty() && self.owned_monitor.is_none() {
                    self.demo = !self.demo;
                    self.interfaces.clear();
                    self.interface = None;
                    self.survey = Survey::default();
                    self.target = None;
                    self.capture_path.clear();
                    self.hash_path.clear();
                    self.wordlist.clear();
                    self.station.clear();
                    self.pending = None;
                    self.log.clear();
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
            Message::ClearLog => self.log.clear(),
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

    pub fn view(&self) -> Element<'_, Message> {
        let title = row![
            column![
                text("CARBON WAFFLE").size(26),
                text("Wireless research workbench")
                    .size(13)
                    .style(text::secondary)
            ]
            .spacing(4),
            widget::space().width(Fill),
            button("Stop all jobs")
                .on_press_maybe((!self.jobs.is_empty()).then_some(Message::StopAll))
                .style(button::danger),
            button(if self.demo {
                "DEMO MODE  ·  switch to live"
            } else {
                "LIVE MODE  ·  try demo"
            })
            .on_press_maybe(
                (self.jobs.is_empty() && self.owned_monitor.is_none() && !self.closing)
                    .then_some(Message::Demo)
            )
            .style(button::secondary),
        ]
        .spacing(12)
        .align_y(iced::Center);

        let tabs = row![
            self.tab("01  Discover", Page::Discover),
            self.tab("02  Capture", Page::Capture),
            self.tab("03  Recover", Page::Recover)
        ]
        .spacing(8);
        let content = match self.page {
            Page::Discover => self.discover(),
            Page::Capture => self.capture(),
            Page::Recover => self.recover(),
        };
        let body = row![
            self.sidebar(),
            container(column![tabs, content].spacing(16)).width(Fill)
        ]
        .spacing(20);
        let logs = self
            .log
            .iter()
            .rev()
            .take(90)
            .rev()
            .fold(column![].spacing(2), |col, line| {
                col.push(text(line).font(Font::MONOSPACE).size(12))
            });
        let console = container(
            column![
                row![
                    text("ACTIVITY").size(12),
                    widget::space().width(Fill),
                    button("Clear")
                        .on_press(Message::ClearLog)
                        .style(button::text)
                ]
                .align_y(iced::Center),
                text(&self.preview)
                    .font(Font::MONOSPACE)
                    .size(12)
                    .style(text::primary),
                scrollable(logs).anchor_bottom().height(140),
            ]
            .spacing(6),
        )
        .padding(14)
        .style(container::rounded_box);
        container(
            column![
                title,
                widget::rule::horizontal(1),
                scrollable(container(body).padding(iced::Padding {
                    right: 14.0,
                    ..Default::default()
                }))
                .height(Fill),
                container(text(&self.status).size(13))
                    .padding(10)
                    .width(Fill)
                    .style(container::rounded_box),
                console,
            ]
            .spacing(14),
        )
        .padding(24)
        .into()
    }

    fn tab<'a>(&self, label: &'a str, page: Page) -> Element<'a, Message> {
        button(text(label).size(14))
            .padding([10, 18])
            .on_press(Message::Page(page))
            .style(if self.page == page {
                button::primary
            } else {
                button::secondary
            })
            .into()
    }

    fn action(&self, label: &'static str, action: Action, enabled: bool) -> Element<'_, Message> {
        button(label)
            .padding([9, 12])
            .on_press_maybe(
                (enabled && !self.closing && self.pending.is_none())
                    .then_some(Message::Run(action)),
            )
            .into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let idle = !self.radio_busy();
        let monitor = self.interface.as_ref().is_some_and(|i| i.monitor);
        let mut adapters = pick_list(
            self.interfaces.clone(),
            self.interface.clone(),
            Message::Interface,
        )
        .placeholder("No wireless adapter")
        .width(Fill);
        if !idle {
            adapters = pick_list(self.interfaces.clone(), self.interface.clone(), |_| {
                Message::Tick
            })
            .width(Fill);
        }
        let tools = self
            .tools
            .iter()
            .fold(column![].spacing(5), |col, (tool, exists)| {
                col.push(
                    row![
                        text(if *exists { "●" } else { "○" }).style(if *exists {
                            text::success
                        } else {
                            text::secondary
                        }),
                        text(tool.name()).size(12)
                    ]
                    .spacing(8),
                )
            });
        let mut jobs = column![].spacing(6);
        for job in &self.jobs {
            jobs = jobs.push(
                row![
                    text(format!(
                        "{}\n{}",
                        job.operation.label(),
                        if job.stopping {
                            "Stopping…"
                        } else if job.started {
                            "Running"
                        } else {
                            "Starting…"
                        }
                    ))
                    .size(12)
                    .width(Fill),
                    button("Stop")
                        .on_press_maybe((!job.stopping).then_some(Message::Stop(job.id)))
                        .style(button::danger),
                ]
                .align_y(iced::Center)
                .spacing(6),
            );
        }
        container(
            column![
                text("ADAPTER").size(12).style(text::secondary),
                adapters,
                button("Refresh adapters & tools")
                    .on_press_maybe(idle.then_some(Message::Refresh))
                    .style(button::text),
                self.action(
                    "Enable monitor mode",
                    Action::Monitor,
                    idle && !monitor && self.interface.is_some()
                ),
                self.action("Restore managed mode", Action::Restore, idle && monitor),
                self.action("Check interfering processes", Action::Check, idle),
                widget::rule::horizontal(1),
                text("TOOLCHAIN").size(12).style(text::secondary),
                tools,
                widget::rule::horizontal(1),
                text(format!("JOBS  /  {}", self.jobs.len()))
                    .size(12)
                    .style(text::secondary),
                jobs,
                button("Stop all jobs")
                    .on_press_maybe((!self.jobs.is_empty()).then_some(Message::StopAll))
                    .style(button::danger),
                text(if self.demo {
                    "Demo data · no tools executed"
                } else {
                    "Radio jobs request desktop authorization. Recovery runs as your user."
                })
                .size(12)
                .style(text::secondary),
            ]
            .spacing(12),
        )
        .width(270)
        .padding(16)
        .style(container::rounded_box)
        .into()
    }

    fn discover(&self) -> Element<'_, Message> {
        let mut networks = column![].spacing(6);
        let filter = self.filter.to_lowercase();
        let selectable = !self.radio_busy()
            || self
                .jobs
                .iter()
                .all(|j| !j.operation.radio() || matches!(j.operation, Operation::Scan { .. }));
        for network in self.survey.networks.iter().filter(|n| {
            n.ssid.to_lowercase().contains(&filter) || n.bssid.to_lowercase().contains(&filter)
        }) {
            let selected = self
                .target
                .as_ref()
                .is_some_and(|n| n.bssid == network.bssid);
            let clients = self
                .survey
                .stations
                .iter()
                .filter(|s| s.bssid == network.bssid)
                .count();
            networks = networks.push(
                button(
                    row![
                        column![
                            text(network.label()).size(16),
                            text(&network.bssid)
                                .size(12)
                                .font(Font::MONOSPACE)
                                .style(text::secondary)
                        ]
                        .width(Fill)
                        .spacing(4),
                        column![
                            text(format!("{} · {}", network.security, network.authentication))
                                .size(12),
                            text(format!(
                                "CH {}   {} dBm   {} clients",
                                network.channel, network.power, clients
                            ))
                            .size(12)
                        ]
                        .spacing(4),
                    ]
                    .align_y(iced::Center)
                    .spacing(8),
                )
                .padding(12)
                .width(Fill)
                .style(if selected {
                    button::primary
                } else {
                    button::secondary
                })
                .on_press_maybe(selectable.then(|| Message::Select(network.bssid.clone()))),
            );
        }
        if self.survey.networks.is_empty() {
            networks = networks.push(
                container(
                    column![
                        text("Your next session starts here.").size(22),
                        text(
                            "Enable monitor mode and start discovery to see nearby access points."
                        )
                        .size(14)
                        .style(text::secondary)
                    ]
                    .spacing(10),
                )
                .padding([40, 12]),
            );
        }
        column![
            text("Discover networks").size(28),
            text("Scan 2.4 and 5 GHz, inspect clients, and choose a target.")
                .size(14)
                .style(text::secondary),
            row![
                text_input("Filter by network name or BSSID", &self.filter)
                    .on_input(Message::Filter),
                self.action("Start discovery", Action::Scan, !self.radio_busy())
            ]
            .spacing(10),
            text(format!(
                "{} access points observed",
                self.survey.networks.len()
            ))
            .size(12)
            .style(text::secondary),
            networks,
            row![
                self.action(
                    "Capture selected network",
                    Action::Capture,
                    self.target.is_some() && selectable
                ),
                button("Open capture workspace →")
                    .on_press(Message::Page(Page::Capture))
                    .style(button::text)
            ]
            .spacing(8),
        ]
        .spacing(14)
        .into()
    }

    fn target_card(&self) -> Element<'_, Message> {
        let content = if let Some(target) = &self.target {
            column![
                text(target.label()).size(22),
                text(format!(
                    "{}  ·  channel {}  ·  {} / {}",
                    target.bssid, target.channel, target.security, target.authentication
                ))
                .size(13)
                .font(Font::MONOSPACE)
            ]
            .spacing(8)
        } else {
            column![
                text("No target selected").size(22),
                text("Choose a network in Discover.").size(14)
            ]
        };
        container(content)
            .padding(16)
            .width(Fill)
            .style(container::rounded_box)
            .into()
    }

    fn capture(&self) -> Element<'_, Message> {
        let mut clients = row![
            button("All clients")
                .on_press(Message::Station(String::new()))
                .style(button::secondary)
        ]
        .spacing(6);
        if let Some(target) = &self.target {
            for client in self
                .survey
                .stations
                .iter()
                .filter(|c| c.bssid == target.bssid)
            {
                clients = clients.push(
                    button(text(&client.mac).size(12))
                        .on_press(Message::Station(client.mac.clone()))
                        .style(button::secondary),
                );
            }
        }
        column![
            text("Capture a handshake").size(28), self.target_card(),
            text("Capture locks the adapter to the selected channel. A running discovery job stops first.").size(14).style(text::secondary),
            self.action("Start target capture", Action::Capture, self.target.is_some() && !self.capture_running()),
            text("Reconnect clients").size(19),
            text("A finite deauthentication burst can trigger a new handshake. Blank client MAC addresses all clients of this AP.").size(13).style(text::secondary),
            clients.wrap(),
            row![text_input("Client MAC (optional)", &self.station).on_input(Message::Station), text_input("Bursts", &self.count).on_input(Message::Count).width(85), self.action("Send deauth", Action::Deauth, self.capture_running())].spacing(10),
            text("Capture file").size(13), text_input("/path/to/capture.cap", &self.capture_path).on_input(Message::CapturePath),
            text("Stop capture to flush packets, then inspect the file. The tool output reports available handshakes; a file alone does not prove one was captured.").size(13).style(text::secondary),
            row![self.action("Inspect handshake", Action::Inspect, !self.capture_path.is_empty() && !self.radio_busy() && !self.offline_busy()), button("Continue to recovery →").on_press(Message::Page(Page::Recover)).style(button::text)].spacing(10),
        ].spacing(14).into()
    }

    fn recover(&self) -> Element<'_, Message> {
        let idle = !self.offline_busy() && !self.radio_busy();
        column![
            text("Recover a WPA/WPA2 PSK").size(28),
            text("Run a local dictionary against captured authentication data.").size(14).style(text::secondary),
            pick_list([Engine::Aircrack, Engine::Hashcat], Some(self.engine), Message::Engine),
            text("Capture file (.cap / .pcap / .pcapng)").size(13),
            text_input("/path/to/capture.cap", &self.capture_path).on_input(Message::CapturePath),
            row![self.action("Inspect handshake", Action::Inspect, idle && !self.capture_path.is_empty()), self.action("Convert for Hashcat", Action::Convert, idle && !self.capture_path.is_empty())].spacing(10),
            text("Hashcat input (.hc22000) · used by the GPU engine").size(13),
            text_input("Generated by conversion, or enter an existing hash file", &self.hash_path).on_input(Message::HashPath),
            text("Wordlist").size(13), text_input("/path/to/wordlist.txt", &self.wordlist).on_input(Message::Wordlist),
            self.action("Start dictionary recovery", Action::Crack, idle && !self.wordlist.is_empty()),
            text("WPA3-only SAE and enterprise authentication need different workflows. Hashcat processes every record in the supplied hash file.").size(13).style(text::secondary),
            text("SESSION ARTIFACTS").size(12).style(text::secondary),
            text(self.session.to_string_lossy().into_owned()).size(12).font(Font::MONOSPACE),
        ].spacing(12).into()
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
