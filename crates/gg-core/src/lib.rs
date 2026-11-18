//! gg core: everything that talks to git and shapes data for the UI.
//!
//! [`Core`] is the one entry point the transports (the Tauri app and the dev server) use:
//! [`Core::dispatch`] takes a command name and JSON arguments and returns JSON.

pub mod cli;
mod dispatch;
pub mod dto;
pub mod error;
pub mod gh;
pub mod git;
pub mod graph;
pub mod hosts;
pub mod launch;
mod mask;
pub mod ops;
mod platform;
pub mod repo;
pub mod store;
#[cfg(any(test, feature = "testutil"))]
pub mod testutil;
pub mod watcher;

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

pub use error::{CoreError, ErrorCode, Result};

/// Whether this build carries the test helpers and test-only hooks (`testutil`: temp
/// repositories, operation delays, a body that panics on request). The app refuses to be
/// built for release with them.
pub const TEST_HOOKS: bool = cfg!(feature = "testutil");

/// Whether this build can be told where to find `gh` and `ssh` (`tool-path`: the devserver
/// and tests only; never the app).
pub const TOOL_PATH_HOOK: bool = cfg!(feature = "tool-path");

use git::runner::{CommandLog, GitRunner};
use repo::RepoManager;
use store::Store;

/// Receives events for the UI (`repo-changed`, `repo-error`). Called from background tasks
/// and request handlers, never while repository state is locked. Must return quickly and
/// must not block on gg-core work: a repository's `repo-changed` events are emitted one at a
/// time, in order.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, name: &str, payload: serde_json::Value);
}

/// The app's core: open repositories, the persistent store, the git command log and perf
/// marks, behind [`Core::dispatch`]. One per app.
pub struct Core {
    /// The repository manager, or why there is none (git was not found).
    repos: std::result::Result<Arc<RepoManager>, CoreError>,
    log: Arc<CommandLog>,
    store: Arc<Store>,
    launch_path: Option<String>,
    perf: Mutex<Perf>,
    /// Store and perf-log writes take turns here, first come first served, so they land in
    /// the order they were asked for.
    io_turn: tokio::sync::Mutex<()>,
    /// Folders the operating system asked the app to open, not yet taken by the UI.
    opened: Mutex<Vec<String>>,
    /// The `gg` terminal command: where it is looked for, and what the shell said of it.
    cli: Mutex<cli::CliContext>,
    sink: Arc<dyn EventSink>,
}

/// Most perf marks kept in memory; older ones go first (the perf log keeps them all).
const MAX_PERF_MARKS: usize = 1_000;

/// Perf marks reported by the UI.
#[derive(Default)]
struct Perf {
    /// File each mark is appended to, if any.
    log: Option<PathBuf>,
    marks: Vec<(String, f64)>,
    /// The login shell's start and end are in the log already.
    login_logged: bool,
}

impl Core {
    /// Finds git, loads the store from `<config_dir>/state.json` and applies the stored
    /// fetch interval. `launch_path` is reported to the UI as `launchPath`. Must be called
    /// inside a Tokio runtime.
    ///
    /// Succeeds even when git is not found, so the UI can say so: the store commands work,
    /// and every repository command fails with `git_not_found`. The `Result` leaves room for
    /// start-up failures; there are none at present.
    pub async fn new(
        config_dir: PathBuf,
        launch_path: Option<PathBuf>,
        sink: Arc<dyn EventSink>,
    ) -> Result<Arc<Self>> {
        let log = CommandLog::new();
        let runner = GitRunner::detect(log.clone()).await;
        Ok(Self::assemble(config_dir, launch_path, sink, log, runner))
    }

    fn assemble(
        config_dir: PathBuf,
        launch_path: Option<PathBuf>,
        sink: Arc<dyn EventSink>,
        log: Arc<CommandLog>,
        runner: Result<GitRunner>,
    ) -> Arc<Self> {
        let store = Arc::new(Store::load(&config_dir));
        // Every event leaves the core with credentials in URLs masked.
        let sink: Arc<dyn EventSink> = Arc::new(mask::MaskingSink(sink));
        let repos = runner.map(|runner| {
            let repos = RepoManager::new(runner, sink.clone());
            repos.set_fetch_interval(store.settings().fetch_interval_secs);
            repos.set_worktree_memory(Arc::new(StoreMemory(store.clone())));
            repos
        });
        let login_path = repos.as_ref().ok().and_then(|r| r.login_path());
        Arc::new(Self {
            repos,
            log,
            store,
            launch_path: launch_path.map(|p| p.to_string_lossy().into_owned()),
            perf: Mutex::new(Perf::default()),
            io_turn: tokio::sync::Mutex::new(()),
            opened: Mutex::new(Vec::new()),
            cli: Mutex::new(cli::CliContext::new(login_path)),
            sink,
        })
    }

    /// Queues folders the operating system asked the app to open (Finder's Open With, a
    /// folder dropped on the Dock icon, `open -a`), starts opening them in the background
    /// like the launch path, and emits `paths-opened` so the UI takes them
    /// (`take_opened_paths`). Must be called inside a Tokio runtime.
    pub fn open_later(self: &Arc<Self>, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if let Ok(repos) = self.repos() {
            for path in &paths {
                let (repos, path) = (repos.clone(), path.clone());
                tokio::spawn(async move {
                    let _ = repos.open_in_background(&path).await;
                });
            }
        }
        lock_ignoring_poison(&self.opened)
            .extend(paths.iter().map(|p| p.to_string_lossy().into_owned()));
        self.sink.emit("paths-opened", serde_json::json!({}));
    }

    /// The queued folders (see [`Core::open_later`]), oldest first; the queue is emptied.
    fn take_opened_paths(&self) -> Vec<String> {
        std::mem::take(&mut *lock_ignoring_poison(&self.opened))
    }

    /// Starts opening, in the background, the repositories the UI opens at start: the saved
    /// session's tabs and the launch path. Their logs then load while the window and page
    /// are still starting, and the UI's own `open_repo` finds them open. Failures are left
    /// to that call, which fails the same way and reports it. Call once, after `new`, inside
    /// a Tokio runtime.
    pub fn open_startup_repos(self: &Arc<Self>) {
        let Ok(repos) = self.repos() else {
            return;
        };
        let session = self.store.session().tabs.into_iter();
        for path in session.chain(self.launch_path.clone()) {
            let repos = repos.clone();
            tokio::spawn(async move {
                let _ = repos.open_in_background(std::path::Path::new(&path)).await;
            });
        }
    }

    /// Sets the file `perf_mark` appends `name<TAB>ms<TAB>epoch_ms` lines to, where
    /// `epoch_ms` is the wall-clock time the mark was recorded (unix milliseconds), so marks
    /// can be compared with timestamps taken outside the app; `None` keeps marks in memory
    /// only.
    pub fn set_perf_log(&self, path: Option<PathBuf>) {
        self.perf().log = path;
    }

    /// From now on, launches (editor, terminal, file manager, browser) are appended to `file`
    /// instead of run (see [`RepoManager::record_launches_to`]); nothing without git.
    pub fn record_launches_to(&self, file: Option<PathBuf>) {
        if let Ok(repos) = self.repos() {
            repos.record_launches_to(file);
        }
    }

    /// Finds `gh` and `ssh` in the first folder of `path` and runs them with it instead of
    /// the login `PATH`: for tests and the devserver only (a fake `gh` and `ssh`; the
    /// `tool-path` feature, never in the app).
    #[cfg(feature = "tool-path")]
    pub fn set_tool_path(&self, path: Option<std::ffi::OsString>) {
        if let Ok(repos) = self.repos() {
            repos.set_tool_path(path);
        }
    }

    /// Every perf mark so far, oldest first.
    pub fn perf_marks(&self) -> Vec<(String, f64)> {
        self.perf().marks.clone()
    }

    /// Records a perf mark and appends it to the perf log, if there is one.
    fn perf_mark(&self, name: &str, ms: f64) -> Result<()> {
        // One line per mark, whatever the name holds.
        let name: String = name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let mut perf = self.perf();
        if perf.marks.len() >= MAX_PERF_MARKS {
            let excess = perf.marks.len() + 1 - MAX_PERF_MARKS;
            perf.marks.drain(..excess);
        }
        perf.marks.push((name.clone(), ms));
        let Some(path) = perf.log.clone() else {
            return Ok(());
        };
        let mut lines = format!("{name}\t{ms}\t{}\n", unix_ms());
        // Once known: when the login shell for git's PATH ran, by the wall clock (no time
        // since navigation: -1), so a launch's marks can be set against it.
        if !perf.login_logged
            && let Some((start, end)) = crate::git::runner::login_path_times()
        {
            perf.login_logged = true;
            lines.push_str(&format!(
                "login-shell-start\t-1\t{start:.0}\nlogin-shell-done\t-1\t{end:.0}\n"
            ));
        }
        for (native, at) in lock_ignoring_poison(&NATIVE_MARKS).drain(..) {
            lines.push_str(&format!("{native}\t-1\t{at}\n"));
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(lines.as_bytes()))
            .map_err(|e| {
                CoreError::new(
                    ErrorCode::Io,
                    format!("Could not write the perf log {}", path.display()),
                )
                .with_detail(e.to_string())
            })
    }

    /// The repository manager (tests only: what it recorded, such as a request's turn wait).
    #[cfg(any(test, feature = "testutil"))]
    pub fn manager(&self) -> Result<&Arc<RepoManager>> {
        self.repos()
    }

    /// The repository manager, or `git_not_found`.
    fn repos(&self) -> Result<&Arc<RepoManager>> {
        self.repos.as_ref().map_err(Clone::clone)
    }

    fn perf(&self) -> MutexGuard<'_, Perf> {
        lock_ignoring_poison(&self.perf)
    }
}

/// Each repository's active worktree, kept in the store's repository preferences.
struct StoreMemory(Arc<Store>);

impl repo::WorktreeMemory for StoreMemory {
    fn remembered(&self, root: &std::path::Path) -> Option<PathBuf> {
        self.0.active_worktree(root)
    }

    fn remember(&self, root: &std::path::Path, worktree: Option<&std::path::Path>) {
        // Only the next start would miss it.
        let _ = self.0.set_active_worktree(root, worktree);
    }

    fn pr_base(&self, root: &std::path::Path) -> Option<String> {
        self.0.repo_prefs(root).pr_base
    }

    fn remember_pr_base(&self, root: &std::path::Path, base: &str) {
        // Only the next pull request's suggestion would miss it.
        let _ = self.0.set_repo_prefs(
            root,
            store::RepoPrefsPatch {
                pr_base: Some(Some(base.to_owned())),
                ..Default::default()
            },
        );
    }
}

pub(crate) fn lock_ignoring_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Milliseconds since the unix epoch (0 if the clock is before it).
/// Start-up marks of the native side (wall clock), waiting for the perf log (performance runs).
static NATIVE_MARKS: Mutex<Vec<(&'static str, u128)>> = Mutex::new(Vec::new());

/// Records native start-up mark `name` at this moment (wall clock): written to the perf log
/// with the page's next mark, as `name<TAB>-1<TAB>unix ms`. For performance runs (the perf
/// build calls it); a few entries otherwise, never written.
pub fn native_mark(name: &'static str) {
    let mut marks = lock_ignoring_poison(&NATIVE_MARKS);
    if marks.len() < 64 {
        marks.push((name, unix_ms()));
    }
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without git the core still starts: store commands work, repository commands say
    /// `git_not_found`.
    #[tokio::test]
    async fn without_git_store_commands_work() {
        use serde_json::json;

        let config = tempfile::tempdir().unwrap();
        let missing = CoreError::new(ErrorCode::GitNotFound, "git was not found on PATH");
        let core = Core::assemble(
            config.path().to_path_buf(),
            None,
            Arc::new(testutil::RecordingSink::default()),
            CommandLog::new(),
            Err(missing),
        );
        let state = core.dispatch("get_app_state", json!({})).await.unwrap();
        assert_eq!(state["settings"]["fetchIntervalSecs"], 300);
        let settings = json!({"patch": {"fetchIntervalSecs": 0}});
        core.dispatch("update_settings", settings).await.unwrap();
        core.dispatch("save_session", json!({"tabs": [], "active": null}))
            .await
            .unwrap();
        core.dispatch("set_window_focused", json!({"focused": true}))
            .await
            .unwrap();
        assert_eq!(
            core.dispatch("command_log", json!({"sinceId": 0}))
                .await
                .unwrap(),
            json!([])
        );
        for (cmd, args) in [
            ("open_repo", json!({"path": config.path()})),
            ("get_snapshot", json!({"repoId": 1})),
            ("fetch", json!({"repoId": 1})),
            ("refresh", json!({"repoId": 1})),
        ] {
            let err = core.dispatch(cmd, args).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::GitNotFound, "{cmd}");
        }
    }
}
