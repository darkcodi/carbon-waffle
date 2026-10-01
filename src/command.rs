use crate::model::{valid_interface, valid_mac};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tool {
    Airmon,
    Airodump,
    Aireplay,
    Aircrack,
    Hashcat,
    Hcx,
    Nmcli,
}

impl Tool {
    pub const ALL: [Self; 7] = [
        Self::Airmon,
        Self::Airodump,
        Self::Aireplay,
        Self::Aircrack,
        Self::Hashcat,
        Self::Hcx,
        Self::Nmcli,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Airmon => "airmon-ng",
            Self::Airodump => "airodump-ng",
            Self::Aireplay => "aireplay-ng",
            Self::Aircrack => "aircrack-ng",
            Self::Hashcat => "hashcat",
            Self::Hcx => "hcxpcapngtool",
            Self::Nmcli => "nmcli",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Engine {
    Aircrack,
    Hashcat,
}
impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Aircrack => "Aircrack-ng · CPU",
            Self::Hashcat => "Hashcat · GPU",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Operation {
    Monitor {
        interface: String,
        enable: bool,
        #[serde(default)]
        restore: Option<crate::monitor::NetworkRestore>,
    },
    Check,
    Scan {
        interface: String,
        prefix: PathBuf,
    },
    Capture {
        interface: String,
        bssid: String,
        channel: u16,
        prefix: PathBuf,
    },
    Deauth {
        interface: String,
        bssid: String,
        station: Option<String>,
        count: u16,
    },
    Inspect {
        capture: PathBuf,
        bssid: String,
    },
    InspectLive {
        capture: PathBuf,
        bssid: String,
        snapshot: PathBuf,
    },
    ReadCapture {
        capture: PathBuf,
    },
    Convert {
        capture: PathBuf,
        output: PathBuf,
    },
    Crack {
        engine: Engine,
        input: PathBuf,
        wordlist: PathBuf,
        #[serde(default)]
        pattern: Option<String>,
        bssid: String,
        output: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub tool: Tool,
    pub args: Vec<String>,
    pub privileged: bool,
}

impl CommandSpec {
    pub fn preview(&self) -> String {
        std::iter::once(self.tool.name().to_owned())
            .chain(self.args.iter().map(|arg| {
                if arg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_./:-".contains(&b))
                {
                    arg.clone()
                } else {
                    format!("'{}'", arg.replace('\'', "'\\''"))
                }
            }))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Operation {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Monitor { enable: true, .. } => "Enable monitor mode",
            Self::Monitor { enable: false, .. } => "Restore managed mode",
            Self::Check => "Check interfering processes",
            Self::Scan { .. } => "Discover networks",
            Self::Capture { .. } => "Capture handshake",
            Self::Deauth { .. } => "Deauthenticate",
            Self::Inspect { .. } => "Inspect capture",
            Self::InspectLive { .. } => "Check for handshake",
            Self::ReadCapture { .. } => "Read saved capture",
            Self::Convert { .. } => "Convert to hc22000",
            Self::Crack {
                pattern: Some(_), ..
            } => "Pattern recovery",
            Self::Crack { .. } => "Dictionary recovery",
        }
    }

    pub fn radio(&self) -> bool {
        matches!(
            self,
            Self::Scan { .. } | Self::Capture { .. } | Self::Monitor { .. }
        )
    }

    pub fn csv_path(&self) -> Option<PathBuf> {
        match self {
            Self::Scan { prefix, .. } | Self::Capture { prefix, .. } => {
                Some(PathBuf::from(format!("{}-01.csv", prefix.display())))
            }
            _ => None,
        }
    }

    pub fn spec(&self) -> Result<CommandSpec, String> {
        let iface = |value: &String| {
            if valid_interface(value) {
                Ok(value.clone())
            } else {
                Err("Select a valid Linux interface.".to_string())
            }
        };
        let mac = |value: &String| {
            if valid_mac(value) {
                Ok(value.to_uppercase())
            } else {
                Err("Enter a valid MAC address (AA:BB:CC:DD:EE:FF).".to_string())
            }
        };
        let path = |value: &Path| {
            if value.is_absolute() && !value.as_os_str().as_encoded_bytes().contains(&0) {
                value
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "This version requires UTF-8 file paths.".to_string())
            } else {
                Err("File paths must be absolute.".to_string())
            }
        };
        let (tool, args, privileged) = match self {
            Self::Monitor {
                interface,
                enable,
                restore,
            } => {
                if let Some(restore) = restore {
                    restore.validate()?;
                    if *enable {
                        return Err(
                            "A new monitoring session cannot carry restoration state.".into()
                        );
                    }
                }
                (
                    Tool::Airmon,
                    vec![
                        if *enable { "start" } else { "stop" }.into(),
                        iface(interface)?,
                    ],
                    true,
                )
            }
            Self::Check => (Tool::Airmon, vec!["check".into()], true),
            Self::Scan { interface, prefix } => (
                Tool::Airodump,
                vec![
                    "--band".into(),
                    "abg".into(),
                    "--write".into(),
                    path(prefix)?,
                    "--output-format".into(),
                    "csv".into(),
                    "--write-interval".into(),
                    "1".into(),
                    iface(interface)?,
                ],
                true,
            ),
            Self::Capture {
                interface,
                bssid,
                channel,
                prefix,
            } => {
                if !(1..=233).contains(channel) {
                    return Err("Select a network with a valid channel.".into());
                }
                (
                    Tool::Airodump,
                    vec![
                        "--bssid".into(),
                        mac(bssid)?,
                        "--channel".into(),
                        channel.to_string(),
                        "--write".into(),
                        path(prefix)?,
                        "--output-format".into(),
                        "pcap,csv".into(),
                        "--write-interval".into(),
                        "1".into(),
                        iface(interface)?,
                    ],
                    true,
                )
            }
            Self::Deauth {
                interface,
                bssid,
                station,
                count,
            } => {
                if !(1..=100).contains(count) {
                    return Err("Choose 1–100 deauthentication bursts.".into());
                }
                let mut args = vec![
                    "--deauth".into(),
                    count.to_string(),
                    "-a".into(),
                    mac(bssid)?,
                ];
                if let Some(station) = station {
                    args.extend(["-c".into(), mac(station)?]);
                }
                args.push(iface(interface)?);
                (Tool::Aireplay, args, true)
            }
            Self::ReadCapture { capture } => (Tool::Aircrack, vec![path(capture)?], false),
            Self::Inspect { capture, bssid } => {
                mac(bssid)?;
                // Read the network summary. Forcing WPA without a wordlist
                // aborts in Aircrack-ng 1.7; -b suppresses handshake counts.
                // The worker selects the matching summary row and stops here.
                (Tool::Aircrack, vec![path(capture)?], false)
            }
            Self::InspectLive {
                capture,
                bssid,
                snapshot,
            } => {
                path(capture)?;
                mac(bssid)?;
                if capture == snapshot {
                    return Err("A live check must use a separate snapshot.".into());
                }
                (Tool::Aircrack, vec![path(snapshot)?], false)
            }
            Self::Convert { capture, output } => (
                Tool::Hcx,
                vec!["-o".into(), path(output)?, path(capture)?],
                false,
            ),
            Self::Crack {
                engine,
                input,
                wordlist,
                pattern,
                bssid,
                output,
            } => {
                if let Some(pattern) = pattern {
                    crate::pattern::Pattern::parse(pattern)?;
                }
                match engine {
                    Engine::Aircrack => (
                        Tool::Aircrack,
                        vec![
                            "-a".into(),
                            "2".into(),
                            "-b".into(),
                            mac(bssid)?,
                            "-w".into(),
                            if pattern.is_some() {
                                "-".into()
                            } else {
                                path(wordlist)?
                            },
                            "-l".into(),
                            path(output)?,
                            path(input)?,
                        ],
                        false,
                    ),
                    Engine::Hashcat => {
                        let mut args = vec![
                            "-m".into(),
                            "22000".into(),
                            "-a".into(),
                            "0".into(),
                            "--status".into(),
                            "--status-timer".into(),
                            "2".into(),
                            "--potfile-path".into(),
                            path(&output.with_extension("potfile"))?,
                            "--restore-disable".into(),
                            "--outfile".into(),
                            path(output)?,
                            path(input)?,
                        ];
                        if pattern.is_some() {
                            // No dictionary argument selects stdin in straight mode.
                            // Generated literals must not be interpreted as $HEX[].
                            args.push("--wordlist-autohex-disable".into());
                        } else {
                            args.push(path(wordlist)?);
                        }
                        (Tool::Hashcat, args, false)
                    }
                }
            }
        };
        Ok(CommandSpec {
            tool,
            args,
            privileged,
        })
    }

    pub fn validate_files(&self) -> Result<(), String> {
        let require = |p: &Path| {
            std::fs::File::options()
                .read(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(p)
                .and_then(|f| f.metadata())
                .and_then(|m| {
                    if m.is_file() {
                        Ok(())
                    } else {
                        Err(std::io::Error::other("not a regular file"))
                    }
                })
                .map_err(|e| format!("Cannot read {}: {e}", p.display()))
        };
        match self {
            Self::Inspect { capture, .. }
            | Self::InspectLive { capture, .. }
            | Self::ReadCapture { capture }
            | Self::Convert { capture, .. } => require(capture)?,
            Self::Crack {
                input,
                wordlist,
                pattern,
                ..
            } => {
                require(input)?;
                let needs_words = match pattern {
                    Some(source) => crate::pattern::Pattern::parse(source)?.needs_words(),
                    None => true,
                };
                if needs_words {
                    require(wordlist)?;
                }
            }
            _ => {}
        }
        if let Self::Convert { output, .. }
        | Self::Crack { output, .. }
        | Self::InspectLive {
            snapshot: output, ..
        } = self
            && output.exists()
        {
            return Err("Output already exists; use a new output file.".into());
        }
        Ok(())
    }
}

pub fn find_tool(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|p| p.join(name))
        .find(|p| {
            p.is_absolute()
                && p.is_file()
                && p.metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_keep_paths_as_single_arguments_and_reject_unbounded_deauth() {
        let op = Operation::Crack {
            engine: Engine::Aircrack,
            input: "/tmp/$(id).cap".into(),
            wordlist: "/tmp/a b.txt".into(),
            pattern: None,
            bssid: "02:00:00:00:00:01".into(),
            output: "/tmp/key.txt".into(),
        };
        let spec = op.spec().unwrap();
        assert_eq!(spec.args[5], "/tmp/a b.txt");
        assert_eq!(spec.args.last().unwrap(), "/tmp/$(id).cap");
        assert!(!spec.privileged);
        let op = Operation::Deauth {
            interface: "wlan0mon".into(),
            bssid: "02:00:00:00:00:01".into(),
            station: None,
            count: 0,
        };
        assert!(op.spec().is_err());
    }

    #[test]
    fn capture_is_locked_to_target_channel() {
        let spec = Operation::Capture {
            interface: "wlan0mon".into(),
            bssid: "02:00:00:00:00:01".into(),
            channel: 36,
            prefix: "/tmp/capture".into(),
        }
        .spec()
        .unwrap();
        assert_eq!(
            &spec.args[..4],
            ["--bssid", "02:00:00:00:00:01", "--channel", "36"]
        );
        assert!(spec.privileged);
    }

    #[test]
    fn pattern_recovery_uses_stdin_and_validates_only_required_files() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("capture.cap");
        std::fs::write(&input, b"synthetic").unwrap();
        for engine in [Engine::Aircrack, Engine::Hashcat] {
            let mut operation = Operation::Crack {
                engine,
                input: input.clone(),
                wordlist: directory.path().join("words.txt"),
                pattern: Some("[0-9]{8}".into()),
                bssid: "02:00:00:00:00:01".into(),
                output: directory.path().join("result.txt"),
            };
            operation.validate_files().unwrap();
            let spec = operation.spec().unwrap();
            assert!(!spec.privileged);
            assert!(!spec.args.iter().any(|arg| arg.ends_with("words.txt")));
            if engine == Engine::Aircrack {
                assert_eq!(&spec.args[4..6], ["-w", "-"]);
            } else {
                assert_eq!(&spec.args[2..4], ["-a", "0"]);
                assert_eq!(spec.args.last().unwrap(), "--wordlist-autohex-disable");
            }
            if let Operation::Crack { pattern, .. } = &mut operation {
                *pattern = Some("{word}{2}".into());
            }
            assert!(
                operation.validate_files().is_err(),
                "word slots need a source file"
            );
            if let Operation::Crack { pattern, .. } = &mut operation {
                *pattern = None;
            }
            assert!(
                operation.validate_files().is_err(),
                "dictionary mode needs a source file"
            );
            assert!(
                operation
                    .spec()
                    .unwrap()
                    .args
                    .iter()
                    .any(|arg| arg.ends_with("words.txt"))
            );
        }
    }
}
