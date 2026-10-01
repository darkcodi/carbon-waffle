use super::*;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Inspection {
    Found,
    NotFound,
    Unknown,
}

// Parse only Aircrack's table row for the selected BSSID, never a handshake
// belonging to another AP or text embedded in an ESSID.
fn summary(line: &str, bssid: &str) -> Option<Inspection> {
    if line.trim() == "No networks found, exiting." {
        return Some(Inspection::NotFound);
    }
    if line.starts_with("Choosing first network as target.")
        || line.starts_with("Index number of target network ?")
    {
        return Some(Inspection::Unknown);
    }
    let mut fields = line.split_whitespace();
    fields.next()?.parse::<usize>().ok()?;
    if !fields.next()?.eq_ignore_ascii_case(bssid) {
        return None;
    }
    let (_, encryption) = line.rsplit_once("  ")?;
    let encryption = encryption
        .trim()
        .strip_prefix("EAPOL+")
        .unwrap_or(encryption.trim());
    let count = encryption
        .strip_prefix("WPA (")?
        .strip_suffix(')')?
        .split_once(" handshake")?
        .0
        .parse::<u32>()
        .ok()?;
    Some(if count > 0 {
        Inspection::Found
    } else {
        Inspection::NotFound
    })
}

pub(super) fn run(
    executable: &Path,
    args: &[String],
    bssid: &str,
    cancel: Arc<AtomicBool>,
    sink: Arc<impl Fn(Event) + Send + Sync + 'static>,
) -> Result<(), String> {
    let result = Arc::new(Mutex::new(None));
    let found = result.clone();
    let stop = cancel.clone();
    let output_sink = sink.clone();
    let bssid = bssid.to_owned();
    let output = run_process(
        executable,
        args,
        None,
        cancel,
        Arc::new(move |event| {
            if let Event::Line(line) = &event
                && let Some(value) = summary(line, &bssid)
            {
                let mut result = found.lock().unwrap();
                if result.is_none() {
                    *result = Some(value);
                }
                // The summary is sufficient; do not wait at a selection prompt.
                stop.store(true, Ordering::Relaxed);
            }
            output_sink(event);
        }),
        Some(Duration::from_secs(30)),
    )?;
    let result = *result.lock().unwrap();
    if let Some(result) = result {
        sink(Event::Inspection(result));
        sink(Event::Finished {
            code: Some(0),
            cancelled: false,
        });
    } else {
        if !output.cancelled {
            sink(Event::Inspection(Inspection::Unknown));
        }
        sink(Event::Finished {
            code: output.code,
            cancelled: output.cancelled,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn inspection_stops_after_summary_and_delivers_result_separately_from_logs() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("aircrack-ng");
        fs::write(&executable, "#!/bin/sh\nprintf '   1  02:11:22:33:44:56  Other AP                  WPA (1 handshake)\\n   2  02:11:22:33:44:55  Lab One                   WPA (0 handshake)\\n'\nsleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let (sender, received) = mpsc::channel();
        let start = Instant::now();
        run(
            &executable,
            &[],
            "02:11:22:33:44:55",
            Arc::new(AtomicBool::new(false)),
            Arc::new(move |event| {
                sender.send(event).unwrap();
            }),
        )
        .unwrap();
        let events: Vec<_> = received.try_iter().collect();
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Inspection(Inspection::NotFound)))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Finished {
                code: Some(0),
                cancelled: false
            }
        )));
    }

    #[test]
    fn successful_exit_without_handshake_evidence_stays_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("aircrack-ng");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'Please specify a dictionary (option -w).\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let (sender, received) = mpsc::channel();
        run(
            &executable,
            &[],
            "02:11:22:33:44:55",
            Arc::new(AtomicBool::new(false)),
            Arc::new(move |event| {
                sender.send(event).unwrap();
            }),
        )
        .unwrap();
        assert!(
            received
                .try_iter()
                .any(|event| matches!(event, Event::Inspection(Inspection::Unknown)))
        );
    }

    #[test]
    fn inspection_requires_the_selected_bssid_and_a_handshake_count() {
        let bssid = "02:11:22:33:44:55";
        assert_eq!(
            summary(
                "   1  02:11:22:33:44:55  Lab One                   WPA (1 handshake)",
                bssid
            ),
            Some(Inspection::Found)
        );
        assert_eq!(
            summary(
                "   2  02:11:22:33:44:55  Lab One                   WPA (0 handshake, with PMKID)",
                bssid
            ),
            Some(Inspection::NotFound)
        );
        assert_eq!(
            summary(
                "   2  02:11:22:33:44:55  WPA (1 handshake)         WPA (0 handshake)",
                bssid
            ),
            Some(Inspection::NotFound)
        );
        assert_eq!(
            summary(
                "   2  02:11:22:33:44:56  Lab One                   WPA (1 handshake)",
                bssid
            ),
            None
        );
        assert_eq!(summary("WPA handshake: 02:11:22:33:44:55", bssid), None);
        assert_eq!(
            summary("Please specify a dictionary (option -w).", bssid),
            None
        );
    }
}
