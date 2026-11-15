//! The persistent store: recent repositories, the open tabs, settings and per-repository
//! preferences, kept in `<config_dir>/state.json`.
//!
//! The file is read once, when the store is created; a missing file gives defaults. It is read
//! field by field, so a value this version does not know gives that field's default; a file
//! that is not a JSON object at all is renamed to `state.json.bad-<ms>` first. A file that exists but
//! cannot be read (permissions, say), or a corrupt one that cannot be moved aside, also gives
//! defaults but is never written over: changes then apply for the running session only, and
//! [`Store::problem`] says why, until [`Store::reset`] moves the file aside. Otherwise every
//! change is written at once and atomically (a temp file renamed over the old one); a change
//! that cannot be written is an `Io` error and is not applied in memory either.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::dto::{Recent, RepoPrefs, Session, Settings, Theme};
use crate::error::{CoreError, ErrorCode, Result};

const FILE_NAME: &str = "state.json";
/// Where a file that could not be parsed is moved: `state.json.bad-<unix ms>`, so an earlier
/// one is kept.
const BAD_FILE_PREFIX: &str = "state.json.bad-";
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
    /// `Some(None)` (JSON `null`) goes back to the system's default opener.
    #[serde(default, deserialize_with = "present")]
    pub editor_command: Option<Option<String>>,
}

/// A change to [`RepoPrefs`]: only the fields present are changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepoPrefsPatch {
    pub merge_no_ff: Option<bool>,
}

/// Reads a field that is present, `null` included, as `Some`; a missing one stays `None`
/// (through `default`).
fn present<'de, D, T>(d: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

/// The contents of `state.json`. Read with [`StoreData::from_json`]: a missing or unreadable
/// field takes its default, so a file written by an older (or newer) version still loads.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoreData {
    recents: Vec<Recent>,
    session: Session,
    settings: Settings,
    /// By the repository's main worktree path, as recents store it.
    repo_prefs: BTreeMap<String, RepoPrefs>,
}

/// Recents, session and settings, persisted in `<config_dir>/state.json`.
pub struct Store {
    dir: PathBuf,
    data: Mutex<StoreData>,
    /// Why `state.json` is not saved to, when it exists but could not be read (or moved
    /// aside): it is not written over until [`Store::reset`].
    unreadable: Mutex<Option<String>>,
    /// The file's top-level object as read, so that saving keeps what this version does not
    /// know (keys of a newer version), and whether the file as it was is still to be copied
    /// aside before the first save (it held a value that fell back, which saving loses).
    original: Mutex<Original>,
}

/// See [`Store::original`].
#[derive(Default)]
struct Original {
    top: Map<String, Value>,
    copy_aside: bool,
}

impl Store {
    /// Loads `<config_dir>/state.json`. Never fails: a missing file gives defaults, a value
    /// that cannot be read gives that field's default, and a file that is not a JSON object
    /// is renamed to `state.json.bad-<ms>` and gives defaults. A
    /// file that cannot be read gives defaults too, and is never written over: changes apply
    /// in memory (see [`Store::problem`]). The directory is created on the first write.
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(FILE_NAME);
        let mut unreadable = None;
        let mut original = Original::default();
        let data = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|v| StoreData::from_json(v.clone()).map(|d| (d.sanitized(), v)))
            {
                Ok((data, Value::Object(top))) => {
                    original.copy_aside = data.lost_something_of(&top);
                    original.top = top;
                    data
                }
                Ok((data, _)) => data,
                Err(parse) => {
                    // Kept for the user to look at. If it cannot be moved aside, it is not
                    // written over either.
                    let aside = config_dir.join(format!("{BAD_FILE_PREFIX}{}", unix_ms()));
                    if let Err(e) = fs::rename(&path, aside) {
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
            original: Mutex::new(original),
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
        // Saving resumes only once a save works; until then changes stay in memory.
        let data = self.lock();
        match self.write(&data) {
            Ok(()) => {
                *unreadable = None;
                Ok(())
            }
            Err(e) => {
                *unreadable = Some(format!(
                    "{} was moved aside to {}, but settings could not be saved ({e})",
                    path.display(),
                    aside.display()
                ));
                Err(CoreError::new(
                    ErrorCode::Io,
                    format!("Could not save settings to {}", self.dir.display()),
                )
                .with_detail(e.to_string()))
            }
        }
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
    /// same folder, and keeps the newest 20. `path` is stored with symlinks resolved; the
    /// stored ones were stored that way, so they are compared as text and their folders are
    /// never touched (one on an unreachable volume would stall this).
    pub fn add_recent(&self, path: &Path) -> Result<()> {
        let path = canonical(path);
        let text = path.to_string_lossy().into_owned();
        let recent = Recent {
            path: text.clone(),
            name: path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            last_opened_ms: unix_ms(),
        };
        self.change(|data| {
            data.recents.retain(|r| r.path != text);
            data.recents.insert(0, recent);
            data.recents.truncate(MAX_RECENTS);
            Ok(())
        })
    }

    /// Removes the recent for `path`, as listed; returns the recents left. Removing a path
    /// that is not listed changes nothing. The folder is never touched.
    pub fn remove_recent(&self, path: &str) -> Result<Vec<Recent>> {
        let listed = |r: &Recent| r.path == path;
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

    /// The preferences of the repository whose main worktree is `root` (defaults if none
    /// were set). Keyed by the path with symlinks resolved, as recents are.
    pub fn repo_prefs(&self, root: &Path) -> RepoPrefs {
        let key = canonical(root).to_string_lossy().into_owned();
        self.lock()
            .repo_prefs
            .get(&key)
            .copied()
            .unwrap_or_default()
    }

    /// Applies `patch` to the preferences of the repository at `root`; returns them.
    pub fn set_repo_prefs(&self, root: &Path, patch: RepoPrefsPatch) -> Result<RepoPrefs> {
        let key = canonical(root).to_string_lossy().into_owned();
        self.change(|data| {
            let prefs = data.repo_prefs.entry(key).or_default();
            if let Some(no_ff) = patch.merge_no_ff {
                prefs.merge_no_ff = no_ff;
            }
            Ok(*prefs)
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
            if let Some(command) = patch.editor_command {
                // A blank command means the default opener, like `null`.
                data.settings.editor_command = command.filter(|c| !c.trim().is_empty());
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
    /// Writes `data` over the file's original object, so keys this version does not know
    /// stay; first copies the file aside once if a value of it fell back (see `load`).
    fn write(&self, data: &StoreData) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut original = lock(&self.original);
        if original.copy_aside {
            copy_aside(&self.dir)?;
            original.copy_aside = false;
        }
        let json = serde_json::to_vec_pretty(&data.merged_into(&original.top))
            .map_err(io::Error::other)?;
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
    /// The data in `value`, field by field: a field (or a recent, or a tab) this version
    /// cannot read takes its default or is left out, so one unknown value (a theme added by
    /// a newer version, a hand edit) loses only itself. `Err` only when `value` is not an
    /// object.
    fn from_json(value: serde_json::Value) -> std::result::Result<Self, String> {
        use serde::de::DeserializeOwned;
        use serde_json::Value;

        fn field<T: DeserializeOwned>(
            object: &serde_json::Map<String, Value>,
            key: &str,
        ) -> Option<T> {
            object
                .get(key)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        }
        /// The elements of the array at `key` that read as `T`.
        fn list<T: DeserializeOwned>(object: &serde_json::Map<String, Value>, key: &str) -> Vec<T> {
            match object.get(key) {
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect(),
                _ => Vec::new(),
            }
        }

        let Value::Object(top) = value else {
            return Err("the file does not hold a JSON object".to_owned());
        };
        let empty = serde_json::Map::new();
        let object = |key: &str| match top.get(key) {
            Some(Value::Object(o)) => o,
            _ => &empty,
        };
        let (session, settings) = (object("session"), object("settings"));
        let defaults = Settings::default();
        let repo_prefs = object("repoPrefs")
            .iter()
            .filter_map(|(root, prefs)| {
                let Value::Object(prefs) = prefs else {
                    return None;
                };
                let merge_no_ff =
                    field(prefs, "mergeNoFf").unwrap_or(RepoPrefs::default().merge_no_ff);
                Some((root.clone(), RepoPrefs { merge_no_ff }))
            })
            .collect();
        Ok(Self {
            recents: list(&top, "recents"),
            session: Session {
                tabs: list(session, "tabs"),
                active: field(session, "active"),
            },
            settings: Settings {
                theme: field(settings, "theme").unwrap_or(defaults.theme),
                fetch_interval_secs: field(settings, "fetchIntervalSecs")
                    .unwrap_or(defaults.fetch_interval_secs),
                editor_command: field(settings, "editorCommand"),
            },
            repo_prefs,
        })
    }

    /// Whether reading `top` lost a value it held: a field present there that this data
    /// would write differently (an unknown theme, an unreadable recent, a bad interval).
    fn lost_something_of(&self, top: &Map<String, Value>) -> bool {
        let Ok(Value::Object(known)) = serde_json::to_value(self) else {
            return false;
        };
        let differs = |was: &Map<String, Value>, now: &Map<String, Value>, key: &str| {
            was.get(key).is_some_and(|v| Some(v) != now.get(key))
        };
        let lost_inner = |key: &str, fields: &[&str]| match (top.get(key), known.get(key)) {
            (None, _) => false,
            (Some(Value::Object(was)), Some(Value::Object(now))) => {
                fields.iter().any(|f| differs(was, now, f))
            }
            _ => true,
        };
        // Each repository's entry as one object of known fields.
        let lost_prefs = match (top.get("repoPrefs"), known.get("repoPrefs")) {
            (None, _) => false,
            (Some(Value::Object(was)), Some(Value::Object(now))) => {
                was.iter()
                    .any(|(root, entry)| match (entry, now.get(root)) {
                        (Value::Object(was), Some(Value::Object(now))) => {
                            differs(was, now, "mergeNoFf")
                        }
                        _ => true,
                    })
            }
            _ => true,
        };
        differs(top, &known, "recents")
            || lost_inner("session", &["tabs", "active"])
            || lost_inner("settings", &["theme", "fetchIntervalSecs", "editorCommand"])
            || lost_prefs
    }

    /// `top` with this data's fields written over it: keys this version does not know, at
    /// any depth inside objects (`session`, `settings`, a repository's preferences), are
    /// kept.
    fn merged_into(&self, top: &Map<String, Value>) -> Map<String, Value> {
        fn merge(into: &mut Map<String, Value>, from: Map<String, Value>) {
            for (key, value) in from {
                match (into.get_mut(&key), value) {
                    (Some(Value::Object(old)), Value::Object(new)) => merge(old, new),
                    (_, value) => {
                        into.insert(key, value);
                    }
                }
            }
        }
        let mut merged = top.clone();
        if let Ok(Value::Object(known)) = serde_json::to_value(self) {
            merge(&mut merged, known);
        }
        merged
    }

    /// Replaces values a hand-edited file may hold but the app never writes.
    fn sanitized(mut self) -> Self {
        self.recents.truncate(MAX_RECENTS);
        if !valid_fetch_interval(self.settings.fetch_interval_secs) {
            self.settings.fetch_interval_secs = Settings::default().fetch_interval_secs;
        }
        self
    }
}

/// Copies `state.json` in `dir` to `state.json.before-<ms>` (with `-<n>` added if that
/// exists): the file as it was before this version first saved over it.
fn copy_aside(dir: &Path) -> io::Result<()> {
    let from = dir.join(FILE_NAME);
    let stamp = unix_ms();
    let mut to = dir.join(format!("{FILE_NAME}.before-{stamp}"));
    let mut n = 1;
    while to.exists() {
        to = dir.join(format!("{FILE_NAME}.before-{stamp}-{n}"));
        n += 1;
    }
    match fs::copy(&from, &to) {
        Ok(_) => Ok(()),
        // Gone meanwhile: nothing left to keep.
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn valid_fetch_interval(secs: u32) -> bool {
    secs == 0 || FETCH_INTERVAL_RANGE.contains(&secs)
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

/// The app's config folder: `GG_CONFIG_DIR` when it is set and not empty (performance runs
/// use it so they never read or change the user's own state), and nothing else happens;
/// otherwise `platform_dir()`, into which the state of the app's config folder from before
/// it was renamed is first copied, once (see [`migrate_config`]). Call it once, before the
/// store loads. A copy that fails is reported on stderr and the app starts without it.
pub fn config_dir(platform_dir: impl FnOnce() -> Result<PathBuf>) -> Result<PathBuf> {
    config_dir_with(std::env::var_os("GG_CONFIG_DIR"), platform_dir)
}

fn config_dir_with(
    gg_config_dir: Option<std::ffi::OsString>,
    platform_dir: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    if let Some(dir) = config_dir_from(gg_config_dir) {
        return Ok(dir);
    }
    let dir = platform_dir()?;
    if let Some(old) = old_config_dir(&dir)
        && let Err(e) = migrate_config(&old, &dir)
    {
        eprintln!("gg: {e}: {}", e.detail.as_deref().unwrap_or_default());
    }
    Ok(dir)
}

fn config_dir_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The name of the app's config folder before the app was renamed to gg (its identifier
/// then), in the same parent folder as the current one.
const OLD_CONFIG_DIR_NAME: &str = "dev.gitgraph.desktop";

/// Where the app kept its config before it was renamed: the folder next to `new_dir` named
/// after its old identifier. `None` when `new_dir` has no parent.
pub fn old_config_dir(new_dir: &Path) -> Option<PathBuf> {
    new_dir
        .parent()
        .map(|parent| parent.join(OLD_CONFIG_DIR_NAME))
}

/// Copies `state.json` from `old_dir` into `new_dir` (created if need be) when `new_dir` is
/// empty: true when it copied. Once the app has written anything there (its state file, or
/// a broken one it moved aside), nothing is copied again, and an existing state file is
/// never written over; `old_dir` is left as it is. The copy is written to a temp file and
/// renamed into place, so it is whole or absent.
pub fn migrate_config(old_dir: &Path, new_dir: &Path) -> Result<bool> {
    let from = old_dir.join(FILE_NAME);
    let failed = |e: io::Error| {
        CoreError::new(
            ErrorCode::Io,
            format!(
                "Could not copy the settings from {} to {}",
                from.display(),
                new_dir.display()
            ),
        )
        .with_detail(e.to_string())
    };
    if !is_empty_dir(new_dir).map_err(failed)? {
        return Ok(false);
    }
    let bytes = match fs::read(&from) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(failed(e)),
    };
    let tmp = new_dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    let written = (|| {
        fs::create_dir_all(new_dir)?;
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, new_dir.join(FILE_NAME))
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(failed(e));
    }
    Ok(true)
}

/// Whether `dir` is missing or holds nothing but hidden files (a `.DS_Store` the Finder
/// left, say).
fn is_empty_dir(dir: &Path) -> io::Result<bool> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e),
    };
    for entry in entries {
        if !entry?.file_name().to_string_lossy().starts_with('.') {
            return Ok(false);
        }
    }
    Ok(true)
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
        // One set aside before must survive the next.
        fs::write(dir.path().join("state.json.bad"), "older").unwrap();
        fs::write(dir.path().join("state.json"), "{ not json").unwrap();
        let store = Store::load(dir.path());
        assert!(store.recents().is_empty());
        assert_eq!(store.settings().fetch_interval_secs, 300);
        assert!(!dir.path().join("state.json").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("state.json.bad")).unwrap(),
            "older"
        );
        let aside: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("state.json.bad-"))
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert_eq!(
            fs::read_to_string(dir.path().join(&aside[0])).unwrap(),
            "{ not json"
        );
    }

    /// A value this version does not know (a theme from a newer version, a hand edit) loses
    /// only itself: everything else in the file is kept, and the file is not set aside.
    #[test]
    fn unknown_values_fall_back_one_field_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let file = r#"{
            "recents": [
                {"path": "/a", "name": "a", "lastOpenedMs": 5},
                {"path": "/b", "name": 7, "lastOpenedMs": 6},
                "junk",
                {"path": "/c", "name": "c", "lastOpenedMs": 8}
            ],
            "session": {"tabs": ["/a", 3, "/c"], "active": "/c"},
            "settings": {"theme": "neon", "fetchIntervalSecs": 600},
            "future": {"anything": true}
        }"#;
        fs::write(dir.path().join("state.json"), file).unwrap();
        let store = Store::load(dir.path());
        assert_eq!(store.problem(), None);
        let paths: Vec<_> = store.recents().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, ["/a", "/c"]);
        assert_eq!(
            store.session(),
            Session {
                tabs: vec!["/a".into(), "/c".into()],
                active: Some("/c".into()),
            }
        );
        assert_eq!(store.settings().theme, Theme::System);
        assert_eq!(store.settings().fetch_interval_secs, 600);
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("state.json")]);

        // A field of the wrong type entirely gives that field's default.
        fs::write(
            dir.path().join("state.json"),
            r#"{"recents": {}, "session": [], "settings": {"theme": "dark", "fetchIntervalSecs": "x"}}"#,
        )
        .unwrap();
        let store = Store::load(dir.path());
        assert!(store.recents().is_empty());
        assert_eq!(store.session(), Session::default());
        assert_eq!(store.settings().theme, Theme::Dark);
        assert_eq!(store.settings().fetch_interval_secs, 300);
    }

    /// Saving keeps what this version does not know: keys of a newer version, at the top
    /// and inside `session` and `settings`, are written back as they were.
    #[test]
    fn unknown_keys_survive_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(
            &path,
            r#"{"future": {"x": [1, 2]},
                "session": {"tabs": ["/a"], "active": "/a", "pinned": ["/a"]},
                "settings": {"theme": "dark", "fetchIntervalSecs": 600, "newSetting": true}}"#,
        )
        .unwrap();
        let store = Store::load(dir.path());
        store
            .save_session(Session {
                tabs: vec!["/b".into()],
                active: Some("/b".into()),
            })
            .unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["future"], serde_json::json!({"x": [1, 2]}));
        assert_eq!(saved["session"]["tabs"], serde_json::json!(["/b"]));
        assert_eq!(saved["session"]["pinned"], serde_json::json!(["/a"]));
        assert_eq!(saved["settings"]["newSetting"], true);
        assert_eq!(saved["settings"]["theme"], "dark");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names.len(),
            1,
            "nothing fell back, nothing set aside: {names:?}"
        );
    }

    /// A value that fell back would be lost by the first save: the file as it was is copied
    /// aside once, under a name of its own, before that save.
    #[test]
    fn a_file_with_values_that_fell_back_is_copied_aside_before_the_first_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let original = r#"{"settings": {"theme": "neon", "fetchIntervalSecs": 300}, "extra": 1}"#;
        fs::write(&path, original).unwrap();
        let store = Store::load(dir.path());
        store.save_session(Session::default()).unwrap();
        store.save_session(Session::default()).unwrap();
        let aside: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("state.json.before-"))
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert_eq!(
            fs::read_to_string(dir.path().join(&aside[0])).unwrap(),
            original
        );
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["settings"]["theme"], "system");
        assert_eq!(saved["extra"], 1);
    }

    /// JSON that is not an object holds nothing this version could use: set aside too.
    #[test]
    fn json_that_is_not_an_object_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("state.json"), "[1, 2]").unwrap();
        let store = Store::load(dir.path());
        assert!(store.recents().is_empty());
        assert!(!dir.path().join("state.json").exists());
    }

    /// A reset that moves the file aside but cannot save keeps the session's changes in
    /// memory and keeps saying why they are not saved, instead of failing every later save.
    #[cfg(unix)]
    #[test]
    fn reset_that_cannot_save_keeps_saving_off() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let store = Store::load(dir.path());
        // The temp file the write would create is taken by a folder.
        fs::create_dir(
            dir.path()
                .join(format!("state.json.{}.tmp", std::process::id())),
        )
        .unwrap();
        let err = store.reset().unwrap_err();
        assert_eq!(err.code, ErrorCode::Io);
        let problem = store.problem().expect("still not saving");
        assert!(problem.contains("could not be saved"), "{problem}");
        store
            .save_session(Session::default())
            .expect("changes still apply in memory");
        for entry in fs::read_dir(dir.path()).unwrap() {
            let _ = fs::set_permissions(entry.unwrap().path(), fs::Permissions::from_mode(0o755));
        }
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

    /// Stored recents are canonical already and are compared as text: adding one never
    /// touches the folders of the others (one on an unreachable network volume would stall
    /// every open). Seen here through a symlink entry, which is not resolved.
    #[cfg(unix)]
    #[test]
    fn add_recent_never_resolves_the_stored_paths() {
        let config = tempfile::tempdir().unwrap();
        let repos = tempfile::tempdir().unwrap();
        let real = dunce::canonicalize(repos.path()).unwrap().join("real");
        fs::create_dir(&real).unwrap();
        let link = real.with_file_name("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        fs::write(
            config.path().join("state.json"),
            serde_json::json!({"recents": [
                {"path": link.to_string_lossy(), "name": "link", "lastOpenedMs": 1}
            ]})
            .to_string(),
        )
        .unwrap();
        let store = Store::load(config.path());
        store.add_recent(&real).unwrap();
        let paths: Vec<_> = store.recents().into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            [
                real.to_string_lossy().into_owned(),
                link.to_string_lossy().into_owned()
            ]
        );
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
    fn editor_command_is_set_and_cleared() {
        let config = tempfile::tempdir().unwrap();
        let store = Store::load(config.path());
        assert_eq!(store.settings().editor_command, None);
        let s = store
            .update_settings(patch(serde_json::json!({"editorCommand": "code -w"})))
            .unwrap();
        assert_eq!(s.editor_command.as_deref(), Some("code -w"));
        // A patch without it keeps it.
        store
            .update_settings(patch(serde_json::json!({"theme": "dark"})))
            .unwrap();
        assert_eq!(
            Store::load(config.path())
                .settings()
                .editor_command
                .as_deref(),
            Some("code -w")
        );
        let s = store
            .update_settings(patch(serde_json::json!({"editorCommand": null})))
            .unwrap();
        assert_eq!(s.editor_command, None);
        let s = store
            .update_settings(patch(serde_json::json!({"editorCommand": "  "})))
            .unwrap();
        assert_eq!(s.editor_command, None, "blank is the default opener");
    }

    /// Repository preferences are read field by field like the rest: a newer version's
    /// keys inside an entry survive a save, and a value that falls back copies the file
    /// aside first.
    #[test]
    fn repo_prefs_keep_unknown_keys_and_fall_back_one_value_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(
            &path,
            r#"{"repoPrefs": {"/a": {"mergeNoFf": false, "future": 1}, "/b": "junk"}}"#,
        )
        .unwrap();
        let store = Store::load(dir.path());
        assert!(!store.repo_prefs(Path::new("/a")).merge_no_ff);
        assert!(store.repo_prefs(Path::new("/b")).merge_no_ff);
        assert!(store.repo_prefs(Path::new("/c")).merge_no_ff);
        store
            .set_repo_prefs(
                Path::new("/c"),
                RepoPrefsPatch {
                    merge_no_ff: Some(false),
                },
            )
            .unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["repoPrefs"]["/a"],
            serde_json::json!({"mergeNoFf": false, "future": 1})
        );
        assert_eq!(
            saved["repoPrefs"]["/c"],
            serde_json::json!({"mergeNoFf": false})
        );
        let aside = fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("state.json.before-")
            })
            .count();
        assert_eq!(aside, 1, "the unreadable entry for /b was kept aside");
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

    /// A config folder of the app under its old name, holding `state` as its state file,
    /// and the path of the new one next to it (not created).
    fn old_and_new(root: &Path, state: &str) -> (PathBuf, PathBuf) {
        let old = root.join(OLD_CONFIG_DIR_NAME);
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join(FILE_NAME), state).unwrap();
        (old, root.join("com.shakirul.gg"))
    }

    #[test]
    fn migrate_config_copies_the_old_state_once_and_keeps_the_old_folder() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = old_and_new(root.path(), r#"{"recents":[]}"#);
        assert!(migrate_config(&old, &new).unwrap());
        assert_eq!(
            fs::read_to_string(new.join(FILE_NAME)).unwrap(),
            r#"{"recents":[]}"#
        );
        assert_eq!(
            fs::read_to_string(old.join(FILE_NAME)).unwrap(),
            r#"{"recents":[]}"#
        );
        // No temp file is left behind.
        assert_eq!(fs::read_dir(&new).unwrap().count(), 1);
        // Once only: the state saved since is kept.
        fs::write(new.join(FILE_NAME), "{}").unwrap();
        assert!(!migrate_config(&old, &new).unwrap());
        assert_eq!(fs::read_to_string(new.join(FILE_NAME)).unwrap(), "{}");
        // Nor once that state was found broken and moved aside: it gives defaults.
        fs::rename(new.join(FILE_NAME), new.join(format!("{BAD_FILE_PREFIX}1"))).unwrap();
        assert!(!migrate_config(&old, &new).unwrap());
        assert!(!new.join(FILE_NAME).exists());
    }

    #[test]
    fn migrate_config_into_a_folder_with_hidden_files_only() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = old_and_new(root.path(), "old");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join(".DS_Store"), "").unwrap();
        assert!(migrate_config(&old, &new).unwrap());
        assert_eq!(fs::read_to_string(new.join(FILE_NAME)).unwrap(), "old");
    }

    #[test]
    fn migrate_config_never_overwrites_a_new_state() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = old_and_new(root.path(), "old");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join(FILE_NAME), "new").unwrap();
        assert!(!migrate_config(&old, &new).unwrap());
        assert_eq!(fs::read_to_string(new.join(FILE_NAME)).unwrap(), "new");
        assert_eq!(fs::read_to_string(old.join(FILE_NAME)).unwrap(), "old");
    }

    #[test]
    fn migrate_config_without_an_old_state_does_nothing() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(OLD_CONFIG_DIR_NAME);
        let new = root.path().join("com.shakirul.gg");
        assert!(!migrate_config(&old, &new).unwrap());
        assert!(!new.exists());
        // An old folder without a state file is no different.
        fs::create_dir_all(&old).unwrap();
        assert!(!migrate_config(&old, &new).unwrap());
        assert!(!new.exists());
    }

    #[test]
    fn migrate_config_that_cannot_write_is_io() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = old_and_new(root.path(), "old");
        // The new folder's path is taken by a file.
        fs::write(&new, "").unwrap();
        assert_eq!(migrate_config(&old, &new).unwrap_err().code, ErrorCode::Io);
    }

    #[test]
    fn the_old_config_folder_is_next_to_the_new_one() {
        let new = Path::new("/config/home/com.shakirul.gg");
        assert_eq!(
            old_config_dir(new),
            Some(Path::new("/config/home").join(OLD_CONFIG_DIR_NAME))
        );
        assert_eq!(old_config_dir(Path::new("/")), None);
    }

    #[test]
    fn config_dir_migrates_into_the_platform_folder() {
        let root = tempfile::tempdir().unwrap();
        let (_, new) = old_and_new(root.path(), "old");
        let dir = config_dir_with(None, || Ok(new.clone())).unwrap();
        assert_eq!(dir, new);
        assert_eq!(fs::read_to_string(new.join(FILE_NAME)).unwrap(), "old");
    }

    #[test]
    fn config_dir_skips_the_migration_with_gg_config_dir() {
        let root = tempfile::tempdir().unwrap();
        let (_, new) = old_and_new(root.path(), "old");
        let override_dir = root.path().join("override");
        let dir = config_dir_with(Some(override_dir.clone().into_os_string()), || {
            Ok(new.clone())
        })
        .unwrap();
        assert_eq!(dir, override_dir);
        assert!(!new.exists());
        assert!(!override_dir.exists());
    }

    #[test]
    fn config_dir_without_a_platform_folder_fails() {
        let err = config_dir_with(None, || Err(CoreError::new(ErrorCode::Io, "none"))).unwrap_err();
        assert_eq!(err.code, ErrorCode::Io);
    }
}
