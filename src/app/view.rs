use super::{
    Action, App, CommandActivity, Engine, Message, Operation, Page, Panel, Tool,
    appearance as style,
};
use iced::{
    Border, Center, Element, Fill, Font, Length, Theme,
    widget::{self, button, column, container, pick_list, row, scrollable, text, text_input},
};

impl App {
    pub fn theme(&self) -> Theme {
        style::theme()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let header = row![
            row![
                text("carbon").font(style::SEMIBOLD).size(21),
                text("waffle").color(style::MUTED).size(21)
            ]
            .spacing(5),
            widget::space().width(Fill),
            text(if self.demo {
                "Demo session"
            } else {
                "Live session"
            })
            .size(12)
            .color(if self.demo {
                style::MUTED
            } else {
                style::ACCENT
            }),
            button(
                text(if self.demo {
                    "Use live mode"
                } else {
                    "Try demo"
                })
                .size(12)
            )
            .on_press_maybe(
                (self.jobs.is_empty()
                    && self.owned_monitor.is_none()
                    && self.network_restore.is_none()
                    && !self.closing)
                    .then_some(Message::Demo)
            )
            .style(style::secondary)
            .padding([8, 12]),
        ]
        .align_y(Center)
        .spacing(14);

        let screen = match self.page {
            Page::Monitoring => self.monitoring(),
            Page::Discover => self.discover(),
            Page::Capture => self.capture(),
            Page::Recover => self.recover(),
        };
        let stage = scrollable(container(screen).center_x(Fill).padding(iced::Padding {
            top: 16.0,
            bottom: 8.0,
            left: 14.0,
            right: 14.0,
        }))
        .id("current-step")
        .width(Fill)
        .height(Length::FillPortion(3));

        let activity_label = if self.activity.is_empty() {
            "Activity".to_string()
        } else {
            format!("Activity  ·  {}", self.activity.len())
        };
        let mut footer = row![
            button(
                row![
                    text(if self.panels.activity { "−" } else { "+" }).size(17),
                    text(activity_label).size(13)
                ]
                .spacing(8)
                .align_y(Center)
            )
            .on_press(Message::Toggle(Panel::Activity))
            .style(style::quiet)
            .padding([8, 0]),
            widget::space().width(18),
            text(&self.status)
                .size(12)
                .color(if self.status_error {
                    style::ERROR
                } else {
                    style::MUTED
                })
                .width(Fill),
        ]
        .spacing(8)
        .align_y(Center)
        .width(Fill);
        if !self.jobs.is_empty() {
            footer = footer.push(
                button(text("Stop all").size(12))
                    .on_press(Message::StopAll)
                    .style(style::danger)
                    .padding([8, 12]),
            );
        }

        let mut layout = column![
            header,
            container(self.steps())
                .center_x(Fill)
                .padding(iced::Padding {
                    top: 28.0,
                    bottom: 8.0,
                    ..Default::default()
                }),
            stage,
            widget::rule::horizontal(1).style(style::divider),
            footer,
        ]
        .width(Fill)
        .height(Fill)
        .spacing(8);
        if self.panels.activity {
            layout = layout.push(self.activity_view());
        }
        container(layout)
            .padding([20, 28])
            .width(Fill)
            .height(Fill)
            .into()
    }

    fn steps(&self) -> Element<'_, Message> {
        let mut steps = row![].align_y(iced::Top).width(Fill);
        for (index, page) in Page::ALL.into_iter().enumerate() {
            if index > 0 {
                steps = steps.push(
                    container(widget::rule::horizontal(1).style(style::divider))
                        .padding(iced::Padding {
                            top: 38.0,
                            ..Default::default()
                        })
                        .width(Fill),
                );
            }
            let active = self.page == page;
            let circle = container(
                text((index + 1).to_string())
                    .font(style::SEMIBOLD)
                    .size(24)
                    .color(if active {
                        style::BACKGROUND
                    } else {
                        style::MUTED
                    }),
            )
            .center_x(64)
            .center_y(64)
            .style(move |_| container::Style {
                background: active.then_some(style::ACCENT.into()),
                border: Border {
                    color: if active { style::ACCENT } else { style::LINE },
                    width: 2.0,
                    radius: 32.0.into(),
                },
                ..Default::default()
            });
            steps = steps.push(
                button(
                    column![
                        circle,
                        text(page.label())
                            .size(14)
                            .font(if active {
                                style::SEMIBOLD
                            } else {
                                Font::DEFAULT
                            })
                            .color(if active {
                                style::FOREGROUND
                            } else {
                                style::MUTED
                            }),
                    ]
                    .align_x(Center)
                    .spacing(12)
                    .width(118),
                )
                .padding([6, 0])
                .on_press_maybe((!self.closing).then_some(Message::Page(page)))
                .style(style::quiet),
            );
        }
        container(steps).max_width(760).width(Fill).into()
    }

    fn monitoring(&self) -> Element<'_, Message> {
        let idle = !self.radio_busy();
        let monitor = self
            .interface
            .as_ref()
            .is_some_and(|interface| interface.monitor);
        let mode_job = self
            .jobs
            .iter()
            .find(|job| matches!(job.operation, Operation::Monitor { .. }));
        let adapter: Element<'_, Message> = if idle {
            pick_list(
                self.interfaces.clone(),
                self.interface.clone(),
                Message::Interface,
            )
            .placeholder("Select a wireless adapter")
            .padding(14)
            .text_size(15)
            .width(Fill)
            .style(style::select)
            .into()
        } else {
            container(
                text(
                    self.interface
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "Adapter in use".into()),
                )
                .size(15),
            )
            .padding(14)
            .width(Fill)
            .style(style::card)
            .into()
        };
        let mut content = column![
            heading(
                "Prepare your adapter.",
                "Enable monitor mode to listen for nearby networks."
            ),
            column![
                row![
                    text("Wireless adapter").size(12).color(style::MUTED),
                    widget::space().width(Fill),
                    button(text("Refresh").size(12))
                        .on_press_maybe(idle.then_some(Message::Refresh))
                        .style(style::quiet)
                        .padding(0),
                ]
                .align_y(Center),
                adapter,
                text(if monitor {
                    "Monitor mode is enabled. You're ready to discover."
                } else if self.interfaces.is_empty() {
                    "No wireless adapters found. Connect an adapter and refresh."
                } else {
                    "This adapter disconnects from Wi-Fi until you restore managed mode."
                })
                .size(12)
                .color(if monitor { style::ACCENT } else { style::MUTED }),
            ]
            .spacing(12),
        ]
        .spacing(30)
        .width(Fill);

        if let Some(job) = mode_job {
            content = content.push(self.job_strip(job));
        } else if monitor {
            content = content
                .push(self.next("Continue to Discover", Page::Discover, true))
                .push(
                    container(
                        self.action("Restore managed mode", Action::Restore, idle)
                            .style(style::quiet),
                    )
                    .center_x(Fill),
                );
        } else if self.network_restore.is_some() || self.owned_monitor.is_some() {
            content = content.push(
                self.action("Restore managed mode", Action::Restore, idle)
                    .width(Fill),
            );
        } else {
            content = content.push(
                self.action(
                    "Enable monitoring",
                    Action::Monitor,
                    idle && self.interface.is_some(),
                )
                .width(Fill),
            );
        }
        content = content.push(disclosure(
            "Adapter & tool details",
            self.panels.tools,
            Panel::Tools,
        ));
        if self.panels.tools {
            let tools = self
                .tools
                .iter()
                .fold(column![].spacing(10), |list, (tool, present)| {
                    list.push(
                        row![
                            text(tool.name()).font(Font::MONOSPACE).size(12).width(Fill),
                            text(if *present {
                                "Available"
                            } else {
                                "Not installed"
                            })
                            .size(12)
                            .color(if *present {
                                style::ACCENT
                            } else {
                                style::MUTED
                            }),
                        ]
                        .align_y(Center),
                    )
                });
            content = content.push(container(column![tools,
                self.action("Check interfering processes", Action::Check, idle).style(style::secondary).width(Fill),
                text("The selected adapter is temporarily released from NetworkManager. Other adapters and networking services stay running.").size(12).color(style::MUTED),
            ].spacing(18)).padding(20).width(Fill).style(style::card));
        }
        container(content).width(Fill).max_width(490).into()
    }

    fn discover(&self) -> Element<'_, Message> {
        let scan = self
            .jobs
            .iter()
            .find(|job| matches!(job.operation, Operation::Scan { .. }));
        let monitor = self
            .interface
            .as_ref()
            .is_some_and(|interface| interface.monitor);
        let selectable = !self.radio_busy()
            || self.jobs.iter().all(|job| {
                !job.operation.radio() || matches!(job.operation, Operation::Scan { .. })
            });
        let filter = self.filter.to_lowercase();
        let mut content = column![heading(
            "Choose a network.",
            "Discover nearby access points and select one for this session."
        )]
        .spacing(12)
        .width(Fill);
        if !monitor {
            content = content.push(
                row![
                    text("Enable monitoring before you scan.")
                        .size(13)
                        .color(style::MUTED)
                        .width(Fill),
                    link("Go to Monitoring", Message::Page(Page::Monitoring)),
                ]
                .align_y(Center),
            );
        }
        let scan_control: Element<'_, Message> = if let Some(job) = scan {
            button(
                text(if job.stopping {
                    "Stopping…"
                } else {
                    "Stop scan"
                })
                .size(13),
            )
            .on_press_maybe((!job.stopping).then_some(Message::Stop(job.id)))
            .style(style::secondary)
            .padding([12, 18])
            .into()
        } else {
            self.action("Scan networks", Action::Scan, monitor && !self.radio_busy())
                .into()
        };
        content = content.push(
            row![
                text_input("Filter by name or BSSID", &self.filter)
                    .on_input(Message::Filter)
                    .padding(12)
                    .size(14)
                    .style(style::input),
                scan_control,
            ]
            .spacing(10)
            .align_y(Center),
        );

        let mut networks = column![].spacing(8).width(Fill);
        let mut count = 0;
        for network in self.survey.networks.iter().filter(|network| {
            network.ssid.to_lowercase().contains(&filter)
                || network.bssid.to_lowercase().contains(&filter)
        }) {
            count += 1;
            let selected = self
                .target
                .as_ref()
                .is_some_and(|target| target.bssid == network.bssid);
            let clients = self
                .survey
                .stations
                .iter()
                .filter(|station| station.bssid == network.bssid)
                .count();
            networks = networks.push(
                button(
                    row![
                        text(if selected { "●" } else { "○" })
                            .size(21)
                            .color(if selected {
                                style::ACCENT
                            } else {
                                style::MUTED
                            }),
                        column![
                            text(network.label()).font(style::SEMIBOLD).size(16),
                            text(&network.bssid)
                                .font(Font::MONOSPACE)
                                .size(11)
                                .color(style::MUTED),
                        ]
                        .spacing(6)
                        .width(Fill),
                        column![
                            text(format!("{} · {}", network.security, network.authentication))
                                .size(12),
                            text(format!(
                                "CH {}   {}   {} clients",
                                network.channel,
                                if network.power == -1 {
                                    "Unknown signal".into()
                                } else {
                                    format!("{} dBm", network.power)
                                },
                                clients
                            ))
                            .size(11)
                            .color(style::MUTED),
                        ]
                        .spacing(6)
                        .align_x(iced::Right),
                    ]
                    .spacing(16)
                    .align_y(Center),
                )
                .on_press_maybe(selectable.then(|| Message::Select(network.bssid.clone())))
                .padding(12)
                .width(Fill)
                .style(move |_, status| style::choice(selected, status)),
            );
        }
        if count == 0 {
            networks = networks.push(
                container(
                    column![
                        text(if self.survey.networks.is_empty() {
                            "No networks yet"
                        } else {
                            "No matching networks"
                        })
                        .size(17)
                        .font(style::SEMIBOLD),
                        text(if scan.is_some() {
                            "Listening for access points. Results will appear here."
                        } else if !filter.is_empty() {
                            "Try a different name or BSSID."
                        } else {
                            "Start a scan to see what's nearby."
                        })
                        .size(13)
                        .color(style::MUTED),
                    ]
                    .align_x(Center)
                    .spacing(10),
                )
                .center_x(Fill)
                .padding([36, 20])
                .style(style::card),
            );
        }
        content = content
            .push(
                text(format!(
                    "{count} networks{}",
                    if scan.is_some() { "  ·  scanning" } else { "" }
                ))
                .size(12)
                .color(style::MUTED),
            )
            .push(networks)
            .push(
                row![
                    link("Back", Message::Page(Page::Monitoring)),
                    widget::space().width(Fill),
                    self.next("Continue to Capture", Page::Capture, self.target.is_some()),
                ]
                .align_y(Center),
            );
        container(content).max_width(780).width(Fill).into()
    }

    fn capture(&self) -> Element<'_, Message> {
        let capture = self
            .jobs
            .iter()
            .find(|job| matches!(job.operation, Operation::Capture { .. }));
        let deauth = self
            .jobs
            .iter()
            .any(|job| matches!(job.operation, Operation::Deauth { .. }));
        let idle = !self.radio_busy() && !self.offline_busy();
        let mut content = column![
            heading(
                "Listen for a handshake.",
                "Record authentication traffic from your selected network."
            ),
            self.target_summary(),
        ]
        .spacing(20)
        .width(Fill);
        if let Some(job) = capture {
            content = content.push(self.job_strip(job));
        } else {
            content = content.push(
                self.action(
                    "Start capture",
                    Action::Capture,
                    self.target.is_some()
                        && self
                            .interface
                            .as_ref()
                            .is_some_and(|interface| interface.monitor)
                        && self.jobs.iter().all(|job| {
                            !job.operation.radio()
                                || matches!(job.operation, Operation::Scan { .. })
                        })
                        && self.pending.is_none(),
                )
                .width(Fill),
            );
        }

        content = content.push(disclosure(
            "Reconnect a client",
            self.panels.reconnect,
            Panel::Reconnect,
        ));
        if self.panels.reconnect {
            let mut reconnect = column![
                text("Send a finite deauthentication burst to trigger a new handshake.")
                    .size(13)
                    .color(style::MUTED),
                row![
                    field(
                        "Client MAC · blank for all clients",
                        "AA:BB:CC:DD:EE:FF",
                        &self.station,
                        Message::Station
                    ),
                    container(field("Bursts", "5", &self.count, Message::Count)).width(80),
                ]
                .spacing(12),
            ]
            .spacing(14);
            if let Some(target) = &self.target {
                let clients = self
                    .survey
                    .stations
                    .iter()
                    .filter(|station| station.bssid == target.bssid)
                    .fold(row![].spacing(6), |clients, station| {
                        clients.push(
                            button(text(&station.mac).font(Font::MONOSPACE).size(11))
                                .on_press(Message::Station(station.mac.clone()))
                                .style(style::secondary)
                                .padding([6, 8]),
                        )
                    });
                reconnect = reconnect.push(clients.wrap());
            }
            reconnect = reconnect.push(
                self.action(
                    if deauth {
                        "Sending…"
                    } else {
                        "Send deauth burst"
                    },
                    Action::Deauth,
                    self.capture_running() && !deauth,
                )
                .style(style::secondary)
                .width(Fill),
            );
            content = content.push(
                container(reconnect)
                    .padding(20)
                    .width(Fill)
                    .style(style::card),
            );
        }
        content = content.push(disclosure(
            "Capture file & inspection",
            self.panels.capture_file,
            Panel::CaptureFile,
        ));
        if self.panels.capture_file {
            content = content.push(column![
                field("Capture file", "/path/to/capture.cap", &self.capture_path, Message::CapturePath),
                self.action("Inspect handshake", Action::Inspect, idle && !self.capture_path.is_empty()).style(style::secondary),
                text("Stop recording before inspection. The command output reports whether a usable handshake was captured.").size(12).color(style::MUTED),
            ].spacing(12));
        }
        content = content.push(
            row![
                link("Back", Message::Page(Page::Discover)),
                widget::space().width(Fill),
                self.next(
                    "Continue to Recover",
                    Page::Recover,
                    !self.capture_path.is_empty() && !self.radio_busy()
                ),
            ]
            .align_y(Center),
        );
        container(content).max_width(590).width(Fill).into()
    }

    fn recover(&self) -> Element<'_, Message> {
        let offline = self.jobs.iter().find(|job| {
            matches!(
                job.operation,
                Operation::Crack { .. } | Operation::Convert { .. } | Operation::Inspect { .. }
            )
        });
        let idle = !self.offline_busy() && !self.radio_busy();
        let is_hashcat = self.engine == Engine::Hashcat;
        let engine = [Engine::Aircrack, Engine::Hashcat].into_iter().fold(
            row![].spacing(8),
            |choices, engine| {
                let selected = self.engine == engine;
                choices.push(
                    button(
                        text(if engine == Engine::Aircrack {
                            "Aircrack-ng"
                        } else {
                            "Hashcat"
                        })
                        .size(13)
                        .align_x(Center),
                    )
                    .on_press_maybe(idle.then_some(Message::Engine(engine)))
                    .padding([12, 18])
                    .width(Fill)
                    .style(move |_, status| style::choice(selected, status)),
                )
            },
        );
        let mut content = column![
            heading(
                "Test your wordlist.",
                "Recover a WPA/WPA2 password from captured authentication data."
            ),
            engine,
        ]
        .spacing(14)
        .width(Fill);
        if is_hashcat {
            content = content
                .push(field(
                    "Hash file",
                    "/path/to/handshake.hc22000",
                    &self.hash_path,
                    Message::HashPath,
                ))
                .push(disclosure(
                    "Convert a capture for Hashcat",
                    self.panels.conversion,
                    Panel::Conversion,
                ));
            if self.panels.conversion {
                content = content.push(
                    container(
                        column![
                            field(
                                "Capture file",
                                "/path/to/capture.cap",
                                &self.capture_path,
                                Message::CapturePath
                            ),
                            self.action(
                                "Convert capture",
                                Action::Convert,
                                idle && !self.capture_path.is_empty()
                            )
                            .style(style::secondary)
                            .width(Fill),
                        ]
                        .spacing(12),
                    )
                    .padding(18)
                    .width(Fill)
                    .style(style::card),
                );
            }
        } else {
            content = content.push(field(
                "Capture file",
                "/path/to/capture.cap",
                &self.capture_path,
                Message::CapturePath,
            ));
            let selected = self
                .target
                .as_ref()
                .map_or("Select a target in Discover".into(), |network| {
                    format!("Target: {}", network.label())
                });
            content = content.push(
                row![
                    text(selected).size(12).color(style::MUTED).width(Fill),
                    link("Change", Message::Page(Page::Discover)),
                    self.action(
                        "Inspect",
                        Action::Inspect,
                        idle && self.target.is_some() && !self.capture_path.is_empty()
                    )
                    .style(style::quiet)
                    .padding([6, 4])
                    .width(76),
                ]
                .align_y(Center),
            );
        }
        content = content.push(field(
            "Wordlist",
            "/path/to/wordlist.txt",
            &self.wordlist,
            Message::Wordlist,
        ));
        if let Some(job) = offline {
            content = content.push(self.job_strip(job));
        } else {
            let has_input = if is_hashcat {
                !self.hash_path.is_empty()
            } else {
                !self.capture_path.is_empty()
                    && self
                        .target
                        .as_ref()
                        .is_some_and(|target| target.supports_dictionary())
            };
            content = content.push(
                self.action(
                    "Start recovery",
                    Action::Crack,
                    idle && has_input && !self.wordlist.is_empty(),
                )
                .width(Fill),
            );
        }
        let required_tool = if is_hashcat {
            Tool::Hashcat
        } else {
            Tool::Aircrack
        };
        if !self.demo
            && self
                .tools
                .iter()
                .any(|(tool, present)| *tool == required_tool && !present)
        {
            content = content.push(
                text(format!(
                    "{} isn't installed. Add it to PATH, then refresh tools in Monitoring.",
                    required_tool.name()
                ))
                .size(12)
                .color(style::ERROR),
            );
        }
        content = content.push(
            row![
                link("Back to Capture", Message::Page(Page::Capture)),
                widget::space().width(Fill),
                link(
                    if self.panels.session {
                        "Hide session details"
                    } else {
                        "Session details"
                    },
                    Message::Toggle(Panel::Session)
                ),
            ]
            .align_y(Center),
        );
        if self.panels.session {
            content = content.push(column![
                text("Files are saved locally in").size(12).color(style::MUTED),
                text(self.session.to_string_lossy().into_owned()).size(12).font(Font::MONOSPACE),
                text(if is_hashcat { "Hashcat processes every record in the supplied hash file." } else { "Dictionary recovery requires a WPA/WPA2 PSK network. SAE and enterprise authentication use different workflows." }).size(12).color(style::MUTED),
            ].spacing(8));
        }
        container(content).max_width(590).width(Fill).into()
    }

    fn target_summary(&self) -> Element<'_, Message> {
        let mut summary = row![].spacing(16).align_y(Center);
        if let Some(target) = &self.target {
            summary = summary.push(
                column![
                    text(target.label()).font(style::SEMIBOLD).size(17),
                    text(format!(
                        "{}   ·   CH {}   ·   {}",
                        target.bssid, target.channel, target.security
                    ))
                    .font(Font::MONOSPACE)
                    .size(11)
                    .color(style::MUTED),
                ]
                .spacing(7)
                .width(Fill),
            );
        } else {
            summary = summary.push(text("Choose a network to capture.").size(14).width(Fill));
        }
        summary = summary.push(link("Change", Message::Page(Page::Discover)));
        container(summary)
            .padding(20)
            .width(Fill)
            .style(style::card)
            .into()
    }

    fn job_strip<'a>(&self, job: &'a crate::runner::Job) -> Element<'a, Message> {
        container(
            row![
                text("●").size(12).color(style::ACCENT),
                column![
                    text(job.operation.label()).size(14).font(style::SEMIBOLD),
                    text(if job.stopping {
                        "Stopping…"
                    } else if job.started {
                        "Running"
                    } else {
                        "Starting · authorization may be required"
                    })
                    .size(12)
                    .color(style::MUTED),
                ]
                .spacing(5)
                .width(Fill),
                button(text("Stop").size(13))
                    .on_press_maybe((!job.stopping).then_some(Message::Stop(job.id)))
                    .style(style::secondary)
                    .padding([10, 16]),
            ]
            .spacing(14)
            .align_y(Center),
        )
        .padding(18)
        .width(Fill)
        .style(style::card)
        .into()
    }

    fn action<'a>(
        &self,
        label: &'a str,
        action: Action,
        enabled: bool,
    ) -> widget::Button<'a, Message> {
        button(
            text(label)
                .size(14)
                .font(style::SEMIBOLD)
                .align_x(Center)
                .width(Fill),
        )
        .width(190)
        .padding([14, 20])
        .on_press_maybe(
            (enabled && !self.closing && self.pending.is_none()).then_some(Message::Run(action)),
        )
        .style(style::primary)
    }

    fn next<'a>(&self, label: &'a str, page: Page, enabled: bool) -> widget::Button<'a, Message> {
        button(
            row![
                text(label).size(14).font(style::SEMIBOLD).width(Fill),
                text("→").size(17)
            ]
            .spacing(24)
            .align_y(Center),
        )
        .padding([14, 20])
        .on_press_maybe((enabled && !self.closing).then_some(Message::Page(page)))
        .style(style::primary)
    }

    fn activity_view(&self) -> Element<'_, Message> {
        let heading = row![
            text("Command activity").size(12).color(style::MUTED),
            widget::space().width(Fill),
            button(text("Clear completed").size(12))
                .on_press_maybe(
                    self.activity
                        .iter()
                        .any(|activity| !self.jobs.iter().any(|job| job.id == activity.id))
                        .then_some(Message::ClearActivity)
                )
                .style(style::quiet),
        ]
        .width(Fill)
        .align_y(Center);
        let panels: Element<'_, Message> = if self.activity.is_empty() {
            container(
                text("Your commands will appear here, each with its own output.")
                    .size(13)
                    .color(style::MUTED),
            )
            .center_x(Fill)
            .center_y(Fill)
            .style(style::console)
            .into()
        } else {
            let entries = widget::keyed_column(
                self.activity
                    .iter()
                    .rev()
                    .map(|activity| (activity.id, self.command_panel(activity))),
            )
            .width(Fill)
            .spacing(10)
            .padding(iced::Padding {
                right: 14.0,
                ..Default::default()
            });
            scrollable(entries)
                .id("activity-history")
                .width(Fill)
                .height(Fill)
                .into()
        };
        column![heading, panels]
            .width(Fill)
            .height(Length::FillPortion(2))
            .spacing(6)
            .into()
    }

    fn command_panel<'a>(&'a self, activity: &'a CommandActivity) -> Element<'a, Message> {
        let job = self.jobs.iter().find(|job| job.id == activity.id);
        let state = job.map_or(activity.outcome.as_str(), |job| {
            if job.stopping {
                "Stopping…"
            } else if job.started {
                "Running"
            } else {
                "Starting…"
            }
        });
        let output = activity
            .output
            .iter()
            .fold(column![].width(Fill).spacing(3), |lines, line| {
                lines.push(text(line).font(Font::MONOSPACE).size(12).width(Fill))
            });
        let mut header = row![
            text(format!("{}  ·  {}", activity.id, activity.label))
                .size(13)
                .font(style::SEMIBOLD)
                .width(Fill),
            text(state).size(12).color(if job.is_some() {
                style::ACCENT
            } else {
                style::MUTED
            }),
        ]
        .width(Fill)
        .spacing(12)
        .align_y(Center);
        if let Some(job) = job {
            header = header.push(
                button(text("Stop").size(12))
                    .on_press_maybe((!job.stopping).then_some(Message::Stop(job.id)))
                    .style(style::quiet),
            );
        }
        header = header.push(
            button(
                text(if self.copied_activity == Some(activity.id) {
                    "Copied!"
                } else {
                    "Copy"
                })
                .size(12),
            )
            .on_press(Message::CopyActivity(activity.id))
            .style(style::secondary)
            .padding([6, 12]),
        );
        container(
            column![
                header,
                scrollable(
                    column![
                        text(format!("$ {}", activity.command))
                            .font(Font::MONOSPACE)
                            .size(12)
                            .color(style::ACCENT)
                            .width(Fill),
                        output
                    ]
                    .width(Fill)
                    .spacing(10)
                    .padding(iced::Padding {
                        right: 14.0,
                        ..Default::default()
                    })
                )
                .id(format!("command-output-{}", activity.id))
                .anchor_bottom()
                .width(Fill)
                .height(132),
            ]
            .width(Fill)
            .spacing(12),
        )
        .padding(16)
        .width(Fill)
        .style(style::console)
        .into()
    }
}

fn heading(title: &'static str, subtitle: &'static str) -> Element<'static, Message> {
    column![
        text(title)
            .size(29)
            .font(style::SEMIBOLD)
            .align_x(Center)
            .width(Fill),
        text(subtitle)
            .size(14)
            .color(style::MUTED)
            .align_x(Center)
            .width(Fill),
    ]
    .spacing(10)
    .width(Fill)
    .into()
}

fn field<'a>(
    label: &'static str,
    placeholder: &'static str,
    value: &'a str,
    on_input: fn(String) -> Message,
) -> Element<'a, Message> {
    column![
        text(label).size(12).color(style::MUTED),
        text_input(placeholder, value)
            .on_input(on_input)
            .padding(13)
            .size(14)
            .style(style::input),
    ]
    .spacing(8)
    .width(Fill)
    .into()
}

fn link<'a>(label: &'a str, message: Message) -> widget::Button<'a, Message> {
    button(text(label).size(12))
        .on_press(message)
        .padding([8, 4])
        .style(style::quiet)
}

fn disclosure(label: &'static str, open: bool, panel: Panel) -> Element<'static, Message> {
    button(
        row![
            text(label).size(13).width(Fill),
            text(if open { "−" } else { "+" }).size(17)
        ]
        .align_y(Center),
    )
    .on_press(Message::Toggle(panel))
    .style(style::quiet)
    .width(Fill)
    .padding([6, 0])
    .into()
}
