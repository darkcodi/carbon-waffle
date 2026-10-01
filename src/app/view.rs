use super::{
    Action, App, Authorization, CommandActivity, Engine, Flow, Message, Network, Operation, Page,
    Panel, RecoveryMode, Tool, appearance as style,
};
use iced::{
    Border, Center, Element, Fill, Font, Length, Theme,
    widget::{self, button, column, container, pick_list, row, scrollable, text, text_input},
};
use std::collections::HashMap;

impl App {
    pub fn theme(&self) -> Theme {
        style::theme()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let tabs =
            [Flow::Capture, Flow::Recover]
                .into_iter()
                .fold(row![].spacing(6), |tabs, flow| {
                    let selected = self.flow == flow;
                    tabs.push(
                        button(
                            text(if flow == Flow::Capture {
                                "Capture"
                            } else {
                                "Recover"
                            })
                            .size(14),
                        )
                        .on_press_maybe((!self.closing).then_some(Message::Flow(flow)))
                        .padding([10, 24])
                        .style(move |_, status| style::choice(selected, status)),
                    )
                });
        let header = row![
            text("carbon").font(style::SEMIBOLD).size(21),
            text("waffle").color(style::MUTED).size(21),
            widget::space().width(Fill),
            tabs,
        ]
        .align_y(Center)
        .spacing(5);

        let screen = if self.flow == Flow::Recover {
            self.recover()
        } else {
            match self.page {
                Page::Elevate => self.elevate(),
                Page::Monitoring => self.monitoring(),
                Page::Discover => self.discover(),
                Page::Capture => self.capture(),
            }
        };
        let stage = scrollable(container(screen).center_x(Fill).padding(iced::Padding {
            top: if self.page == Page::Capture || self.flow == Flow::Recover {
                8.0
            } else {
                16.0
            },
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

        let mut layout = column![header].width(Fill).height(Fill).spacing(8);
        if self.flow == Flow::Capture {
            layout = layout.push(
                container(self.steps())
                    .center_x(Fill)
                    .padding(iced::Padding {
                        top: 28.0,
                        bottom: 8.0,
                        ..Default::default()
                    }),
            );
        } else {
            layout = layout.push(widget::space().height(12));
        }
        layout = layout.push(stage);
        if self.flow == Flow::Capture {
            layout = layout.push(self.navigation());
        }
        layout = layout
            .push(widget::rule::horizontal(1).style(style::divider))
            .push(footer);
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
            let available = self.can_select_page(page);
            let inactive_color = if available {
                style::MUTED
            } else {
                style::MUTED.scale_alpha(0.4)
            };
            let circle = container(
                text((index + 1).to_string())
                    .font(style::SEMIBOLD)
                    .size(24)
                    .color(if active {
                        style::BACKGROUND
                    } else {
                        inactive_color
                    }),
            )
            .center_x(64)
            .center_y(64)
            .style(move |_| container::Style {
                background: active.then_some(style::ACCENT.into()),
                border: Border {
                    color: if active {
                        style::ACCENT
                    } else if available {
                        style::LINE
                    } else {
                        style::LINE.scale_alpha(0.45)
                    },
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
                                inactive_color
                            }),
                    ]
                    .align_x(Center)
                    .spacing(12)
                    .width(118),
                )
                .padding([6, 0])
                .on_press_maybe(available.then_some(Message::Page(page)))
                .style(style::quiet),
            );
        }
        container(steps).max_width(820).width(Fill).into()
    }

    fn navigation(&self) -> Element<'_, Message> {
        let back = button(
            row![
                text("←").size(17),
                text("Back").size(14).font(style::SEMIBOLD)
            ]
            .spacing(16)
            .align_y(Center),
        )
        .width(120)
        .padding([12, 20])
        .on_press_maybe(self.can_go_back().then_some(Message::Back))
        .style(style::secondary);
        let next = button(
            row![
                text("Next").size(14).font(style::SEMIBOLD).width(Fill),
                text("→").size(17),
            ]
            .spacing(16)
            .align_y(Center),
        )
        .width(120)
        .padding([12, 20])
        .on_press_maybe(self.can_advance().then_some(Message::Next))
        .style(|theme, status| {
            if status == button::Status::Disabled {
                style::secondary(theme, status)
            } else {
                style::primary(theme, status)
            }
        });
        let mut navigation = row![].align_y(Center).width(Fill);
        if self.page.previous().is_some() {
            navigation = navigation.push(back);
        }
        navigation = navigation.push(widget::space().width(Fill));
        if self.page.next().is_some() {
            navigation = navigation.push(next);
        }
        container(navigation).padding([8, 14]).width(Fill).into()
    }

    fn elevate(&self) -> Element<'_, Message> {
        let ready = self.authorization == Authorization::Ready;
        let (label, hint) = match &self.authorization {
            Authorization::Idle => (
                "Elevate permissions",
                "Authorize once. Permissions last until you close the app.",
            ),
            Authorization::Pending => (
                "Waiting for authorization…",
                "Complete the authorization prompt on your desktop.",
            ),
            Authorization::Ready if self.demo => (
                "Demo permissions enabled",
                "Demo mode simulates commands and needs no authorization.",
            ),
            Authorization::Ready => (
                "Permissions elevated",
                "You're authorized for the rest of this session.",
            ),
            Authorization::Failed(error) => ("Elevate permissions", error.as_str()),
        };
        let permissions = column![
            button(
                text(label)
                    .size(14)
                    .font(style::SEMIBOLD)
                    .align_x(Center)
                    .width(Fill)
            )
            .on_press_maybe(
                (matches!(
                    self.authorization,
                    Authorization::Idle | Authorization::Failed(_)
                ) && !self.closing
                    && self.jobs.is_empty())
                .then_some(Message::Elevate)
            )
            .padding([14, 20])
            .width(Fill)
            .style(style::primary),
            text(hint).size(12).align_x(Center).width(Fill).color(
                if matches!(self.authorization, Authorization::Failed(_)) {
                    style::ERROR
                } else if ready {
                    style::ACCENT
                } else {
                    style::MUTED
                }
            ),
        ]
        .spacing(12);
        let mut dependencies = column![disclosure(
            "Required dependencies",
            self.panels.dependencies,
            Panel::Dependencies,
        )]
        .spacing(12)
        .width(Fill);
        if self.panels.dependencies {
            dependencies = dependencies.push(
                row![
                    widget::space().width(Fill),
                    button(text("Refresh").size(12))
                        .on_press_maybe(
                            (!self.radio_busy() && !self.closing).then_some(Message::Refresh),
                        )
                        .style(style::quiet)
                        .padding(0),
                ]
                .align_y(Center)
                .width(Fill),
            );
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
            dependencies =
                dependencies.push(container(tools).padding(20).width(Fill).style(style::card));
        }
        container(
            column![
                heading(
                    "Elevate permissions.",
                    "Get your session ready before working with your adapter."
                ),
                permissions,
                dependencies,
            ]
            .spacing(24)
            .width(Fill),
        )
        .width(Fill)
        .max_width(490)
        .into()
    }

    fn monitoring(&self) -> Element<'_, Message> {
        let idle = !self.radio_busy();
        let monitor = self
            .interface
            .as_ref()
            .is_some_and(|interface| interface.monitor);
        let changing_mode = self
            .jobs
            .iter()
            .any(|job| matches!(job.operation, Operation::Monitor { .. }));
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

        if changing_mode {
            content = content.push(container(self.spinner()).center_x(Fill).center_y(48));
        } else if monitor {
            content = content.push(
                self.action("Restore managed mode", Action::Restore, idle)
                    .width(Fill)
                    .style(style::quiet),
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
        container(content).width(Fill).max_width(490).into()
    }

    fn spinner(&self) -> Element<'_, Message> {
        let dots = (0..8).map(|index| {
            let angle = index as f32 * std::f32::consts::TAU / 8.0 - std::f32::consts::FRAC_PI_2;
            let age = (self.spinner_frame + 8 - index) % 8;
            let color = style::ACCENT.scale_alpha(1.0 - age as f32 / 8.0 * 0.85);
            let dot = container(widget::space())
                .width(4)
                .height(4)
                .style(move |_| container::Style {
                    background: Some(color.into()),
                    border: Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });
            container(dot)
                .padding(iced::Padding {
                    top: 10.0 + angle.sin() * 10.0,
                    left: 10.0 + angle.cos() * 10.0,
                    ..Default::default()
                })
                .width(Fill)
                .height(Fill)
                .into()
        });
        widget::stack(dots).width(24).height(24).into()
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
        let groups = self.survey.network_groups(&self.filter);
        let hidden = groups.iter().find(|group| group.ssid.is_empty());
        let hidden_count = hidden.map_or(0, |group| group.access_points.len());
        let ssid_count = groups.iter().filter(|group| !group.ssid.is_empty()).count();
        let count: usize = groups.iter().map(|group| group.access_points.len()).sum();
        // Count clients once per survey view, instead of scanning every station
        // again for every access point on each UI update.
        let mut client_counts = HashMap::new();
        for station in &self.survey.stations {
            *client_counts
                .entry(station.bssid.as_str())
                .or_insert(0usize) += 1;
        }
        for group in groups.iter().filter(|group| !group.ssid.is_empty()) {
            networks = networks.push(self.network_group(group, &client_counts, selectable));
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
                        } else if !self.filter.is_empty() {
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
                    "{ssid_count} SSIDs  ·  {count} BSSIDs{}{}",
                    if hidden_count > 0 {
                        format!("  ·  {hidden_count} hidden")
                    } else {
                        String::new()
                    },
                    if scan.is_some() { "  ·  scanning" } else { "" }
                ))
                .size(12)
                .color(style::MUTED),
            )
            .push(networks);
        if let Some(hidden) = hidden {
            content = content.push(disclosure(
                "Hidden networks",
                self.panels.hidden_networks,
                Panel::HiddenNetworks,
            ));
            if self.panels.hidden_networks {
                let hidden_networks = hidden.access_points.iter().fold(
                    column![].spacing(8).width(Fill),
                    |list, network| {
                        list.push(
                            self.network_entry(
                                network,
                                client_counts
                                    .get(network.bssid.as_str())
                                    .copied()
                                    .unwrap_or(0),
                                selectable,
                            ),
                        )
                    },
                );
                content = content.push(hidden_networks);
            }
        }
        container(content).max_width(780).width(Fill).into()
    }

    fn network_group<'a>(
        &'a self,
        group: &crate::model::NetworkGroup<'a>,
        client_counts: &HashMap<&str, usize>,
        selectable: bool,
    ) -> Element<'a, Message> {
        let open = self.expanded_networks.contains(group.ssid);
        let selected = self.target.as_ref().filter(|target| {
            group
                .access_points
                .iter()
                .any(|network| network.bssid == target.bssid)
        });
        let bssids = format!(
            "{} BSSID{}",
            group.access_points.len(),
            if group.access_points.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        let summary = if let Some(target) = selected {
            format!(
                "{bssids}  ·  selected {}  ·  CH {}",
                target.bssid, target.channel
            )
        } else {
            bssids
        };
        let header = button(
            row![
                column![
                    text(group.ssid).font(style::SEMIBOLD).size(16),
                    text(summary).size(11).color(style::MUTED),
                ]
                .spacing(6)
                .width(Fill),
                text(if open { "−" } else { "+" })
                    .size(20)
                    .width(20)
                    .align_x(Center),
            ]
            .spacing(16)
            .align_y(Center),
        )
        .on_press(Message::ToggleNetworkGroup(group.ssid.to_owned()))
        .padding(12)
        .width(Fill)
        .style(move |_, status| style::choice(selected.is_some(), status));
        let mut content = column![header].spacing(8).width(Fill);
        if open {
            let entries = group.access_points.iter().fold(
                column![].spacing(8).width(Fill),
                |list, network| {
                    list.push(
                        self.network_entry(
                            network,
                            client_counts
                                .get(network.bssid.as_str())
                                .copied()
                                .unwrap_or(0),
                            selectable,
                        ),
                    )
                },
            );
            content = content.push(container(entries).padding(iced::Padding {
                left: 20.0,
                ..Default::default()
            }));
        }
        content.into()
    }

    fn network_entry<'a>(
        &'a self,
        network: &'a Network,
        clients: usize,
        selectable: bool,
    ) -> Element<'a, Message> {
        let selected = self
            .target
            .as_ref()
            .is_some_and(|target| target.bssid == network.bssid);
        let identity = text(&network.bssid)
            .font(Font::MONOSPACE)
            .size(14)
            .width(Fill);
        button(
            row![
                text(if selected { "●" } else { "○" })
                    .size(21)
                    .color(if selected {
                        style::ACCENT
                    } else {
                        style::MUTED
                    }),
                identity,
                column![
                    text(format!("{} · {}", network.security, network.authentication)).size(12),
                    text(format!(
                        "CH {}   {}   {} clients",
                        network.channel,
                        if network.power == -1 {
                            "Unknown signal".into()
                        } else {
                            format!("{} dBm", network.power)
                        },
                        clients,
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
        .style(move |_, status| style::choice(selected, status))
        .into()
    }

    fn capture(&self) -> Element<'_, Message> {
        let capture = self
            .jobs
            .iter()
            .find(|job| matches!(job.operation, Operation::Capture { .. }));
        let checking = self
            .jobs
            .iter()
            .find(|job| matches!(job.operation, Operation::Inspect { .. }));
        let switching = matches!(self.pending, Some(Operation::Capture { .. }));
        let idle = !self.radio_busy() && !self.offline_busy() && self.pending.is_none();
        let can_record = self.target.is_some()
            && self
                .interface
                .as_ref()
                .is_some_and(|interface| interface.monitor)
            && !self.offline_busy()
            && self.jobs.iter().all(|job| {
                !job.operation.radio() || matches!(job.operation, Operation::Scan { .. })
            });
        let found =
            self.capture_check == Some(super::Inspection::Found) && idle && self.capture_available;
        let elapsed = self
            .capture_started
            .map_or(0, |start| start.elapsed().as_secs());
        let clients = self.target.as_ref().map_or(0, |target| {
            self.survey
                .stations
                .iter()
                .filter(|station| station.bssid == target.bssid)
                .count()
        });

        let (title, explanation, busy) = if switching {
            (
                "Preparing to record".to_string(),
                "Finishing discovery and switching to your selected network’s channel.",
                true,
            )
        } else if let Some(job) = capture {
            if job.stopping {
                (
                    "Saving your recording".into(),
                    if self.check_after_capture {
                        "The handshake check will start as soon as recording stops."
                    } else {
                        "Wait for the recording to finish saving before checking it."
                    },
                    true,
                )
            } else if !job.started {
                (
                    "Starting recording".into(),
                    "Keep a phone or laptop nearby. You’ll reconnect it once recording starts.",
                    true,
                )
            } else {
                (
                    format!("Recording · {:02}:{:02}", elapsed / 60, elapsed % 60),
                    "On a phone or laptop, turn Wi-Fi off and on, then reconnect to this network. Once it reconnects, stop and check the recording.",
                    true,
                )
            }
        } else if checking.is_some() {
            (
                "Checking for a handshake".into(),
                "Checking the saved recording for your selected network. This usually takes a moment.",
                true,
            )
        } else if found {
            (
                if self.demo {
                    "Demo handshake found"
                } else {
                    "Handshake found"
                }
                .into(),
                if self.demo {
                    "This is a simulated result. Open it in Recover to preview password recovery."
                } else {
                    "Your recording contains a handshake for this network. Open it in Recover now, or use the saved file later."
                },
                false,
            )
        } else if self.capture_check == Some(super::Inspection::NotFound) {
            (
                "No handshake found yet".into(),
                "Try another recording. Start first, reconnect a device to this Wi-Fi, then stop and check again.",
                false,
            )
        } else if self.capture_check == Some(super::Inspection::Unknown) {
            (
                "Couldn’t verify this recording".into(),
                "See Activity for the check details, then retry. You can also start a new recording.",
                false,
            )
        } else if self.capture_available {
            (
                "Recording ready to check".into(),
                "Check this recording for a handshake before moving to password recovery.",
                false,
            )
        } else if !self.capture_path.is_empty() {
            (
                "Capture file unavailable".into(),
                "The file is missing or empty. Choose a different file below, or start a new recording.",
                false,
            )
        } else {
            (
                "Ready to record".into(),
                "Start recording, then reconnect a phone or laptop to this Wi-Fi. Your recording is saved automatically; stop and check once the device reconnects.",
                false,
            )
        };

        let target: Element<'_, Message> = if let Some(target) = &self.target {
            column![
                text(target.label()).font(style::SEMIBOLD).size(16),
                text(format!(
                    "{}  ·  Channel {}{}",
                    target.bssid,
                    target.channel,
                    if self.capture_running() {
                        format!(
                            "  ·  {clients} {} seen",
                            if clients == 1 { "device" } else { "devices" }
                        )
                    } else {
                        String::new()
                    }
                ))
                .font(Font::MONOSPACE)
                .size(11)
                .color(style::MUTED),
            ]
            .spacing(5)
            .width(Fill)
            .into()
        } else {
            text("Select a network in Discover first.")
                .size(14)
                .width(Fill)
                .into()
        };
        let network = row![
            target,
            button(text("Change").size(12))
                .on_press_maybe(
                    (capture.is_none() && checking.is_none() && !switching)
                        .then_some(Message::Page(Page::Discover))
                )
                .padding([8, 4])
                .style(style::quiet),
        ]
        .spacing(12)
        .align_y(Center)
        .width(Fill);
        let indicator: Element<'_, Message> = if busy {
            self.spinner()
        } else {
            container(
                text(if found { "✓" } else { "○" })
                    .size(26)
                    .color(if found { style::ACCENT } else { style::MUTED }),
            )
            .center_x(36)
            .center_y(36)
            .into()
        };
        let mut body = column![
            container(
                row![indicator, text(title).size(23).font(style::SEMIBOLD)]
                    .spacing(10)
                    .align_y(Center)
            )
            .center_x(Fill),
            text(explanation)
                .size(14)
                .line_height(1.5)
                .color(style::MUTED)
                .align_x(Center)
                .width(Fill),
        ]
        .spacing(10)
        .align_x(Center)
        .width(Fill);

        if let Some(job) = capture {
            if job.started && !job.stopping {
                body = body.push(
                    button(
                        text("Stop & check")
                            .size(15)
                            .font(style::SEMIBOLD)
                            .align_x(Center)
                            .width(Fill),
                    )
                    .on_press_maybe(
                        (!self.closing && self.authorization == Authorization::Ready)
                            .then_some(Message::StopAndCheck),
                    )
                    .padding([15, 20])
                    .width(Fill)
                    .style(style::primary),
                );
            }
        } else if let Some(job) = checking {
            body = body.push(
                button(text("Cancel check").size(12))
                    .on_press_maybe((!job.stopping).then_some(Message::Stop(job.id)))
                    .style(style::quiet),
            );
        } else if !switching {
            if self.target.is_none() {
                body = body.push(
                    button(text("Choose a network").align_x(Center).width(Fill))
                        .on_press(Message::Page(Page::Discover))
                        .width(Fill)
                        .padding(15)
                        .style(style::primary),
                );
            } else if found {
                body = body.push(
                    button(
                        text("Open in Recover →")
                            .size(15)
                            .font(style::SEMIBOLD)
                            .align_x(Center)
                            .width(Fill),
                    )
                    .on_press_maybe(self.can_use_capture().then_some(Message::UseCapture))
                    .padding([15, 20])
                    .width(Fill)
                    .style(style::primary),
                );
            } else if self.capture_check == Some(super::Inspection::NotFound)
                || !self.capture_available
            {
                body = body.push(
                    self.action(
                        if self.capture_path.is_empty() {
                            "Start recording"
                        } else {
                            "Record again"
                        },
                        Action::Capture,
                        can_record,
                    )
                    .width(Fill),
                );
            } else {
                body = body.push(
                    self.action(
                        if self.capture_check == Some(super::Inspection::Unknown) {
                            "Retry check"
                        } else {
                            "Check handshake"
                        },
                        Action::Inspect,
                        idle && self.capture_available,
                    )
                    .width(Fill),
                );
            }
            if !can_record && !self.capture_available && self.target.is_some() {
                body = body.push(
                    text("Return to Monitoring and enable monitor mode to record.")
                        .size(12)
                        .color(style::MUTED),
                );
            }
        }
        let card = container(
            column![
                network,
                widget::rule::horizontal(1).style(style::divider),
                body
            ]
            .spacing(12),
        )
        .padding(16)
        .width(Fill)
        .style(move |theme| {
            let mut card = style::card(theme);
            if found {
                card.border.color = style::ACCENT.scale_alpha(0.6);
            }
            card
        });
        let mut content = column![card].spacing(10).width(Fill);

        if self.capture_running() {
            content = content.push(self.reconnect_options());
        }
        content = content.push(disclosure(
            if self.capture_path.is_empty() {
                "Use an existing capture instead"
            } else {
                "Recording file & other options"
            },
            self.panels.capture_file,
            Panel::CaptureFile,
        ));
        if self.panels.capture_file {
            let discovery_running = self
                .jobs
                .iter()
                .any(|job| matches!(job.operation, Operation::Scan { .. }));
            let editor = column![
                text("Capture file").size(12).color(style::MUTED),
                text_input("/path/to/capture.cap", &self.capture_path)
                    .on_input_maybe(idle.then_some(Message::CapturePath as fn(String) -> Message))
                    .padding(13)
                    .size(13)
                    .style(style::input),
                text(if idle {
                    "Choose an existing file, then check it for the selected network."
                } else if discovery_running {
                    "Stop discovery to choose and check an existing capture."
                } else {
                    "Your recording is being saved here. File changes are available once it stops."
                })
                .size(12)
                .color(style::MUTED),
            ]
            .spacing(8);
            let mut options = column![editor].spacing(14);
            if discovery_running {
                options = options.push(
                    button(text("Stop discovery").size(13))
                        .on_press(Message::StopAll)
                        .style(style::secondary)
                        .padding([10, 16]),
                );
            }
            if idle && !self.capture_path.is_empty() {
                options = options.push(
                    row![
                        self.action("Check again", Action::Inspect, self.capture_available)
                            .style(style::secondary)
                            .width(Fill),
                        self.action("New recording", Action::Capture, can_record)
                            .style(style::secondary)
                            .width(Fill),
                    ]
                    .spacing(12),
                );
            }
            content = content.push(options);
        }
        container(content).max_width(640).width(Fill).into()
    }

    fn reconnect_options(&self) -> Element<'_, Message> {
        let deauth = self
            .jobs
            .iter()
            .any(|job| matches!(job.operation, Operation::Deauth { .. }));
        let mut content = column![disclosure(
            "Need help reconnecting a device?",
            self.panels.reconnect,
            Panel::Reconnect
        )]
        .spacing(12);
        if self.panels.reconnect {
            let mut options = column![
                text("You can reconnect a device manually. Or send a brief disconnect request below so it can reconnect while recording continues.").size(13).line_height(1.5).color(style::MUTED),
                row![
                    field("Device MAC · empty means all devices", "AA:BB:CC:DD:EE:FF", &self.station, Message::Station),
                    container(field("Bursts", "5", &self.count, Message::Count)).width(90),
                ].spacing(12),
            ].spacing(14);
            if let Some(target) = &self.target {
                let mut clients = row![
                    button(text("All devices").size(11))
                        .on_press(Message::Station(String::new()))
                        .style(style::secondary)
                        .padding([6, 8])
                ]
                .spacing(6);
                for station in self
                    .survey
                    .stations
                    .iter()
                    .filter(|station| station.bssid == target.bssid)
                {
                    clients = clients.push(
                        button(text(&station.mac).font(Font::MONOSPACE).size(11))
                            .on_press(Message::Station(station.mac.clone()))
                            .style(style::secondary)
                            .padding([6, 8]),
                    );
                }
                options = options.push(clients.wrap());
            }
            options = options.push(self.action(if deauth { "Sending request…" } else { "Send disconnect request" }, Action::Deauth, self.capture_running() && !deauth).style(style::secondary).width(Fill))
                .push(text("This briefly interrupts Wi-Fi for the selected device, or all devices if left empty. Reconnection is not guaranteed.").size(12).color(style::MUTED));
            content = content.push(
                container(options)
                    .padding(20)
                    .width(Fill)
                    .style(style::card),
            );
        }
        content.width(Fill).into()
    }

    fn recover(&self) -> Element<'_, Message> {
        let offline = self.jobs.iter().find(|job| {
            matches!(
                job.operation,
                Operation::Crack { .. }
                    | Operation::Convert { .. }
                    | Operation::Inspect { .. }
                    | Operation::ReadCapture { .. }
            )
        });
        let idle = !self.offline_busy() && !self.radio_busy();
        let is_hashcat = self.engine == Engine::Hashcat;
        let pattern_mode = self.recovery_mode == RecoveryMode::Pattern;
        let needs_words = !pattern_mode
            || self
                .pattern_check
                .as_ref()
                .is_ok_and(|pattern| pattern.needs_words());
        let candidate_input_ready = (!pattern_mode || self.pattern_check.is_ok())
            && (!needs_words || !self.wordlist.is_empty());
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
                "Recover the password.",
                "Open a saved capture or hash file. No elevation or Wi-Fi adapter needed."
            ),
            engine,
        ]
        .spacing(8)
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
                                &self.recovery_capture_path,
                                Message::RecoveryCapturePath
                            ),
                            self.action(
                                "Convert capture",
                                Action::Convert,
                                idle && !self.recovery_capture_path.is_empty()
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
            content = content.push(
                row![
                    container(field(
                        "Capture file",
                        "/path/to/capture.cap",
                        &self.recovery_capture_path,
                        Message::RecoveryCapturePath
                    ))
                    .width(Fill),
                    self.action(
                        "Read capture",
                        Action::ReadCapture,
                        idle && !self.recovery_capture_path.trim().is_empty()
                    )
                    .style(style::secondary)
                    .padding([12, 14]),
                ]
                .spacing(10)
                .align_y(iced::Bottom),
            );
            if self.recovery_networks.is_empty() {
                content = content.push(field(
                    "Network BSSID · read the capture to fill this",
                    "AA:BB:CC:DD:EE:FF",
                    &self.recovery_bssid,
                    Message::RecoveryBssid,
                ));
            } else {
                let selected = self
                    .recovery_networks
                    .iter()
                    .find(|network| network.bssid.eq_ignore_ascii_case(&self.recovery_bssid));
                content = content.push(
                    column![
                        text("Network in this capture").size(12).color(style::MUTED),
                        pick_list(
                            self.recovery_networks.as_slice(),
                            selected,
                            Message::RecoveryNetwork
                        )
                        .placeholder("Choose a network")
                        .padding(12)
                        .text_size(13)
                        .style(style::select)
                        .width(Fill),
                    ]
                    .spacing(8),
                );
                if selected.is_some_and(|network| network.handshakes == 0) {
                    content = content.push(text("No handshake found for this network. Choose another network or capture file.").size(12).color(style::ERROR));
                }
            }
        }
        let modes = [RecoveryMode::Dictionary, RecoveryMode::Pattern]
            .into_iter()
            .fold(row![].spacing(8), |choices, mode| {
                let selected = self.recovery_mode == mode;
                choices.push(
                    button(
                        text(if mode == RecoveryMode::Dictionary {
                            "Dictionary"
                        } else {
                            "Pattern / regex"
                        })
                        .size(13)
                        .align_x(Center)
                        .width(Fill),
                    )
                    .on_press_maybe(idle.then_some(Message::RecoveryMode(mode)))
                    .padding([10, 16])
                    .width(Fill)
                    .style(move |_, status| style::choice(selected, status)),
                )
            });
        content = content.push(modes);
        if needs_words {
            content = content.push(
                column![
                    text(if pattern_mode {
                        "Source wordlist · one word per line"
                    } else {
                        "Wordlist · one complete password per line"
                    })
                    .size(12)
                    .color(style::MUTED),
                    text_input("/path/to/wordlist.txt", &self.wordlist)
                        .on_input_maybe(idle.then_some(Message::Wordlist as fn(String) -> Message))
                        .padding(12)
                        .size(14)
                        .style(style::input),
                ]
                .spacing(8),
            );
        }
        if pattern_mode {
            content = content.push(self.pattern_editor(idle));
        }
        if let Some(job) = offline {
            content = content.push(self.job_strip(job));
            if matches!(
                job.operation,
                Operation::Crack {
                    pattern: Some(_),
                    ..
                }
            ) && let Some((generated, total)) = &self.candidate_progress
            {
                content = content.push(
                    text(format!(
                        "{} / {total} candidates generated",
                        crate::pattern::format_count(u128::from(*generated))
                    ))
                    .size(12)
                    .color(style::MUTED),
                );
            }
        } else {
            let has_input = if is_hashcat {
                !self.hash_path.is_empty()
            } else {
                !self.recovery_capture_path.trim().is_empty()
                    && crate::model::valid_mac(&self.recovery_bssid)
                    && !self.recovery_networks.iter().any(|network| {
                        network.bssid.eq_ignore_ascii_case(&self.recovery_bssid)
                            && network.handshakes == 0
                    })
            };
            let start = self
                .action(
                    "Start recovery",
                    Action::Crack,
                    idle && has_input && candidate_input_ready,
                )
                .width(Fill);
            if pattern_mode {
                content = content.push(
                    row![
                        button(
                            text(if self.preview_cancel.is_some() {
                                "Reading wordlist…"
                            } else {
                                "Preview & count"
                            })
                            .size(13)
                        )
                        .on_press_maybe(
                            (idle && candidate_input_ready && self.preview_cancel.is_none())
                                .then_some(Message::PreviewPattern)
                        )
                        .padding([14, 16])
                        .style(style::secondary),
                        start,
                    ]
                    .spacing(10),
                );
            } else {
                content = content.push(start);
            }
        }
        if pattern_mode {
            if let Some(preview) = &self.pattern_preview {
                match preview {
                    Ok(preview) => {
                        let mut examples = column![
                            text(format!(
                                "{} candidate combinations",
                                crate::pattern::format_count(preview.total)
                            ))
                            .size(14)
                            .font(style::SEMIBOLD),
                            text(format!(
                                "{} source entries · 8–63 byte candidates",
                                crate::pattern::format_count(preview.words as u128)
                            ))
                            .size(12)
                            .color(style::MUTED),
                        ]
                        .spacing(7);
                        for sample in preview.samples.iter().take(3) {
                            examples = examples.push(text(sample).font(Font::MONOSPACE).size(12));
                        }
                        content = content.push(
                            container(examples)
                                .padding(14)
                                .width(Fill)
                                .style(style::card),
                        );
                    }
                    Err(error) => {
                        content = content.push(text(error).size(12).color(style::ERROR));
                    }
                }
            }
            content = content.push(self.pattern_help(idle));
            content = content.push(text("Candidates are generated on demand. No combined wordlist is saved; large combinations can still take a long time.").size(12).color(style::MUTED));
        }
        if self.radio_busy() {
            content = content.push(
                text("Stop the active Capture job before starting recovery.")
                    .size(12)
                    .color(style::MUTED),
            );
        }
        if let Some(error) = &self.save_error {
            content = content.push(text(error).size(12).color(style::ERROR));
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
                    "{} isn't installed. Add it to PATH, then refresh Required dependencies in Elevate.",
                    required_tool.name()
                ))
                .size(12)
                .color(style::ERROR),
            );
        }
        content = content.push(
            row![
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
                text(if is_hashcat { "Hashcat processes every record in the supplied hash file." } else { "Recovery requires a WPA/WPA2 PSK network. SAE and enterprise authentication use different workflows." }).size(12).color(style::MUTED),
            ].spacing(8));
        }
        container(content).max_width(590).width(Fill).into()
    }

    fn pattern_editor(&self, idle: bool) -> Element<'_, Message> {
        let mut editor = column![
            text("Password pattern").size(12).color(style::MUTED),
            text_input("{Word}{Word}[0-9]{3}", &self.pattern)
                .on_input_maybe(idle.then_some(Message::Pattern as fn(String) -> Message))
                .font(Font::MONOSPACE)
                .padding(12)
                .size(14)
                .style(style::input),
        ]
        .spacing(8)
        .width(Fill);
        if let Err(error) = &self.pattern_check {
            editor = editor.push(text(error).size(12).color(style::ERROR));
        }
        editor.into()
    }

    fn pattern_help(&self, idle: bool) -> Element<'_, Message> {
        let mut help = column![
            row![
                button(text("2 words + 3 digits").size(12))
                    .on_press_maybe(idle.then(|| Message::Pattern("{Word}{Word}[0-9]{3}".into())))
                    .padding([6, 10])
                    .style(style::secondary),
                button(text("5 words").size(12))
                    .on_press_maybe(idle.then(|| Message::Pattern("{word}{5}".into())))
                    .padding([6, 10])
                    .style(style::secondary),
                widget::space().width(Fill),
                button(
                    text(if self.panels.pattern_help {
                        "−  Syntax & examples"
                    } else {
                        "+  Syntax & examples"
                    })
                    .size(12)
                )
                .on_press(Message::Toggle(Panel::PatternHelp))
                .padding([6, 0])
                .style(style::quiet),
            ]
            .spacing(8)
            .align_y(Center),
        ]
        .spacing(8)
        .width(Fill);
        if self.panels.pattern_help {
            help = help.push(column![
                text("{word}  as written    {Word}  Capitalized\n{WORD}  UPPERCASE     {lower}  lowercase").font(Font::MONOSPACE).size(12),
                text("{word}{5} = five independent choices from your list. A word may be chosen more than once.").size(12).color(style::MUTED),
                text("Use [0-9] or \\d for a digit, {3} for exactly three, {1,5} for one to five, and (red|blue) for alternatives. Literal spaces and punctuation are preserved; escape regex punctuation with \\. For five words separated by hyphens: ({word}-){4}{word}.").size(12).color(style::MUTED),
                text("Repeats must be bounded; * and +, lookarounds, and backreferences are not supported. Character classes use printable ASCII; case tokens change A–Z / a–z. Only complete 8–63 byte passphrases are sent.").size(12).color(style::MUTED),
            ].spacing(8));
        }
        help.into()
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
                        "Starting…"
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
            (enabled
                && (!action.requires_authorization()
                    || self.authorization == Authorization::Ready)
                && !self.closing
                && self.pending.is_none())
            .then_some(Message::Run(action)),
        )
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
