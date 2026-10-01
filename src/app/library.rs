//! Capture storage and discovery. Directory walks and imports run off the UI thread.
use super::*;
use std::{
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub detail: String,
    pub modified: SystemTime,
    pub hash: bool,
}

#[derive(Default)]
pub(super) struct Library {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub loading: bool,
    pub importing: bool,
    pub error: Option<String>,
    pub revision: u64,
    pub show: bool,
    pub dirty: bool,
}

#[derive(Serialize, Deserialize)]
struct Label {
    ssid: String,
    bssid: String,
}

pub(super) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            nix::unistd::User::from_uid(nix::unistd::getuid())
                .ok()
                .flatten()
                .map(|u| u.dir)
        })
}

pub(super) fn supported(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["cap", "pcap", "pcapng", "hc22000", "22000"]
            .iter()
            .any(|ext| e.eq_ignore_ascii_case(ext))
    })
}

fn small_text(path: &Path) -> Option<String> {
    let file = fs::File::options()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > 1_048_576 {
        return None;
    }
    let mut text = String::new();
    file.take(1_048_576).read_to_string(&mut text).ok()?;
    Some(text)
}

fn label(path: &Path) -> Option<Label> {
    if let Some(label) =
        small_text(&path.with_extension("carbon.json")).and_then(|s| serde_json::from_str(&s).ok())
    {
        return Some(label);
    }
    // Old recordings already have an airodump CSV alongside them.
    let survey = model::parse_survey(&small_text(&path.with_extension("csv"))?);
    let network = survey.networks.first()?;
    (survey.networks.len() == 1).then(|| Label {
        ssid: network.ssid.clone(),
        bssid: network.bssid.clone(),
    })
}

fn write_label(path: &Path, label: &Label) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path.with_extension("carbon.json"))
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(label).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

pub(super) fn record_label(path: &Path, target: &Network) -> Result<(), String> {
    write_label(
        path,
        &Label {
            ssid: target.ssid.clone(),
            bssid: target.bssid.clone(),
        },
    )
}

pub(super) fn entry(path: &Path) -> Result<Entry, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || !supported(path) {
        return Err(
            "Choose a nonempty capture (.cap, .pcap, .pcapng) or WPA hash (.hc22000, .22000)."
                .into(),
        );
    }
    let label = label(path);
    let name = label
        .as_ref()
        .map(|label| {
            if label.ssid.is_empty() {
                "Hidden network".into()
            } else {
                label.ssid.clone()
            }
        })
        .unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        .as_secs();
    let age = match age {
        0..60 => "Just now".into(),
        60..3600 => format!("{} min ago", age / 60),
        3600..7200 => "1 hour ago".into(),
        7200..86400 => format!("{} hours ago", age / 3600),
        86400..172800 => "1 day ago".into(),
        _ => format!("{} days ago", age / 86400),
    };
    let size = if metadata.len() >= 1_048_576 {
        format!("{:.1} MiB", metadata.len() as f64 / 1_048_576.0)
    } else {
        format!("{:.1} KiB", metadata.len() as f64 / 1024.0)
    };
    let hash = path.extension().is_some_and(|ext| {
        ext.eq_ignore_ascii_case("hc22000") || ext.eq_ignore_ascii_case("22000")
    });
    let identity = label
        .map(|l| l.bssid)
        .unwrap_or_else(|| if hash { "WPA hash" } else { "Capture" }.into());
    Ok(Entry {
        path: path.into(),
        name,
        detail: format!("{identity} · {age} · {size}"),
        modified,
        hash,
    })
}

pub(super) fn scan(root: &Path) -> Result<Vec<Entry>, String> {
    if !root.is_absolute() {
        return Err("Could not locate your home directory.".into());
    }
    fn walk(directory: &Path, depth: usize, entries: &mut Vec<Entry>) -> Result<(), String> {
        let files = match fs::read_dir(directory) {
            Ok(files) => files,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && depth == 0 => return Ok(()),
            Err(e) => return Err(format!("Cannot read {}: {e}", directory.display())),
        };
        for file in files {
            let file = file.map_err(|e| e.to_string())?;
            let kind = file.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() && depth < 8 && file.file_name() != "recovery" {
                walk(&file.path(), depth + 1, entries)?;
            } else if kind.is_file()
                && supported(&file.path())
                && let Ok(item) = entry(&file.path())
            {
                entries.push(item);
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    walk(root, 0, &mut entries)?;
    entries.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(entries)
}

pub(super) fn import(root: &Path, source: &Path) -> Result<Entry, String> {
    let source = fs::canonicalize(source).map_err(|e| e.to_string())?;
    let original = entry(&source)?;
    if fs::canonicalize(root).is_ok_and(|root| {
        source.strip_prefix(root).is_ok_and(|relative| {
            relative.components().count() <= 9
                && !relative
                    .components()
                    .any(|part| part.as_os_str() == "recovery")
        })
    }) {
        return Ok(original);
    }
    if !root.is_absolute() {
        return Err("Could not locate your home directory.".into());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = root.join(format!("import-{stamp}-{}", std::process::id()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|e| e.to_string())?;
    let destination = directory.join(source.file_name().ok_or("Missing filename.")?);
    let result = (|| {
        let mut input = fs::File::options()
            .read(true)
            .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW)
            .open(&source)
            .map_err(|e| e.to_string())?;
        if !input.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Choose a regular file.".into());
        }
        let temporary = directory.join(".importing");
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        output
            .set_times(fs::FileTimes::new().set_modified(original.modified))
            .map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        if let Some(label) = label(&source) {
            write_label(&destination, &label)?;
        }
        fs::rename(&temporary, &destination).map_err(|e| e.to_string())?;
        entry(&destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(directory);
    }
    result
}

impl App {
    pub(super) fn refresh_library(&mut self) -> Task<Message> {
        self.library.dirty = false;
        if self.demo {
            return Task::none();
        }
        self.library.revision += 1;
        self.library.loading = true;
        let revision = self.library.revision;
        let root = self.library.root.clone();
        background(
            move || scan(&root),
            move |result| Message::LibraryLoaded(revision, result),
        )
    }

    pub(super) fn select_capture(&mut self, entry: Entry) {
        if self.offline_busy() || self.radio_busy() || self.closing || self.library.importing {
            return;
        }
        if Path::new(&self.recovery_capture_path) != entry.path {
            self.recovery_bssid.clear();
        }
        self.recovery_capture_path.clear();
        self.recovery_networks.clear();
        self.hash_path.clear();
        self.invalidate_preview();
        self.library.show = false;
        if entry.hash {
            self.engine = Engine::Hashcat;
            self.hash_path = entry.path.to_string_lossy().into_owned();
        } else {
            self.recovery_capture_path = entry.path.to_string_lossy().into_owned();
            self.prepare_recovery_input();
        }
        self.save_recovery();
    }

    pub(super) fn prepare_recovery_input(&mut self) {
        if self.offline_busy() || self.radio_busy() || self.recovery_capture_path.is_empty() {
            return;
        }
        if self.engine == Engine::Hashcat {
            if self.hash_path.is_empty() {
                self.run(Action::Convert);
            }
        } else if self.recovery_networks.is_empty() {
            self.run(Action::ReadCapture);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn library_lists_nonempty_recordings_and_ignores_generated_outputs_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("session")).unwrap();
        fs::create_dir(root.join("session/recovery")).unwrap();
        fs::write(root.join("session/capture-1-01.cap"), b"synthetic").unwrap();
        fs::write(root.join("session/empty.cap"), b"").unwrap();
        fs::write(root.join("session/survey.csv"), b"csv").unwrap();
        fs::write(root.join("session/recovery/handshake.hc22000"), b"hash").unwrap();
        fs::write(root.join("saved.HC22000"), b"hash").unwrap();
        symlink(root, root.join("loop")).unwrap();
        symlink(root.join("saved.HC22000"), root.join("link.cap")).unwrap();
        let entries = scan(root).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.hash));
        assert!(scan(&root.join("missing")).unwrap().is_empty());
    }

    #[test]
    fn imports_preserve_originals_names_and_private_permissions_without_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.cap");
        fs::write(&source, b"synthetic capture").unwrap();
        write_label(
            &source,
            &Label {
                ssid: "Research Wi-Fi".into(),
                bssid: "02:00:00:00:00:01".into(),
            },
        )
        .unwrap();
        let root = dir.path().join("library");
        let first = import(&root, &source).unwrap();
        let second = import(&root, &source).unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(first.name, "Research Wi-Fi");
        assert_eq!(fs::read(&source).unwrap(), fs::read(&first.path).unwrap());
        assert_eq!(
            fs::metadata(&first.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(first.path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(import(&root, &first.path).unwrap().path, first.path);
        assert_eq!(scan(&root).unwrap().len(), 2);
    }
}
