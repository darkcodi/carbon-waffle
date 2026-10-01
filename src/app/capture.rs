//! One capture session owns its reconnect requests and live checks.
use super::*;
use std::collections::HashMap;

pub(super) const LIMIT: Duration = Duration::from_secs(120);
const CHECK_INTERVAL: Duration = Duration::from_secs(3);
const RECONNECT_PAUSE: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopReason {
    Handshake,
    Timeout,
    Cancelled,
    Failed,
}

pub(super) struct Automatic {
    pub job: u64,
    pub started: Instant,
    pub ready: bool,
    pub next_check: Instant,
    pub next_request: Instant,
    pub requests: usize,
    broadcast: Option<Instant>,
    sent: HashMap<String, Instant>,
}

impl Automatic {
    pub fn new(job: u64, now: Instant) -> Self {
        Self {
            job,
            started: now,
            ready: false,
            next_check: now + Duration::from_secs(2),
            next_request: now + Duration::from_secs(2),
            requests: 0,
            broadcast: None,
            sent: HashMap::new(),
        }
    }

    // Start with a broadcast, then include every newly observed client. Retry
    // known clients at a slower cadence; never leave aireplay running forever.
    fn request(
        &mut self,
        now: Instant,
        clients: &[model::Station],
        bssid: &str,
    ) -> Option<Option<String>> {
        if now < self.next_request {
            return None;
        }
        let candidate = clients
            .iter()
            .filter(|client| {
                client.bssid.eq_ignore_ascii_case(bssid) && model::valid_mac(&client.mac)
            })
            .map(|client| client.mac.to_ascii_uppercase())
            .filter(|mac| {
                self.sent
                    .get(mac)
                    .is_none_or(|last| now.duration_since(*last) >= RETRY_INTERVAL)
            })
            .min_by_key(|mac| self.sent.get(mac).copied());
        let station = if self.broadcast.is_none() {
            None
        } else if candidate
            .as_ref()
            .is_some_and(|mac| !self.sent.contains_key(mac))
        {
            candidate
        } else if self
            .broadcast
            .is_some_and(|last| now.duration_since(last) >= RETRY_INTERVAL)
        {
            None
        } else if let Some(mac) = candidate {
            Some(mac)
        } else {
            return None;
        };
        if let Some(mac) = &station {
            self.sent.insert(mac.clone(), now);
        } else {
            self.broadcast = Some(now);
        }
        self.requests += 1;
        self.next_request = now + RECONNECT_PAUSE;
        Some(station)
    }
}

impl App {
    pub(super) fn stop_automatic(&mut self, reason: StopReason, check_saved: bool) {
        self.automatic_capture = None;
        self.capture_stop = Some(reason);
        self.check_after_capture = check_saved && !self.closing;
        if matches!(self.pending, Some(Operation::Capture { .. })) {
            self.pending = None;
        }
        for job in &mut self.jobs {
            if matches!(
                job.operation,
                Operation::Capture { .. }
                    | Operation::Deauth { .. }
                    | Operation::InspectLive { .. }
            ) {
                job.stop();
            }
        }
    }

    pub(super) fn capture_failed(&mut self, error: String, check_saved: bool) {
        self.capture_problem = Some(error.clone());
        self.stop_automatic(StopReason::Failed, check_saved);
        self.fail(error);
    }

    pub(super) fn live_inspection_matches(&self, operation: &Operation) -> bool {
        matches!(operation, Operation::InspectLive { capture, bssid, .. }
            if absolute_path(&self.capture_path).as_ref().ok() == Some(capture)
                && self.automatic_capture.as_ref().is_some_and(|automatic| self.jobs.iter().any(|job|
                    job.id == automatic.job && !job.stopping && matches!(&job.operation,
                        Operation::Capture { bssid: recorded, .. } if recorded.eq_ignore_ascii_case(bssid)))))
    }

    pub(super) fn automatic_capture_tick(&mut self, now: Instant) {
        let Some(automatic) = &self.automatic_capture else {
            return;
        };
        if self.closing || self.authorization != Authorization::Ready {
            self.stop_automatic(StopReason::Cancelled, false);
            return;
        }
        if now.duration_since(automatic.started) >= LIMIT {
            self.stop_automatic(StopReason::Timeout, true);
            self.status = "Capture time limit reached. Saving and checking the recording…".into();
            return;
        }
        if !self.capture_running() {
            return;
        }
        let Some(job) = self
            .jobs
            .iter()
            .find(|job| job.id == automatic.job && job.started && !job.stopping)
        else {
            return;
        };
        let Operation::Capture { bssid, .. } = &job.operation else {
            return;
        };
        let bssid = bssid.clone();
        if !automatic.ready
            || self
                .jobs
                .iter()
                .any(|job| matches!(job.operation, Operation::InspectLive { .. }))
        {
            return;
        }
        if now >= automatic.next_check {
            self.automatic_capture.as_mut().unwrap().next_check = now + CHECK_INTERVAL;
            self.refresh_capture_available();
            if self.capture_available {
                let operation = Operation::InspectLive {
                    capture: PathBuf::from(&self.capture_path),
                    bssid,
                    snapshot: self
                        .session
                        .join("recovery")
                        .join(format!(".live-check-{}.cap", self.next_id + 1)),
                };
                self.launch(operation);
                if !self
                    .jobs
                    .iter()
                    .any(|job| matches!(job.operation, Operation::InspectLive { .. }))
                {
                    self.capture_failed(self.status.clone(), false);
                }
            }
            return;
        }
        if !self.capture_available
            || self
                .jobs
                .iter()
                .any(|job| matches!(job.operation, Operation::Deauth { .. }))
        {
            return;
        }
        if let Some(station) =
            self.automatic_capture
                .as_mut()
                .unwrap()
                .request(now, &self.survey.stations, &bssid)
        {
            self.deauth_station = station;
            self.run(Action::Deauth);
            if !self
                .jobs
                .iter()
                .any(|job| matches!(job.operation, Operation::Deauth { .. }))
            {
                self.capture_failed(self.status.clone(), true);
            }
        }
    }

    pub(super) fn reconnect_finished(&mut self) {
        if let Some(automatic) = &mut self.automatic_capture {
            automatic.next_request = Instant::now() + RECONNECT_PAUSE;
        }
    }

    pub(super) fn live_check_finished(&mut self) {
        if let Some(automatic) = &mut self.automatic_capture {
            // A slow checker must leave time for reconnect requests, instead
            // of starting another overdue check immediately on completion.
            automatic.next_check = Instant::now() + CHECK_INTERVAL;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_scheduler_includes_new_clients_and_allows_time_to_reconnect() {
        let now = Instant::now();
        let mut automatic = Automatic::new(1, now);
        let bssid = "02:00:00:00:00:01";
        let mut clients = vec![model::Station {
            mac: "02:00:00:00:01:01".into(),
            bssid: bssid.into(),
            power: -40,
            packets: 1,
        }];
        assert_eq!(automatic.request(now, &clients, bssid), None);
        assert_eq!(
            automatic.request(now + Duration::from_secs(2), &clients, bssid),
            Some(None)
        );
        assert_eq!(
            automatic.request(now + Duration::from_secs(3), &clients, bssid),
            None
        );
        assert_eq!(
            automatic.request(now + Duration::from_secs(7), &clients, bssid),
            Some(Some(clients[0].mac.clone()))
        );
        clients.push(model::Station {
            mac: "02:00:00:00:01:02".into(),
            ..clients[0].clone()
        });
        clients.push(model::Station {
            mac: "02:00:00:00:01:03".into(),
            bssid: "02:00:00:00:00:02".into(),
            ..clients[0].clone()
        });
        assert_eq!(
            automatic.request(now + Duration::from_secs(12), &clients, bssid),
            Some(Some(clients[1].mac.clone()))
        );
        assert_eq!(
            automatic.request(now + Duration::from_secs(17), &clients, bssid),
            None
        );
        assert_eq!(
            automatic.request(now + Duration::from_secs(22), &clients, bssid),
            Some(None)
        );
        assert_eq!(
            automatic.request(now + Duration::from_secs(27), &clients, bssid),
            Some(Some(clients[0].mac.clone()))
        );
    }
}
