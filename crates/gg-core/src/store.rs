//! The persistent store: recent repositories, the open tabs and settings, kept in
//! `<config_dir>/state.json`.
//!
//! The file is read once, when the store is created; a missing file gives defaults and one
//! that cannot be parsed is renamed to `state.json.bad` first. A file that exists but cannot
//! be read (permissions, say), or a corrupt one that cannot be moved aside, also gives
//! defaults but is never written over: changes then apply for the running session only, and
//! [`Store::problem`] says why, until [`Store::reset`] moves the file aside. Otherwise every
//! change is written at once and atomically (a temp file renamed over the old one); a change
//! that cannot be written is an `Io` error and is not applied in memory either.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::dto::{Recent, Session, Settings, Theme};
use crate::error::{CoreError, ErrorCode, Result};

const FILE_NAME: &str = "state.json";
/// Where a file that could not be parsed is moved.
const BAD_FILE_NAME: &str = "state.json.bad";
/// Number of recents kept.
const MAX_RECENTS: usize = 20;
/// Allowed nonzero background fetch intervals, in seconds.
const FETCH_INTERVAL_RANGE: std::ops::RangeInclusive<u32> = 30..=86_400;

/// A change to [`Settings`]: only the fields present are changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub theme: Option<Theme>,
    pub fetch_interval_secs: Option<u32>,
}

/// The contents of `state.json`. A missing field takes its default, so a file written by an
/// older version still loads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoreData {
    recents: Vec<Recent>,
    session: Session,
    settings: Settings,
}

/// Recents, session and settings, persisted in `<config_dir>/state.json`.
pub struct Store {
    dir: PathBuf,
    data: Mutex<StoreData>,
    /// Why `state.json` is not saved to, when it exists but could not be read (or moved
    /// aside): it is not written over until [`Store::reset`].
    unreadable: Mutex<Option<String>>,
}

impl Store {
    /// Loads `<config_dir>/state.json`. Never fails: a missing file gives defaults, and a
    /// file that cannot be parsed is renamed to `state.json.bad` and gives defaults. A file
    /// that cannot be read gives defaults too, and makes every change fail rather than
    /// replace it. The directory is created on the first write.
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(FILE_NAME);
        let mut unreadable = None;
        let data = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<StoreData>(&bytes) {
                Ok(data) => data.sanitized(),
                Err(parse) => {
                    // Kept for the user to look at. If it cannot be moved aside, it is not
                    // written over either.
                    if let Err(e) = fs::rename(&path, config_dir.join(BAD_FILE_NAME)) {
                        unreadable = Some(format!(
                            "{} could not be parsed ({parse}) and could not be moved aside ({e})",
                            path.display()
                        ));
                    }
                    StoreData::default()
                }
            },
            // Missing (or its folder is not a folder): nothing to keep; a write reports it.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                StoreData::default()
            }
            Err(e) => {
                unreadable = Some(format!("{} could not be read ({e})", path.display()));
                StoreData::default()
            }
        };
        Self {
            dir: config_dir.to_path_buf(),
            data: Mutex::new(data),
            unreadable: Mutex::new(unreadable),
        }
    }

    /// Why changes are not being saved (the settings file exists but could not be read or
    /// moved aside), or `None` when they are.
    pub fn problem(&self) -> Option<String> {
        lock(&self.unreadable).clone()
    }

    /// Moves a settings file that could not be read aside, to
    /// `state.json.unreadable-<unix ms>`, and saves what the session has. Fails, changing
    /// nothing, if the file cannot be moved. Does nothing when there is no problem.
    pub fn reset(&self) -> Result<()> {
        let mut unreadable = lock(&self.unreadable);
        if unreadable.is_none() {
            return Ok(());
        }
        let path = self.dir.join(FILE_NAME);
        let aside = self
            .dir
            .join(format!("{FILE_NAME}.unreadable-{}", unix_ms()));
        fs::rename(&path, &aside).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("Could not move {} aside", path.display()),
            )
            .with_detail(e.to_string())
        })?;
        *unreadable = None;
        drop(unreadable);
        let data = self.lock();
        self.write(&data).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("Could not save settings to {}", self.dir.display()),
            )
            .with_detail(e.to_string())
        })
    }

    /// Most recent first.
    pub fn recents(&self) -> Vec<Recent> {
        self.lock().recents.clone()
    }

    pub fn session(&self) -> Session {
        self.lock().session.clone()
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    /// Puts the repository at `path` first in the recents, removing any other entry for the
    /// same folder (compared with symlinks resolved), and keeps the newest 20.
    pub fn add_recent(&self, path: &Path) -> Result<()> {
        let path = canonical(path);
        let recent = Recent {
            path: path.to_string_lossy().into_owned(),
            name: path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            last_opened_ms: unix_ms(),
        };
        self.change(|data| {
            data.recents.retain(|r| !same_folder(&r.path, &path));
            data.recents.insert(0, recent);
            data.recents.truncate(MAX_RECENTS);
            Ok(())
        })
    }

    /// Removes the recent for `path` (as listed, or another spelling of the same folder);
    /// returns the recents left. Removing a path that is not listed changes nothing.
    pub fn remove_recent(&self, path: &str) -> Result<Vec<Recent>> {
        let target = canonical(Path::new(path));
        let listed = |r: &Recent| r.path == path || same_folder(&r.path, &target);
        if !self.lock().recents.iter().any(listed) {
            return Ok(self.recents());
        }
        self.change(|data| {
            data.recents.retain(|r| !listed(r));
            Ok(data.recents.clone())
        })
    }

    pub fn save_session(&self, session: Session) -> Result<()> {
        self.change(|data| {
            data.session = session;
            Ok(())
        })
    }

    /// Applies `patch` and returns the new settings. `BadRequest` (and nothing changes) if
    /// `fetchIntervalSecs` is neither 0 nor within 30..=86400.
    pub fn update_settings(&self, patch: SettingsPatch) -> Result<Settings> {
        if let Some(secs) = patch.fetch_interval_secs
            && !valid_fetch_interval(secs)
        {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!("The fetch interval must be 0 (off) or 30 to 86400 seconds, not {secs}."),
            ));
        }
        self.change(|data| {
            if let Some(theme) = patch.theme {
                data.settings.theme = theme;
            }
            if let Some(secs) = patch.fetch_interval_secs {
                data.settings.fetch_interval_secs = secs;
            }
            Ok(data.settings.clone())
        })
    }

    /// Applies `f` to a copy of the data and writes it; the copy replaces the data in memory
    /// only once it is written. Holds the lock throughout, so writes land in order. While the
    /// file is not saved to (see [`Store::problem`]), the change applies in memory only.
    fn change<R>(&self, f: impl FnOnce(&mut StoreData) -> Result<R>) -> Result<R> {
        let mut data = self.lock();
        let mut next = data.clone();
        let out = f(&mut next)?;
        if lock(&self.unreadable).is_some() {
            *data = next;
            return Ok(out);
        }
        self.write(&next).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("Could not save settings to {}", self.dir.display()),
            )
            .with_detail(e.to_string())
        })?;
        *data = next;
        Ok(out)
    }

    /// Writes `data` to a temp file next to `state.json` and renames it over it.
    fn write(&self, data: &StoreData) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec_pretty(data).map_err(io::Error::other)?;
        let tmp = self
            .dir
            .join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
        let written = (|| {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&json)?;
            file.sync_all()?;
            fs::rename(&tmp, self.dir.join(FILE_NAME))
        })();
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written
    }

    fn lock(&self) -> MutexGuard<'_, StoreData> {
        // Every change replaces the data whole, so a panic cannot leave it half-changed.
        lock(&self.data)
    }
}

impl StoreData {
    /// Replaces values a hand-edited file may hold but the app never writes.
    fn sanitized(mut self) -> Self {
        self.recents.truncate(MAX_RECENTS);
        if !valid_fetch_interval(self.settings.fetch_interval_secs) {
            self.settings.fetch_interval_secs = Settings::default().fetch_interval_secs;
        }
        self
    }
}

fn valid_fetch_interval(secs: u32) -> bool {
    secs == 0 || FETCH_INTERVAL_RANGE.contains(&secs)
}

/// Whether the recent at `listed` is the folder `canonical_path`.
fn same_folder(listed: &str, canonical_path: &Path) -> bool {
    canonical(Path::new(listed)) == canonical_path
}

/// `path` with symlinks resolved (unchanged if it cannot be, for example a deleted folder).
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The config folder to use instead of the platform's: `GG_CONFIG_DIR` when it is set and
/// not empty. Performance runs use it so they never read or change the user's own state.
pub fn config_dir_override() -> Option<PathBuf> {
    config_dir_from(std::env::var_os("GG_CONFIG_DIR"))
}

fn config_dir_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::Theme;
    use crate::error::ErrorCode;
    use std::fs;

    fn patch(v: serde_json::Value) -> SettingsPatch {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn defaults_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load(&dir.path().join("not-yet-created"));
        assert!(store.recents().is_empty());
        assert_eq!(store.session(), Session::default());
        let settings = store.settings();
        assert_eq!(settings.theme, Theme::System);
        assert_eq!(settings.fetch_interval_secs, 300);
    }

    #[test]
    fn corrupt_file_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("state.json"), "{ not json").unwrap();
        let store = Store::load(dir.path());
        assert!(store.recents().is_empty());
        assert_eq!(store.settings().fetch_interval_secs, 300);
        assert!(!dir.path().join("state.json").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("state.json.bad")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn recents_dedupe_order_cap() {
        let config = tempfile::tempdir().unwrap();
        let repos = tempfile::tempdir().unwrap();
        let dirs: Vec<_> = (0..25)
            .map(|i| {
                let d = repos.path().join(format!("repo {i}"));
                fs::create_dir(&d).unwrap();
                dunce::canonicalize(d).unwrap()
            })
            .collect();
        let store = Store::load(config.path());
        store.add_recent(&dirs[0]).unwrap();
        store.add_recent(&dirs[1]).unwrap();
        // Another spelling of the same folder counts as the same recent.
        store.add_recent(&dirs[0].join(".")).unwrap();
        let paths =
            |s: &Store| -> Vec<String> { s.recents().into_iter().map(|r| r.path).collect() };
        let s = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(paths(&store), vec![s(&dirs[0]), s(&dirs[1])]);
        let first = &store.recents()[0];
        assert_eq!(first.name, "repo 0");
        assert!(first.last_opened_ms > 0);

        for d in &dirs {
            store.add_recent(d).unwrap();
        }
        let all = paths(&store);
        assert_eq!(all.len(), 20);
        assert_eq!(all[0], s(&dirs[24]));
        assert_eq!(all[19], s(&dirs[5]));
    }

    #[test]
    fn remove_recent_returns_the_rest() {
        let config = tempfile::tempdir().unwrap();
        let repos = tempfile::tempdir().unwrap();
        let a = repos.path().join("a");
        let b = repos.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let store = Store::load(config.path());
        store.add_recent(&a).unwrap();
        store.add_recent(&b).unwrap();
        let a_path = store.recents()[1].path.clone();
        // Its folder is gone: still removable by the path the recent lists.
        fs::remove_dir(&a).unwrap();
        let rest = store.remove_recent(&a_path).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].name, "b");
        assert_eq!(Store::load(config.path()).recents(), rest);
    }

    #[test]
    fn roundtrip_persists() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        {
            let store = Store::load(config.path());
            store.add_recent(repo.path()).unwrap();
            store
                .save_session(Session {
                    tabs: vec!["/x".into(), "/y".into()],
                    active: Some("/y".into()),
                })
                .unwrap();
            store
                .update_settings(patch(serde_json::json!({"theme": "dark"})))
                .unwrap();
        }
        let again = Store::load(config.path());
        assert_eq!(again.recents().len(), 1);
        assert_eq!(
            again.session(),
            Session {
                tabs: vec!["/x".into(), "/y".into()],
                active: Some("/y".into()),
            }
        );
        assert_eq!(again.settings().theme, Theme::Dark);
        assert_eq!(again.settings().fetch_interval_secs, 300);
        // Written atomically: no temp file is left behind.
        let names: Vec<_> = fs::read_dir(config.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("state.json")]);
    }

    #[test]
    fn settings_validation() {
        let config = tempfile::tempdir().unwrap();
        let store = Store::load(config.path());
        let err = store
            .update_settings(patch(serde_json::json!({"fetchIntervalSecs": 5})))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(store.settings().fetch_interval_secs, 300, "unchanged");
        for bad in [29, 86_401] {
            let p = patch(serde_json::json!({ "fetchIntervalSecs": bad }));
            assert!(store.update_settings(p).is_err(), "{bad} accepted");
        }
        for ok in [0, 30, 300, 86_400] {
            let p = patch(serde_json::json!({ "fetchIntervalSecs": ok }));
            assert_eq!(store.update_settings(p).unwrap().fetch_interval_secs, ok);
        }
        // A patch changes only what it names.
        let s = store
            .update_settings(patch(serde_json::json!({"theme": "light"})))
            .unwrap();
        assert_eq!(s.theme, Theme::Light);
        assert_eq!(s.fetch_interval_secs, 86_400);
        assert!(
            serde_json::from_value::<SettingsPatch>(serde_json::json!({"color": 1})).is_err(),
            "unknown settings are rejected"
        );
    }

    /// A file that exists but cannot be read (here: no permission) is the user's data: it
    /// gives defaults for now and is never written over. Changes still apply for the running
    /// session, and the store says once, with the cause, why they are not saved.
    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_never_overwritten_and_changes_apply_in_memory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let contents = r#"{"settings":{"theme":"dark","fetchIntervalSecs":600}}"#;
        fs::write(&path, contents).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let store = Store::load(dir.path());
        assert_eq!(store.settings().fetch_interval_secs, 300);
        let problem = store.problem().expect("the problem is reported");
        assert!(problem.contains("could not be read"), "{problem}");
        assert!(problem.contains("ermission denied"), "the cause: {problem}");

        let session = Session {
            tabs: vec!["/a".into()],
            active: Some("/a".into()),
        };
        store.save_session(session.clone()).unwrap();
        assert_eq!(store.session(), session);
        let settings = store
            .update_settings(patch(serde_json::json!({"fetchIntervalSecs": 60})))
            .unwrap();
        assert_eq!(settings.fetch_interval_secs, 60);
        assert_eq!(store.settings().fetch_interval_secs, 60);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }

    /// A corrupt file that cannot be moved aside says so, with both causes.
    #[cfg(unix)]
    #[test]
    fn corrupt_file_that_cannot_be_moved_aside_reports_why() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        fs::create_dir(&config).unwrap();
        fs::write(config.join("state.json"), "{ not json").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o555)).unwrap();
        let store = Store::load(&config);
        let problem = store.problem();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
        let problem = problem.expect("the problem is reported");
        assert!(problem.contains("could not be parsed"), "{problem}");
        assert!(problem.contains("could not be moved aside"), "{problem}");
        assert!(problem.contains("ermission denied"), "{problem}");
    }

    /// Resetting moves the unreadable file aside (state.json.unreadable-<ms>) and saving
    /// resumes, with what the session changed meanwhile.
    #[cfg(unix)]
    #[test]
    fn reset_moves_the_unreadable_file_aside_and_saving_resumes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let store = Store::load(dir.path());
        store
            .update_settings(patch(serde_json::json!({"theme": "dark"})))
            .unwrap();
        store.reset().unwrap();
        assert_eq!(store.problem(), None);
        let aside: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("state.json.unreadable-"))
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["settings"]["theme"], "dark");
        store.save_session(Session::default()).unwrap();
        for entry in fs::read_dir(dir.path()).unwrap() {
            let p = entry.unwrap().path();
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o644));
        }
    }

    /// When the file cannot be moved aside, reset fails and nothing is written.
    #[cfg(unix)]
    #[test]
    fn reset_that_cannot_move_the_file_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        fs::create_dir(&config).unwrap();
        let path = config.join("state.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o555)).unwrap();
        let store = Store::load(&config);
        let result = store.reset();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(result.unwrap_err().code, ErrorCode::Io);
        assert!(store.problem().is_some());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
    }

    #[test]
    fn save_failure_is_io() {
        let dir = tempfile::tempdir().unwrap();
        // The config dir path is taken by a file, so nothing can be written there.
        let blocked = dir.path().join("config");
        fs::write(&blocked, "").unwrap();
        let store = Store::load(&blocked);
        let err = store.save_session(Session::default()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Io);
    }

    #[test]
    fn config_dir_override_needs_a_non_empty_value() {
        assert_eq!(
            config_dir_from(Some("/tmp/gg perf/config".into())),
            Some(PathBuf::from("/tmp/gg perf/config"))
        );
        assert_eq!(config_dir_from(Some("".into())), None);
        assert_eq!(config_dir_from(None), None);
    }
}
