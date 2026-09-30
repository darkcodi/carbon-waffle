use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub phy: String,
    pub monitor: bool,
}

impl fmt::Display for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} · {} · {}",
            self.name,
            self.phy,
            if self.monitor { "monitor" } else { "managed" }
        )
    }
}

pub fn interfaces() -> Vec<Interface> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return vec![];
    };
    let mut found = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(phy) = fs::read_link(path.join("phy80211")) else {
            continue;
        };
        found.push(Interface {
            name: entry.file_name().to_string_lossy().into_owned(),
            phy: phy
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            monitor: fs::read_to_string(path.join("type")).is_ok_and(|v| v.trim() == "803"),
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Network {
    pub bssid: String,
    pub ssid: String,
    pub channel: u16,
    pub security: String,
    pub authentication: String,
    pub power: i16,
    pub beacons: u64,
}

impl Network {
    pub fn label(&self) -> &str {
        if self.ssid.is_empty() {
            "<hidden network>"
        } else {
            &self.ssid
        }
    }

    pub fn supports_dictionary(&self) -> bool {
        self.security.contains("WPA") && self.authentication.contains("PSK")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Station {
    pub mac: String,
    pub bssid: String,
    pub power: i16,
    pub packets: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Survey {
    pub networks: Vec<Network>,
    pub stations: Vec<Station>,
}

pub struct NetworkGroup<'a> {
    pub ssid: &'a str,
    pub access_points: Vec<&'a Network>,
}

impl Survey {
    pub fn network_groups(&self, filter: &str) -> Vec<NetworkGroup<'_>> {
        let filter = filter.to_lowercase();
        let mut groups: Vec<NetworkGroup<'_>> = Vec::new();
        let mut indices = HashMap::new();
        // Preserve survey signal order, grouping exact SSIDs without combining
        // the BSSIDs, channels, or security settings used for target selection.
        for network in &self.networks {
            if !network.ssid.to_lowercase().contains(&filter)
                && !network.bssid.to_lowercase().contains(&filter)
            {
                continue;
            }
            let index = *indices.entry(network.ssid.as_str()).or_insert_with(|| {
                groups.push(NetworkGroup {
                    ssid: &network.ssid,
                    access_points: Vec::new(),
                });
                groups.len() - 1
            });
            groups[index].access_points.push(network);
        }
        groups
    }
}

/// Airodump's CSV does not quote commas in ESSIDs. Its byte-length field is
/// authoritative; a generic CSV reader would silently change network names.
pub fn parse_survey(input: &str) -> Survey {
    let mut result = Survey::default();
    let mut stations = false;
    // Ignore an incomplete final row while airodump rewrites the snapshot.
    for line in input
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
    {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.starts_with("Station MAC,") {
            stations = true;
            continue;
        }
        if line.starts_with("BSSID,") {
            stations = false;
            continue;
        }
        if stations {
            let fields: Vec<_> = line.splitn(7, ',').map(str::trim).collect();
            if fields.len() < 6 || !valid_mac(fields[0]) || !valid_mac(fields[5]) {
                continue;
            }
            let (Ok(power), Ok(packets)) = (fields[3].parse(), fields[4].parse()) else {
                continue;
            };
            result.stations.push(Station {
                mac: fields[0].to_uppercase(),
                bssid: fields[5].to_uppercase(),
                power,
                packets,
            });
        } else {
            let fields: Vec<_> = line.splitn(14, ',').collect();
            if fields.len() != 14 || !valid_mac(fields[0].trim()) {
                continue;
            }
            let (Ok(channel), Ok(power), Ok(beacons), Ok(length)) = (
                fields[3].trim().parse::<u16>(),
                fields[8].trim().parse(),
                fields[9].trim().parse(),
                fields[12].trim().parse::<usize>(),
            ) else {
                continue;
            };
            if channel == 0 || channel > 233 || length > 128 {
                continue;
            }
            let tail = fields[13].strip_prefix(' ').unwrap_or(fields[13]);
            let Some(ssid) = tail.get(..length) else {
                continue;
            };
            let bssid = fields[0].trim().to_uppercase();
            if result.networks.iter().any(|n| n.bssid == bssid) {
                continue;
            }
            result.networks.push(Network {
                bssid,
                ssid: clean_terminal(ssid),
                channel,
                security: fields[5].trim().into(),
                authentication: fields[7].trim().into(),
                power,
                beacons,
            });
        }
    }
    result.networks.sort_by_key(|n| {
        if n.power == -1 {
            i32::MAX
        } else {
            -i32::from(n.power)
        }
    });
    result
}

pub fn valid_mac(input: &str) -> bool {
    input.len() == 17
        && input.split(':').count() == 6
        && input
            .split(':')
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn valid_interface(input: &str) -> bool {
    !input.is_empty()
        && input.len() <= 15
        && !input.starts_with('-')
        && input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

pub fn absolute_path(input: &str) -> Result<std::path::PathBuf, String> {
    if input.is_empty() || input.contains('\0') {
        return Err("Enter a file path.".into());
    }
    let path = Path::new(input);
    if path.is_absolute() {
        Ok(path.into())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|e| e.to_string())
    }
}

/// Strip terminal control sequences before putting CLI output in a GUI label.
pub fn clean_terminal(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for v in chars.by_ref() {
                        if ('@'..='~').contains(&v) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(v) = chars.next() {
                        if v == '\x07' || (v == '\x1b' && chars.next() == Some('\\')) {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

pub fn demo_survey() -> Survey {
    Survey {
        networks: vec![
            Network {
                bssid: "02:00:00:00:01:01".into(),
                ssid: "Research Lab".into(),
                channel: 6,
                security: "WPA2".into(),
                authentication: "PSK".into(),
                power: -37,
                beacons: 2481,
            },
            Network {
                bssid: "02:00:00:00:01:02".into(),
                ssid: "Lab · 5 GHz".into(),
                channel: 36,
                security: "WPA2".into(),
                authentication: "PSK".into(),
                power: -54,
                beacons: 1720,
            },
            Network {
                bssid: "02:00:00:00:01:03".into(),
                ssid: "".into(),
                channel: 11,
                security: "WPA3".into(),
                authentication: "SAE".into(),
                power: -68,
                beacons: 611,
            },
        ],
        stations: vec![Station {
            mac: "02:00:00:00:02:01".into(),
            bssid: "02:00:00:00:01:01".into(),
            power: -42,
            packets: 83,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survey_handles_commas_hidden_names_clients_and_partial_writes() {
        let data = "BSSID, First time seen, Last time seen, channel, Speed, Privacy, Cipher, Authentication, Power, # beacons, # IV, LAN IP, ID-length, ESSID, Key\n\
02:00:00:00:00:01, x, x, 6, 54, WPA2, CCMP, PSK, -40, 20, 0, 0, 8, Lab, One, \n\
02:00:00:00:00:02, x, x, 11, 54, WPA2, CCMP, PSK, -1, 20, 0, 0, 0, , \n\
Station MAC, First time seen, Last time seen, Power, # packets, BSSID, Probed ESSIDs\n\
02:00:00:00:01:01, x, x, -50, 100, 02:00:00:00:00:01, \n\
02:00:00:00:01:02, x, x, -50, 100, (not associated), Lab\n\
02:00:00:00:01:03, x, x, -50, 100, 02:00:00:00:00:01";
        let survey = parse_survey(data);
        assert_eq!(survey.networks.len(), 2);
        assert_eq!(survey.networks[0].ssid, "Lab, One");
        assert_eq!(survey.networks[1].label(), "<hidden network>");
        assert_eq!(survey.stations.len(), 1);
    }

    #[test]
    fn ssid_groups_preserve_bssids_security_and_filter_matches() {
        let mut survey = demo_survey();
        let ssid = survey.networks[0].ssid.clone();
        survey.networks[1].ssid = ssid.clone();
        survey.networks[1].bssid = "02:AA:00:00:01:02".into();
        survey.networks[1].authentication = "SAE".into();
        let mut different_case = survey.networks[0].clone();
        different_case.ssid = ssid.to_lowercase();
        different_case.bssid = "02:00:00:00:01:04".into();
        survey.networks.push(different_case);
        let mut another_hidden = survey.networks[2].clone();
        another_hidden.bssid = "02:00:00:00:01:05".into();
        survey.networks.push(another_hidden);

        let groups = survey.network_groups("");
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].ssid, ssid);
        assert_eq!(
            groups[0].access_points,
            vec![&survey.networks[0], &survey.networks[1]]
        );
        assert_ne!(
            groups[0].access_points[0].channel,
            groups[0].access_points[1].channel
        );
        assert_ne!(
            groups[0].access_points[0].authentication,
            groups[0].access_points[1].authentication
        );
        assert!(groups[1].ssid.is_empty());
        assert_eq!(groups[1].access_points.len(), 2);
        assert_eq!(groups[2].ssid, ssid.to_lowercase());

        let by_name = survey.network_groups(&ssid.to_uppercase());
        assert_eq!(by_name.len(), 2); // Search ignores case, grouping preserves it.
        let by_bssid = survey.network_groups("02:aa:00:00:01:02");
        assert_eq!(by_bssid.len(), 1);
        assert_eq!(by_bssid[0].access_points, vec![&survey.networks[1]]);
        assert!(survey.network_groups("no such network").is_empty());
    }

    #[test]
    fn terminal_controls_and_option_injection_are_rejected() {
        assert_eq!(
            clean_terminal("\x1b[31mhello\x1b[0m\r\x1b]0;title\x07!"),
            "hello!"
        );
        assert!(valid_interface("wlp3s0mon"));
        assert!(!valid_interface("--help"));
        assert!(!valid_interface("wlan0;id"));
        assert!(!valid_mac("02:00:00:00:00:xx"));
    }
}
