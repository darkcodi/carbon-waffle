//! Remember recovery inputs, never authorization or running processes.
use super::*;
use std::{
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Debug, Serialize, Deserialize)]
struct SavedRecovery {
    version: u32,
    flow: Flow,
    capture: String,
    hash: String,
    bssid: String,
    wordlist: String,
    mode: RecoveryMode,
    pattern: String,
    engine: Engine,
}

pub(super) fn state_path() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".local/state"))
        })
        .map(|path| path.join("carbon-waffle/recovery.json"))
}

fn read(path: &Path) -> Result<Option<SavedRecovery>, String> {
    let file = match fs::File::options()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Saved recovery settings are not a regular file.".into());
    }
    let mut data = Vec::new();
    file.take(65_537)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > 65_536 {
        return Err("Saved recovery settings are too large.".into());
    }
    let saved: SavedRecovery = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if saved.version != 1 {
        return Err("Unsupported recovery settings version.".into());
    }
    Ok(Some(saved))
}

fn write(path: &Path, saved: &SavedRecovery) -> Result<(), String> {
    let directory = path.parent().ok_or("Missing settings directory.")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|e| e.to_string())?;
    let data = serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?;
    if data.len() > 65_536 {
        return Err("Recovery settings are too large to save.".into());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = directory.join(format!(".recovery-{}-{stamp}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(&data)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}

impl App {
    pub(super) fn save_recovery(&mut self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let absolute = |value: &str| {
            absolute_path(value)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let saved = SavedRecovery {
            version: 1,
            flow: self.flow,
            capture: absolute(&self.recovery_capture_path),
            hash: absolute(&self.hash_path),
            bssid: self.recovery_bssid.clone(),
            wordlist: absolute(&self.wordlist),
            mode: self.recovery_mode,
            pattern: self.pattern.clone(),
            engine: self.engine,
        };
        self.save_error = write(path, &saved)
            .err()
            .map(|error| format!("Could not save recovery settings: {error}"));
    }

    pub(super) fn load_recovery(&mut self) {
        let Some(path) = &self.state_path else {
            return;
        };
        match read(path) {
            Ok(Some(saved)) => {
                self.flow = saved.flow;
                self.recovery_capture_path = saved.capture;
                self.hash_path = saved.hash;
                self.recovery_bssid = saved.bssid;
                self.wordlist = saved.wordlist;
                self.recovery_mode = saved.mode;
                self.pattern_check = Pattern::parse(&saved.pattern);
                self.pattern = saved.pattern;
                self.engine = saved.engine;
                if self.flow == Flow::Recover {
                    self.status =
                        "Recovery settings restored. Review the inputs, then start when ready."
                            .into();
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.save_error = Some(format!("Could not load recovery settings: {error}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn reopening_restores_inputs_without_authorization_or_jobs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/recovery.json");
        let mut app = App::new(false);
        app.state_path = Some(path.clone());
        app.flow = Flow::Recover;
        app.authorization = Authorization::Ready;
        app.recovery_capture_path = "captures/saved.cap".into();
        app.recovery_bssid = "02:00:00:00:00:01".into();
        app.hash_path = "/tmp/saved.hc22000".into();
        app.wordlist = "/tmp/words.txt".into();
        app.recovery_mode = RecoveryMode::Pattern;
        app.pattern = "{word}{5}".into();
        app.engine = Engine::Hashcat;
        app.save_recovery();
        assert!(app.save_error.is_none());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let mut reopened = App::new(false);
        reopened.state_path = Some(path.clone());
        reopened.load_recovery();
        assert_eq!(reopened.flow, Flow::Recover);
        assert_eq!(reopened.page, Page::Elevate);
        assert_eq!(reopened.authorization, Authorization::Idle);
        assert!(reopened.jobs.is_empty());
        assert!(reopened.target.is_none());
        assert_eq!(reopened.recovery_bssid, app.recovery_bssid);
        assert!(Path::new(&reopened.recovery_capture_path).is_absolute());
        assert_eq!(reopened.hash_path, app.hash_path);
        assert_eq!(reopened.engine, Engine::Hashcat);
        assert_eq!(reopened.recovery_mode, RecoveryMode::Pattern);
        assert_eq!(reopened.pattern, app.pattern);
        assert_eq!(reopened.wordlist, app.wordlist);
        assert!(reopened.operation(Action::Crack).is_ok());
        reopened.wordlist = "/tmp/new-words.txt".into();
        reopened.save_recovery();
        assert_eq!(read(&path).unwrap().unwrap().wordlist, reopened.wordlist);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn missing_or_corrupt_settings_leave_a_usable_app() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovery.json");
        assert!(read(&path).unwrap().is_none());
        fs::write(&path, b"not json").unwrap();
        let mut app = App::new(false);
        app.state_path = Some(path);
        app.load_recovery();
        assert!(app.save_error.is_some());
        assert_eq!(app.authorization, Authorization::Idle);
        let _ = app.update(Message::Flow(Flow::Recover));
        assert_eq!(app.flow, Flow::Recover);
        assert!(app.jobs.is_empty());
    }
}
