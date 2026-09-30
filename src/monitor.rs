//! Adapter-scoped handoff between NetworkManager and airmon-ng.
use crate::{
    command::Tool,
    model::{Interface, valid_interface},
    runner::Event,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRestore {
    pub interface: String,
    pub phy: String,
}

impl NetworkRestore {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_interface(&self.interface)
            || !self.phy.starts_with("phy")
            || self.phy.len() <= 3
            || !self.phy[3..].bytes().all(|b| b.is_ascii_digit())
        {
            return Err("Invalid adapter restoration state.".into());
        }
        Ok(())
    }
}

pub struct Output {
    pub code: Option<i32>,
    pub cancelled: bool,
    pub stdout: String,
}

impl Output {
    fn require_success(&self, command: &str) -> Result<(), String> {
        if self.cancelled {
            return Err("Monitoring setup was cancelled.".into());
        }
        if self.code != Some(0) {
            return Err(format!(
                "{command} failed (exit {:?}); see command output.",
                self.code
            ));
        }
        Ok(())
    }
}

pub trait Host {
    fn interfaces(&self) -> Vec<Interface>;
    fn has_nmcli(&self) -> bool;
    fn cancelled(&self) -> bool;
    /// Cleanup commands ignore the requested cancellation, but remain bounded.
    fn run(&self, tool: Tool, args: &[&str], cleanup: bool) -> Result<Output, String>;
    fn emit(&self, event: Event);
}

fn managed(host: &impl Host, interface: &str, cleanup: bool) -> Result<Option<bool>, String> {
    let output = host.run(
        Tool::Nmcli,
        &[
            "--wait",
            "10",
            "--get-values",
            "GENERAL.NM-MANAGED",
            "device",
            "show",
            interface,
        ],
        cleanup,
    )?;
    if output.cancelled {
        return Err("Monitoring setup was cancelled.".into());
    }
    // NetworkManager can be absent, or not know this device. Neither requires
    // changing its configuration; other errors must not be mistaken for 'no'.
    if matches!(output.code, Some(8 | 10)) {
        return Ok(None);
    }
    output.require_success("NetworkManager inspection")?;
    match output.stdout.trim() {
        "yes" => Ok(Some(true)),
        "no" => Ok(Some(false)),
        _ => Err("NetworkManager returned an unrecognized managed state.".into()),
    }
}

fn set_managed(
    host: &impl Host,
    interface: &str,
    value: bool,
    cleanup: bool,
) -> Result<(), String> {
    host.run(
        Tool::Nmcli,
        &[
            "--wait",
            "10",
            "device",
            "set",
            interface,
            "managed",
            if value { "yes" } else { "no" },
        ],
        cleanup,
    )?
    .require_success("NetworkManager adapter handoff")?;
    if managed(host, interface, cleanup)? != Some(value) {
        return Err("NetworkManager did not apply the requested adapter state.".into());
    }
    Ok(())
}

fn restore_network(host: &impl Host, restore: &NetworkRestore) -> Result<(), String> {
    if !host.has_nmcli() {
        return Err("nmcli is required to return this adapter to NetworkManager.".into());
    }
    let interfaces = host.interfaces();
    let mut candidates = interfaces
        .iter()
        .filter(|iface| iface.phy == restore.phy && !iface.monitor);
    let interface = if let Some(original) = interfaces
        .iter()
        .find(|iface| iface.name == restore.interface && iface.phy == restore.phy && !iface.monitor)
    {
        original
    } else {
        let candidate = candidates.next().ok_or(
            "No managed-mode interface found on the original PHY; restore the adapter first.",
        )?;
        if candidates.next().is_some() {
            return Err("Multiple restored interfaces found on the original PHY; cannot choose one automatically.".into());
        }
        candidate
    };
    host.emit(Event::Line(format!(
        "Returning {} to NetworkManager.",
        interface.name
    )));
    set_managed(host, &interface.name, true, true)?;
    host.emit(Event::NetworkRestore(None));
    Ok(())
}

pub fn enable(host: &impl Host, name: &str) -> Result<(), String> {
    let before = host.interfaces();
    let original = before
        .iter()
        .find(|iface| iface.name == name)
        .ok_or("The selected adapter no longer exists. Refresh adapters.")?;
    if original.monitor {
        return Err("The selected adapter is already in monitor mode.".into());
    }
    let mut restore = None;
    let result: Result<(), String> = (|| {
        if host.cancelled() {
            return Err("Monitoring setup was cancelled.".into());
        }
        if host.has_nmcli() {
            if managed(host, name, false)? == Some(true) {
                let state = NetworkRestore {
                    interface: name.into(),
                    phy: original.phy.clone(),
                };
                // Record the obligation in the GUI before the first mutation.
                host.emit(Event::NetworkRestore(Some(state.clone())));
                restore = Some(state);
                host.emit(Event::Line(format!(
                    "Releasing {name} from NetworkManager for this session."
                )));
                set_managed(host, name, false, false)?;
            }
        } else {
            host.emit(Event::Line(
                "nmcli is unavailable; the adapter must already be free of network management."
                    .into(),
            ));
        }
        if host.cancelled() {
            return Err("Monitoring setup was cancelled.".into());
        }
        host.run(Tool::Airmon, &["start", name], false)?
            .require_success("airmon-ng start")?;
        if host.cancelled() {
            return Err("Monitoring setup was cancelled.".into());
        }
        let found = host.interfaces().into_iter().find(|iface| {
            iface.phy == original.phy
                && iface.monitor
                && !before
                    .iter()
                    .any(|old| old.name == iface.name && old.monitor)
        });
        let interface = found.ok_or("airmon-ng did not create a monitor interface on the selected PHY. Another network manager or supplicant may still own it; see the command output.")?;
        if host.cancelled() {
            return Err("Monitoring setup was cancelled.".into());
        }
        host.emit(Event::MonitorReady(interface));
        Ok(())
    })();
    if let Err(error) = result {
        host.emit(Event::Line(error.clone()));
        host.emit(Event::Line(
            "Setup did not complete; restoring the adapter's previous management state.".into(),
        ));
        let mut rollback_errors = vec![];
        for iface in host.interfaces().into_iter().filter(|iface| {
            iface.phy == original.phy
                && iface.monitor
                && !before
                    .iter()
                    .any(|old| old.name == iface.name && old.monitor)
        }) {
            if let Err(error) = stop_interface(host, &iface.name, true) {
                rollback_errors.push(error);
            }
        }
        if rollback_errors.is_empty()
            && let Some(restore) = &restore
            && let Err(error) = restore_network(host, restore)
        {
            rollback_errors.push(error);
        }
        if !rollback_errors.is_empty() {
            return Err(format!(
                "{error} Cleanup also failed: {}. Use Restore managed mode to retry.",
                rollback_errors.join("; ")
            ));
        }
        return if host.cancelled() { Ok(()) } else { Err(error) };
    }
    Ok(())
}

fn stop_interface(host: &impl Host, name: &str, cleanup: bool) -> Result<(), String> {
    let output = host.run(Tool::Airmon, &["stop", name], cleanup)?;
    // airmon may claim success despite leaving a monitor interface behind.
    if host
        .interfaces()
        .iter()
        .any(|iface| iface.name == name && iface.monitor)
    {
        return Err(format!(
            "{name} is still in monitor mode after airmon-ng stop."
        ));
    }
    if let Err(error) = output.require_success("airmon-ng stop") {
        host.emit(Event::Line(format!(
            "{error} The monitor interface is gone; continuing restoration."
        )));
    }
    Ok(())
}

pub fn disable(
    host: &impl Host,
    name: &str,
    restore: Option<&NetworkRestore>,
) -> Result<(), String> {
    let interfaces = host.interfaces();
    if let Some(iface) = interfaces
        .iter()
        .find(|iface| iface.name == name && iface.monitor)
    {
        if restore.is_some_and(|restore| restore.phy != iface.phy) {
            return Err("The monitor interface no longer belongs to the original adapter.".into());
        }
        // Finishing restoration is preferable to leaving a half-restored radio
        // when Stop is pressed or the GUI closes its control pipe.
        stop_interface(host, name, true)?;
    } else if restore.is_none() {
        return Err("The monitor interface no longer exists. Refresh adapters.".into());
    }
    if let Some(restore) = restore {
        restore_network(host, restore)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeHost {
        devices: RefCell<Vec<Interface>>,
        managed: Cell<bool>,
        cancel: Cell<bool>,
        no_monitor: bool,
        start_fails_after_creation: bool,
        stop_reports_failure: bool,
        cancel_after_release: bool,
        restore_fails: bool,
        calls: RefCell<Vec<(Tool, Vec<String>, bool)>>,
        events: RefCell<Vec<Event>>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                devices: RefCell::new(vec![
                    Interface {
                        name: "wlp4s0".into(),
                        phy: "phy0".into(),
                        monitor: false,
                    },
                    Interface {
                        name: "wlan1".into(),
                        phy: "phy1".into(),
                        monitor: false,
                    },
                ]),
                managed: Cell::new(true),
                cancel: Cell::new(false),
                no_monitor: false,
                start_fails_after_creation: false,
                stop_reports_failure: false,
                cancel_after_release: false,
                restore_fails: false,
                calls: RefCell::new(vec![]),
                events: RefCell::new(vec![]),
            }
        }

        fn saved_restore(&self) -> Option<NetworkRestore> {
            self.events
                .borrow()
                .iter()
                .rev()
                .find_map(|event| match event {
                    Event::NetworkRestore(restore) => Some(restore.clone()),
                    _ => None,
                })
                .flatten()
        }
    }

    impl Host for FakeHost {
        fn interfaces(&self) -> Vec<Interface> {
            self.devices.borrow().clone()
        }
        fn has_nmcli(&self) -> bool {
            true
        }
        fn cancelled(&self) -> bool {
            self.cancel.get()
        }
        fn emit(&self, event: Event) {
            self.events.borrow_mut().push(event);
        }
        fn run(&self, tool: Tool, args: &[&str], cleanup: bool) -> Result<Output, String> {
            self.calls.borrow_mut().push((
                tool,
                args.iter().map(|arg| arg.to_string()).collect(),
                cleanup,
            ));
            let mut output = Output {
                code: Some(0),
                cancelled: false,
                stdout: String::new(),
            };
            if self.cancel.get() && !cleanup {
                output.cancelled = true;
                return Ok(output);
            }
            if tool == Tool::Nmcli && args.contains(&"--get-values") {
                assert!(args.contains(&"GENERAL.NM-MANAGED"));
                output.stdout = if self.managed.get() { "yes\n" } else { "no\n" }.into();
            } else if tool == Tool::Nmcli {
                let manage = args.last() == Some(&"yes");
                if manage && self.restore_fails {
                    output.code = Some(1);
                } else {
                    self.managed.set(manage);
                    if !manage && self.cancel_after_release {
                        self.cancel.set(true);
                    }
                }
            } else if tool == Tool::Airmon {
                if args[0] == "start" && !self.no_monitor {
                    let mut devices = self.devices.borrow_mut();
                    let interface = devices
                        .iter_mut()
                        .find(|iface| iface.name == "wlp4s0")
                        .unwrap();
                    interface.name = "research0".into();
                    interface.monitor = true;
                    if self.start_fails_after_creation {
                        output.code = Some(1);
                    }
                } else if args[0] == "stop" {
                    let mut devices = self.devices.borrow_mut();
                    let interface = devices
                        .iter_mut()
                        .find(|iface| iface.name == args[1])
                        .unwrap();
                    interface.name = "wlp4s0".into();
                    interface.monitor = false;
                    if self.stop_reports_failure {
                        output.code = Some(1);
                    }
                }
            }
            Ok(output)
        }
    }

    #[test]
    fn releases_only_selected_adapter_and_restores_original_name() {
        let host = FakeHost::new();
        enable(&host, "wlp4s0").unwrap();
        assert!(!host.managed.get());
        let restore = host.saved_restore().unwrap();
        assert_eq!(restore.interface, "wlp4s0");
        assert!(
            host.events.borrow().iter().any(
                |event| matches!(event, Event::MonitorReady(iface) if iface.name == "research0")
            )
        );
        disable(&host, "research0", Some(&restore)).unwrap();
        assert!(host.managed.get());
        assert!(host.saved_restore().is_none());
        let calls = host.calls.borrow();
        let release = calls
            .iter()
            .position(|(tool, args, _)| {
                *tool == Tool::Nmcli && args.last().is_some_and(|arg| arg == "no")
            })
            .unwrap();
        let start = calls
            .iter()
            .position(|(tool, args, _)| *tool == Tool::Airmon && args[0] == "start")
            .unwrap();
        assert!(release < start);
        assert!(
            calls
                .iter()
                .all(|(_, args, _)| !args.iter().any(|arg| arg == "wlan1" || arg == "kill"))
        );
    }

    #[test]
    fn zero_exit_without_monitor_is_failure_and_rolls_back_networkmanager() {
        let mut host = FakeHost::new();
        host.no_monitor = true;
        let error = enable(&host, "wlp4s0").unwrap_err();
        assert!(error.contains("did not create a monitor interface"));
        assert!(host.managed.get());
        assert!(host.saved_restore().is_none());
        assert!(
            !host
                .events
                .borrow()
                .iter()
                .any(|event| matches!(event, Event::MonitorReady(_)))
        );
    }

    #[test]
    fn partial_start_failure_removes_monitor_before_returning_to_networkmanager() {
        let mut host = FakeHost::new();
        host.start_fails_after_creation = true;
        assert!(enable(&host, "wlp4s0").is_err());
        assert!(host.managed.get());
        assert!(host.saved_restore().is_none());
        assert!(host.interfaces().iter().all(|iface| !iface.monitor));
        let calls = host.calls.borrow();
        let stop = calls
            .iter()
            .position(|(tool, args, cleanup)| {
                *tool == Tool::Airmon && args == &["stop", "research0"] && *cleanup
            })
            .unwrap();
        let handoff = calls
            .iter()
            .position(|(tool, args, _)| {
                *tool == Tool::Nmcli && args.last().is_some_and(|arg| arg == "yes")
            })
            .unwrap();
        assert!(stop < handoff);
    }

    #[test]
    fn restoration_continues_when_stop_reports_failure_but_monitor_is_gone() {
        let mut host = FakeHost::new();
        enable(&host, "wlp4s0").unwrap();
        let restore = host.saved_restore().unwrap();
        host.stop_reports_failure = true;
        disable(&host, "research0", Some(&restore)).unwrap();
        assert!(host.managed.get());
        assert!(host.saved_restore().is_none());
    }

    #[test]
    fn cancellation_after_release_restores_even_with_cancel_flag_set() {
        let mut host = FakeHost::new();
        host.cancel_after_release = true;
        enable(&host, "wlp4s0").unwrap();
        assert!(host.cancelled());
        assert!(host.managed.get());
        assert!(host.saved_restore().is_none());
        assert!(
            !host
                .calls
                .borrow()
                .iter()
                .any(|(tool, _, _)| *tool == Tool::Airmon)
        );
    }

    #[test]
    fn failed_rollback_keeps_restoration_state_for_retry() {
        let mut host = FakeHost::new();
        host.no_monitor = true;
        host.restore_fails = true;
        assert!(
            enable(&host, "wlp4s0")
                .unwrap_err()
                .contains("Cleanup also failed")
        );
        let restore = host.saved_restore().unwrap();
        assert!(!host.managed.get());
        host.restore_fails = false;
        disable(&host, "wlp4s0", Some(&restore)).unwrap();
        assert!(host.managed.get());
        assert!(host.saved_restore().is_none());
    }

    #[test]
    fn preexisting_unmanaged_adapter_is_not_reassigned_to_networkmanager() {
        let host = FakeHost::new();
        host.managed.set(false);
        enable(&host, "wlp4s0").unwrap();
        disable(&host, "research0", None).unwrap();
        assert!(!host.managed.get());
        assert!(host.saved_restore().is_none());
        assert!(
            !host
                .calls
                .borrow()
                .iter()
                .any(|(tool, args, _)| *tool == Tool::Nmcli && args.iter().any(|arg| arg == "set"))
        );
    }

    #[test]
    fn unrelated_monitor_on_same_phy_does_not_count_as_success_or_get_stopped() {
        let mut host = FakeHost::new();
        host.no_monitor = true;
        host.devices.borrow_mut().push(Interface {
            name: "existingmon".into(),
            phy: "phy0".into(),
            monitor: true,
        });
        assert!(enable(&host, "wlp4s0").is_err());
        assert!(
            host.interfaces()
                .iter()
                .any(|iface| iface.name == "existingmon" && iface.monitor)
        );
        assert!(host.managed.get());
        assert!(
            !host
                .calls
                .borrow()
                .iter()
                .any(|(_, args, _)| args.iter().any(|arg| arg == "existingmon"))
        );
    }
}
