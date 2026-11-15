//! `GitRunner` is the only place the app spawns `git`. Every command gets the same
//! non-interactive environment and leading `-c` options, and lands in the `CommandLog`.
//! `ToolRunner` runs another program the app needs (`gh`) the same way: no terminal, its own
//! process group, a time limit, the operation's Stop, the log, masking; and stdin null or
//! bytes it is given, never the app's.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::process::{Child, Command};

use crate::error::{CoreError, ErrorCode, Result};
use crate::mask::mask_userinfo;

/// Number of reads, and separately of writes, the log keeps; older ones are dropped.
const LOG_CAPACITY: usize = 500;
/// Longest stdout/stderr text kept per log record, in bytes.
const MAX_LOG_TEXT: usize = 16 * 1024;
/// How long the login shell gets to report its `PATH`.
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the login shell's output may take to arrive once the shell has exited.
#[cfg(unix)]
const READ_GRACE: Duration = Duration::from_millis(500);

/// Environment for every git child: never prompt, never open an editor (for messages, rebase
/// todo lists or merge messages) or pager, never take optional locks, and keep messages in a
/// stable (C) locale.
const GIT_ENV: [(&str, &str); 7] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_EDITOR", "true"),
    ("GIT_SEQUENCE_EDITOR", "true"),
    ("GIT_MERGE_AUTOEDIT", "no"),
    ("GIT_PAGER", "cat"),
    ("LC_ALL", "C"),
];

/// Options placed before the subcommand of every git invocation. `log.showSignature=false`
/// keeps GPG output out of machine-readable log output whatever the user's config says.
/// `core.fsmonitor=false` keeps git from running a hook program or starting a daemon that a
/// repository (or one of its submodules) configures; git passes `-c` options on to the git
/// processes it starts for submodules.
const LEADING_ARGS: [&str; 8] = [
    "-c",
    "core.quotePath=false",
    "-c",
    "color.ui=false",
    "-c",
    "log.showSignature=false",
    "-c",
    "core.fsmonitor=false",
];

/// Where `core.hooksPath` points for the git another program runs: nowhere.
const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// The environment that gives the git another program runs in a repository (`gh` runs git
/// itself) the protection the app's own git children have, through what git honours in
/// every child it is started with: [`GIT_ENV`], and the settings of [`LEADING_ARGS`] as
/// command-line config, plus `core.hooksPath` set to the null device (such a program never
/// needs the repository's hooks). The settings go into `GIT_CONFIG_PARAMETERS` (what `-c`
/// passes on), after whatever the app inherited there: git reads it after the
/// `GIT_CONFIG_COUNT` settings and takes the last of a key, so neither can override them,
/// and a user's own `GIT_CONFIG_COUNT` settings of other keys still apply. Built from the
/// same constants as git's own, so the two cannot drift; the variables of an enclosing git
/// process are removed by [`child_command`] for every child.
pub(crate) fn git_protection_env() -> Vec<(String, String)> {
    protection_env_after(
        std::env::var("GIT_CONFIG_PARAMETERS")
            .ok()
            .as_deref()
            .filter(|v| !v.trim().is_empty()),
    )
}

/// [`git_protection_env`] with `inherited` as the `GIT_CONFIG_PARAMETERS` inherited.
fn protection_env_after(inherited: Option<&str>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = GIT_ENV
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let mut settings: Vec<String> = LEADING_ARGS
        .chunks(2)
        .filter_map(|pair| match pair {
            ["-c", setting] => Some((*setting).to_owned()),
            _ => None,
        })
        .collect();
    settings.push(format!("core.hooksPath={NULL_DEVICE}"));
    let mut params: Vec<String> = inherited.map(str::to_owned).into_iter().collect();
    params.extend(settings.iter().map(|s| config_quoted(s)));
    env.push(("GIT_CONFIG_PARAMETERS".to_owned(), params.join(" ")));
    env
}

/// `text` quoted for `GIT_CONFIG_PARAMETERS`: in single quotes, a quote in it as `'\''`.
fn config_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Variables an enclosing git process (for example a hook) exports. Inherited, they would
/// point our commands at that process's repository instead of `cwd`.
const INHERITED_REPO_VARS: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

/// Brackets the `PATH` the login shell prints, so profile output around it cannot be taken
/// for part of it.
const PATH_MARKER: &[u8] = b"__GG_PATH__";

/// Whether a command only reads the repository or may change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CmdKind {
    Read,
    Write,
}

/// When [`GitRunner::run_with_timeout`] gives up on a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeLimit {
    /// After this long, whatever it is doing.
    pub total: Duration,
    /// Also once it has written nothing (stdout or stderr) for this long, if set.
    pub idle: Option<Duration>,
}

impl TimeLimit {
    /// Gives up after `total`.
    pub fn total(total: Duration) -> Self {
        Self { total, idle: None }
    }

    /// Gives up after `idle` without output, or after `total` in any case.
    pub fn idle(idle: Duration, total: Duration) -> Self {
        Self {
            total,
            idle: Some(idle),
        }
    }
}

/// Which limit of a [`TimeLimit`] a command ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expired {
    /// It ran for the whole `total`.
    Total(Duration),
    /// It wrote nothing for `idle`.
    Idle(Duration),
    /// It was cancelled (see [`GitRunner::run_cancellable`]).
    Cancelled,
}

/// A command [`GitRunner::run_with_timeout`] gave up on: the limit it ran into and the tail
/// of what it had written by then.
#[derive(Debug, Clone)]
pub struct TimedOut {
    pub limit: Expired,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// How long a cancelled command has to stop before it is killed.
pub const CANCEL_GRACE: Duration = Duration::from_secs(1);
/// How long a killed command's output may take to end; past it, it is left (something it
/// started that left its process group holds it open).
const KILLED_GRACE: Duration = Duration::from_secs(2);

/// The oldest git the app works with: `--no-auto-maintenance` (scheduled fetches) came in
/// 2.29; `--end-of-options` in 2.24.
pub const MIN_GIT: (u32, u32) = (2, 29);

/// `(major, minor, patch)` of `git --version`'s output ("git version 2.54.0 (Apple
/// Git-157)", "git version 2.25.1.windows.1"), if it has one.
pub fn parse_git_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().nth(2)?;
    let mut parts = word.split('.').map(|p| {
        p.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

/// Why `version` is too old for the app (M3), if it is.
pub fn version_problem(version: (u32, u32, u32)) -> Option<String> {
    ((version.0, version.1) < MIN_GIT).then(|| {
        format!(
            "Git {}.{} or newer is needed; found {}.{}.{}. Some actions and the background \
             fetch fail with it: install a newer git, then restart the app.",
            MIN_GIT.0, MIN_GIT.1, version.0, version.1, version.2
        )
    })
}

/// Said for a command (and an operation) the user stopped.
pub const STOPPED: &str = "Stopped.";

/// The user's Stop for one operation's git commands (R41): once stopped, the command running
/// is ended with everything it started (its process group: SIGTERM, then SIGKILL after
/// [`CANCEL_GRACE`]), and no further command starts. Cheap to share.
#[derive(Default)]
pub struct Stop {
    stopped: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Stop {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Stops the operation's commands; harmless if it was stopped before.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Resolves once stopped (at once if it was before).
    async fn stopped(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_stopped() {
            return;
        }
        notified.await;
    }
}

/// The error of a command that was stopped, or not started because its operation was.
fn stopped_error(detail: &str) -> CoreError {
    let err = CoreError::new(ErrorCode::Io, STOPPED);
    if detail.trim().is_empty() {
        err
    } else {
        err.with_detail(detail)
    }
}

/// Most bytes [`GitRunner::run_with_timeout`] keeps of stdout and of stderr: the last ones.
pub const KEPT_TAIL: usize = 64 * 1024;
/// Put before output of which only the tail was kept.
const DROPPED_MARK: &str = "\u{2026} (earlier output not kept)\n";

/// Result of a finished git command.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// Exit code; -1 when git was killed by a signal.
    pub code: i32,
}

impl GitOutput {
    /// Stdout decoded lossily as UTF-8.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// One entry of the command log. `args` excludes the injected leading `-c` options.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRecord {
    pub id: u64,
    /// The program run: `git`, or another tool (`gh`).
    pub program: String,
    pub kind: CmdKind,
    pub cwd: String,
    pub args: Vec<String>,
    pub started_at_ms: i64,
    pub duration_ms: u32,
    pub exit_code: i32,
    /// Kept only for `Write` commands, truncated to 16 KiB.
    pub stdout: String,
    /// Always kept, truncated to 16 KiB.
    pub stderr: String,
    /// The open repository it ran for, if any (not while a repository is being opened).
    pub repo_id: Option<u32>,
    /// The operation it was part of, if any.
    pub op_id: Option<u64>,
    /// That operation's title ("Merge feature/x into main"), once it has one. Filled when
    /// records are served.
    pub op_title: Option<String>,
}

/// The last 500 reads and, separately, the last 500 writes, so that a write stays findable
/// however many reads come after it. Ids start at 1 and increase by one across both.
pub struct CommandLog {
    inner: Mutex<LogInner>,
}

struct LogInner {
    next_id: u64,
    reads: VecDeque<CommandRecord>,
    writes: VecDeque<CommandRecord>,
    /// Operations' titles by id, the newest [`2 * LOG_CAPACITY`](LOG_CAPACITY) of them.
    op_titles: BTreeMap<u64, String>,
}

/// Which records [`CommandLog::query`] returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Only records with a greater id.
    pub since_id: u64,
    /// Only records of this repository.
    pub repo_id: Option<u32>,
    /// Only writes.
    pub writes_only: bool,
}

impl CommandLog {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(LogInner {
                next_id: 1,
                reads: VecDeque::with_capacity(LOG_CAPACITY),
                writes: VecDeque::with_capacity(LOG_CAPACITY),
                op_titles: BTreeMap::new(),
            }),
        })
    }

    /// Records with an id greater than `id`, oldest first. `since(0)` returns everything kept.
    pub fn since(&self, id: u64) -> Vec<CommandRecord> {
        self.query(LogFilter {
            since_id: id,
            ..LogFilter::default()
        })
    }

    /// Gives operation `op_id` its title, which its records are served with.
    pub fn name_op(&self, op_id: u64, title: &str) {
        let mut inner = self.lock();
        inner.op_titles.insert(op_id, title.to_owned());
        while inner.op_titles.len() > 2 * LOG_CAPACITY {
            inner.op_titles.pop_first();
        }
    }

    /// The records `filter` selects, oldest first, each with its operation's title.
    pub fn query(&self, filter: LogFilter) -> Vec<CommandRecord> {
        let inner = self.lock();
        let after = |ring: &VecDeque<CommandRecord>| {
            let start = ring.partition_point(|r| r.id <= filter.since_id);
            ring.range(start..)
                .filter(|r| filter.repo_id.is_none_or(|id| r.repo_id == Some(id)))
                .map(|r| CommandRecord {
                    op_title: r.op_id.and_then(|id| inner.op_titles.get(&id).cloned()),
                    ..r.clone()
                })
                .collect::<Vec<_>>()
        };
        let writes = after(&inner.writes);
        if filter.writes_only {
            return writes;
        }
        let reads = after(&inner.reads);
        // Merged by id: each ring is in id order.
        let mut merged = Vec::with_capacity(reads.len() + writes.len());
        let (mut r, mut w) = (reads.into_iter().peekable(), writes.into_iter().peekable());
        loop {
            let next = match (r.peek(), w.peek()) {
                (Some(a), Some(b)) if a.id < b.id => r.next(),
                (Some(_), Some(_)) | (None, Some(_)) => w.next(),
                (Some(_), None) => r.next(),
                (None, None) => break,
            };
            merged.extend(next);
        }
        merged
    }

    /// Ids of the first and last write of operation `op_id` still kept, if any.
    pub fn op_writes(&self, op_id: u64) -> Option<(u64, u64)> {
        let inner = self.lock();
        let mut ids = inner
            .writes
            .iter()
            .filter(|r| r.op_id == Some(op_id))
            .map(|r| r.id);
        let first = ids.next()?;
        Some((first, ids.next_back().unwrap_or(first)))
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &self,
        tags: Tags,
        kind: CmdKind,
        cwd: &Path,
        args: &[&str],
        started: Instant,
        exit_code: i32,
        stdout: &[u8],
        stderr: &str,
    ) {
        let elapsed = started.elapsed();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let duration_ms = u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX);
        // A credential in a remote URL never reaches the log.
        let stdout = match kind {
            CmdKind::Write => {
                truncate_text(mask_userinfo(&String::from_utf8_lossy(stdout)).into_owned())
            }
            CmdKind::Read => String::new(),
        };
        let mut inner = self.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        let ring = match kind {
            CmdKind::Read => &mut inner.reads,
            CmdKind::Write => &mut inner.writes,
        };
        if ring.len() == LOG_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(CommandRecord {
            id,
            program: tags.program.to_owned(),
            kind,
            cwd: cwd.to_string_lossy().into_owned(),
            args: args.iter().map(|a| mask_userinfo(a).into_owned()).collect(),
            started_at_ms: now_ms - i64::from(duration_ms),
            duration_ms,
            exit_code,
            stdout,
            stderr: truncate_text(mask_userinfo(stderr).into_owned()),
            repo_id: tags.repo_id,
            op_id: tags.op_id,
            op_title: None,
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LogInner> {
        // A panic while holding the lock cannot leave the buffer inconsistent; keep using it.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn lock_tail(t: &Mutex<Tail>) -> std::sync::MutexGuard<'_, Tail> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// The last `keep` bytes ([`KEPT_TAIL`] by default) written to it, starting at a line when
/// it had to drop some.
struct Tail {
    bytes: Vec<u8>,
    dropped: bool,
    keep: usize,
}

impl Default for Tail {
    fn default() -> Self {
        Self::keeping(KEPT_TAIL)
    }
}

impl Tail {
    fn keeping(keep: usize) -> Self {
        Self {
            bytes: Vec::new(),
            dropped: false,
            keep,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        // Trimmed only once it holds twice the tail, so each byte is moved at most once.
        if self.bytes.len() > 2 * self.keep {
            self.trim();
        }
    }

    fn trim(&mut self) {
        if self.bytes.len() <= self.keep {
            return;
        }
        let mut cut = self.bytes.len() - self.keep;
        // Start at the next line, if one starts early in the tail.
        if let Some(nl) = self.bytes[cut..].iter().position(|&b| b == b'\n')
            && nl < self.keep / 2
        {
            cut += nl + 1;
        }
        self.bytes.drain(..cut);
        self.dropped = true;
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.trim();
        if self.dropped {
            let mut marked = DROPPED_MARK.as_bytes().to_vec();
            marked.append(&mut self.bytes);
            return marked;
        }
        self.bytes
    }
}

/// Cuts `s` to at most `MAX_LOG_TEXT` bytes on a char boundary.
fn truncate_text(mut s: String) -> String {
    if s.len() > MAX_LOG_TEXT {
        let mut end = MAX_LOG_TEXT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// What a runner's log records are tagged with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tags {
    /// The program's name, as the log shows it.
    program: &'static str,
    repo_id: Option<u32>,
    op_id: Option<u64>,
}

impl Default for Tags {
    fn default() -> Self {
        Self {
            program: "git",
            repo_id: None,
            op_id: None,
        }
    }
}

/// What a command reads on its standard input: nothing (null), or these bytes, written and
/// then closed. Never the app's own stdin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum StdinPolicy {
    #[default]
    Null,
    Bytes(Vec<u8>),
}

/// Runs git commands. Cheap to clone; clones share the command log.
#[derive(Clone)]
pub struct GitRunner {
    log: Arc<CommandLog>,
    /// `PATH` for git children, from the user's login shell when it could be read.
    path: Option<OsString>,
    tags: Tags,
    /// The Stop of the operation this runner runs for, if any (see [`Stop`]).
    stop: Option<Arc<Stop>>,
    /// The git version found at start, when it could be read.
    version: Option<(u32, u32, u32)>,
    /// The index file git uses instead of the worktree's own (`GIT_INDEX_FILE`), if any: a
    /// commit of chosen files builds its tree in a temporary index (see
    /// [`GitRunner::with_index_file`]).
    index_file: Option<PathBuf>,
}

impl GitRunner {
    /// Finds git and checks that it runs: with the login shell's `PATH` when it can be read
    /// and git runs with it, else with the inherited `PATH`. Fails with `GitNotFound` if
    /// neither works. The check itself is not logged.
    pub async fn detect(log: Arc<CommandLog>) -> Result<Self> {
        Self::detect_from(log, || LOGIN_PATH.get(resolve_login_path)).await
    }

    /// [`GitRunner::detect`] with the login shell's `PATH` from `login_path`, which may
    /// block (it runs on a blocking thread).
    async fn detect_from(
        log: Arc<CommandLog>,
        login_path: impl FnOnce() -> Option<OsString> + Send + 'static,
    ) -> Result<Self> {
        let path = tokio::task::spawn_blocking(login_path)
            .await
            .unwrap_or(None);
        Self::detect_with_login_path(log, path).await
    }

    /// [`GitRunner::detect`] with `login_path` as the login shell's `PATH`.
    async fn detect_with_login_path(
        log: Arc<CommandLog>,
        login_path: Option<OsString>,
    ) -> Result<Self> {
        if let Some(path) = login_path {
            let mut runner = Self {
                log: log.clone(),
                path: Some(path),
                tags: Tags::default(),
                stop: None,
                version: None,
                index_file: None,
            };
            if let Ok(version) = runner.check().await {
                runner.version = version;
                return Ok(runner);
            }
        }
        let mut runner = Self {
            log,
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        runner.version = runner.check().await?;
        Ok(runner)
    }

    /// This runner, logging its commands as run for repository `repo_id`.
    pub fn for_repo(&self, repo_id: u32) -> Self {
        let mut runner = self.clone();
        runner.tags.repo_id = Some(repo_id);
        runner
    }

    /// This runner, logging its commands as part of operation `op_id`, whose commands
    /// `stop` ends (see [`Stop`]).
    pub(crate) fn for_op(&self, op_id: u64, stop: Arc<Stop>) -> Self {
        let mut runner = self.clone();
        runner.tags.op_id = Some(op_id);
        runner.stop = Some(stop);
        runner
    }

    /// This runner without its operation's Stop: for what must still run once git has done
    /// something that cannot be taken back (the user's index following a commit made).
    pub(crate) fn unstoppable(&self) -> Self {
        let mut runner = self.clone();
        runner.stop = None;
        runner
    }

    /// Whether the operation this runner runs for was stopped.
    pub(crate) fn is_stopped(&self) -> bool {
        self.stop.as_ref().is_some_and(|s| s.is_stopped())
    }

    /// This runner, with git using the index file `index` (an absolute path) instead of the
    /// worktree's own: `GIT_INDEX_FILE` for git and the hooks it runs.
    pub(crate) fn with_index_file(&self, index: &Path) -> Self {
        let mut runner = self.clone();
        runner.index_file = Some(index.to_path_buf());
        runner
    }

    /// Why the git found is too old for the app (M3), if it is; checked once, at start.
    pub fn version_problem(&self) -> Option<String> {
        self.version.and_then(version_problem)
    }

    /// Runs `git --version`; `GitNotFound` unless it succeeds. Its version, if readable.
    async fn check(&self) -> Result<Option<(u32, u32, u32)>> {
        let mut cmd = self.command(None, &["--version"]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match cmd.output().await {
            Ok(out) if out.status.success() => {
                Ok(parse_git_version(&String::from_utf8_lossy(&out.stdout)))
            }
            Ok(out) => Err(
                CoreError::new(ErrorCode::GitNotFound, "git --version failed")
                    .with_detail(String::from_utf8_lossy(&out.stderr)),
            ),
            Err(e) => Err(
                CoreError::new(ErrorCode::GitNotFound, "git was not found on PATH")
                    .with_detail(e.to_string()),
            ),
        }
    }

    /// Runs git in `cwd`. A nonzero exit becomes `GitFailed` with the first stderr line as
    /// `message` and the full stderr as `detail`.
    pub async fn run(&self, cwd: &Path, args: &[&str], kind: CmdKind) -> Result<GitOutput> {
        let out = self.run_unchecked(cwd, args, kind).await?;
        if out.code == 0 {
            return Ok(out);
        }
        Err(git_failed(args, out.code, &out.stderr))
    }

    /// Runs git in `cwd` and returns its output whatever the exit code. Errs only when git
    /// could not be started, or when the operation it runs for was stopped (see [`Stop`]).
    pub async fn run_unchecked(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
    ) -> Result<GitOutput> {
        if let Some(stop) = &self.stop {
            return self.run_stoppable(cwd, args, kind, stop).await;
        }
        let started = Instant::now();
        let mut cmd = self.command(Some(cwd), args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match cmd.output().await {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                self.log.push(
                    self.tags,
                    kind,
                    cwd,
                    args,
                    started,
                    code,
                    &out.stdout,
                    &stderr,
                );
                Ok(GitOutput {
                    stdout: out.stdout,
                    stderr,
                    code,
                })
            }
            Err(e) => {
                let err = spawn_error(cwd, e);
                self.log
                    .push(self.tags, kind, cwd, args, started, -1, b"", &err.message);
                Err(err)
            }
        }
    }

    /// [`GitRunner::run_unchecked`] for an operation's command, which `stop` can end: it is
    /// not started once stopped; stopped while it runs, its process group (git and what it
    /// started: a hook, a signing program) is asked to end, then killed. Both err with
    /// [`STOPPED`] (the output so far as detail), and the command is logged with exit -1.
    async fn run_stoppable(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        stop: &Stop,
    ) -> Result<GitOutput> {
        if stop.is_stopped() {
            return Err(stopped_error(""));
        }
        let started = Instant::now();
        let mut cmd = self.command(Some(cwd), args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = cmd.spawn().map_err(|e| {
            let err = spawn_error(cwd, e);
            self.log
                .push(self.tags, kind, cwd, args, started, -1, b"", &err.message);
            err
        })?;
        let group = child.id();
        let output = child.wait_with_output();
        tokio::pin!(output);
        let finished = tokio::select! {
            out = &mut output => Some(out),
            () = stop.stopped() => None,
        };
        let out = match finished {
            Some(out) => out,
            None => {
                let (stdout, stderr) = match end_group(group, &mut output).await {
                    Some(Ok(out)) => (
                        out.stdout,
                        String::from_utf8_lossy(&out.stderr).into_owned(),
                    ),
                    _ => (Vec::new(), String::new()),
                };
                let note = if stderr.is_empty() {
                    STOPPED.to_owned()
                } else {
                    format!("{}\n{STOPPED}", stderr.trim_end())
                };
                self.log
                    .push(self.tags, kind, cwd, args, started, -1, &stdout, &note);
                return Err(stopped_error(&stderr));
            }
        };
        match out {
            Ok(out) => {
                let code = out.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                self.log.push(
                    self.tags,
                    kind,
                    cwd,
                    args,
                    started,
                    code,
                    &out.stdout,
                    &stderr,
                );
                Ok(GitOutput {
                    stdout: out.stdout,
                    stderr,
                    code,
                })
            }
            Err(e) => {
                let err = spawn_error(cwd, e);
                self.log
                    .push(self.tags, kind, cwd, args, started, -1, b"", &err.message);
                Err(err)
            }
        }
    }

    /// Like [`GitRunner::run_unchecked`], for long-running commands whose output is shown
    /// rather than parsed (fetch): only the last [`KEPT_TAIL`] bytes of stdout and of stderr
    /// are kept (the pipes are still read to the end), and it gives up when `limit` is
    /// reached. Then git and everything it started (a remote helper, ssh) are killed, the
    /// command is logged as timed out, and the result is `Ok(Err(limit hit))`. On Unix git
    /// runs in a session (and so a process group) of its own, without a controlling
    /// terminal, as every git child does; elsewhere only git itself is killed. An operation's
    /// Stop cancels it too (see [`Stop`]).
    ///
    /// The group's id is git's pid, taken at spawn. On timeout the group is killed even when
    /// git has already exited (a helper it started still holding its output open): a group
    /// id is not reused while the group has members. Then git is waited for. Without a
    /// controlling terminal, a helper that wants to ask on the terminal (an ssh passphrase
    /// or host-key prompt; git's own prompts are off) cannot open it and fails at once,
    /// whether or not the app was started from a terminal. Askpass helpers still work.
    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        limit: TimeLimit,
    ) -> Result<std::result::Result<GitOutput, TimedOut>> {
        self.run_with_progress(cwd, args, kind, limit, &|_| {})
            .await
    }

    /// [`GitRunner::run_with_timeout`] that also hands each chunk of stderr to `on_stderr`
    /// as it arrives (git writes its progress there).
    pub async fn run_with_progress(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        limit: TimeLimit,
        on_stderr: &(dyn Fn(&[u8]) + Send + Sync),
    ) -> Result<std::result::Result<GitOutput, TimedOut>> {
        self.run_limited(cwd, args, kind, limit, on_stderr, None)
            .await
    }

    /// [`GitRunner::run_with_timeout`] that also stops when `cancel` is notified: then git
    /// and what it started are asked to stop (SIGTERM on Unix, so git removes its lock
    /// files), killed if still there after [`CANCEL_GRACE`], and the result is
    /// `Ok(Err(..))` with [`Expired::Cancelled`].
    pub async fn run_cancellable(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        limit: TimeLimit,
        cancel: &tokio::sync::Notify,
    ) -> Result<std::result::Result<GitOutput, TimedOut>> {
        self.run_limited(cwd, args, kind, limit, &|_| {}, Some(cancel))
            .await
    }

    async fn run_limited(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        limit: TimeLimit,
        on_stderr: &(dyn Fn(&[u8]) + Send + Sync),
        cancel: Option<&tokio::sync::Notify>,
    ) -> Result<std::result::Result<GitOutput, TimedOut>> {
        let exec = Exec {
            log: &self.log,
            tags: self.tags,
            stop: self.stop.as_deref(),
        };
        let cmd = self.command(Some(cwd), args);
        let run = Run {
            cwd,
            args,
            kind,
            stdin: StdinPolicy::Null,
            limit,
            keep_stdout: Some(KEPT_TAIL),
        };
        exec_limited(exec, cmd, run, on_stderr, cancel).await
    }

    /// Starts a read-only git command with stdout and stderr piped, for output too large to
    /// buffer. Must be called inside a Tokio runtime. Not logged until the caller reports
    /// completion with [`GitRunner::log_stream_done`], unless it cannot start: that is
    /// logged at once.
    ///
    /// The caller must read stdout and drain stderr concurrently: git blocks once either pipe
    /// is full, so reading one to the end before the other can deadlock. If reading stdout
    /// fails, kill the child so the stderr drain can finish. For `git log`, use
    /// [`crate::git::log::read_log`], which does all of this.
    pub fn spawn_stream(&self, cwd: &Path, args: &[&str]) -> Result<Child> {
        let mut cmd = self.command(Some(cwd), args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let started = Instant::now();
        cmd.spawn().map_err(|e| {
            let err = spawn_error(cwd, e);
            self.log.push(
                self.tags,
                CmdKind::Read,
                cwd,
                args,
                started,
                -1,
                b"",
                &err.message,
            );
            err
        })
    }

    /// Records a `Read` command started with [`GitRunner::spawn_stream`] at `started`.
    pub fn log_stream_done(
        &self,
        cwd: &Path,
        args: &[&str],
        started: Instant,
        exit_code: i32,
        stderr: &str,
    ) {
        self.log.push(
            self.tags,
            CmdKind::Read,
            cwd,
            args,
            started,
            exit_code,
            b"",
            stderr,
        );
    }

    /// Records a command started at `started` with [`GitRunner::run_unchecked`] that was
    /// killed for taking too long (its future dropped), as exit code -1 with stderr
    /// "timed out".
    pub(crate) fn log_timed_out(&self, cwd: &Path, args: &[&str], kind: CmdKind, started: Instant) {
        self.log
            .push(self.tags, kind, cwd, args, started, -1, b"", "timed out");
    }

    pub fn log(&self) -> &Arc<CommandLog> {
        &self.log
    }

    /// The `PATH` git children get (the login shell's), if it was read.
    pub(crate) fn login_path(&self) -> Option<&OsString> {
        self.path.as_ref()
    }

    /// Every git child: the environment and leading options above, stdin null, and (Unix)
    /// a session of its own (R15, R41): no controlling terminal, so nothing git starts (a
    /// hook, ssh, a signing program) can open `/dev/tty` and wait there when the app was
    /// started from a terminal; it fails at once instead. The session leader (git) also leads
    /// a new process group whose id is its pid: what a time limit or a Stop ends.
    fn command(&self, cwd: Option<&Path>, args: &[&str]) -> Command {
        // With PATH overridden, the program lookup for "git" also uses this PATH.
        let mut cmd = child_command(std::ffi::OsStr::new("git"), self.path.as_ref(), cwd);
        cmd.args(LEADING_ARGS).args(args).envs(GIT_ENV);
        if let Some(index) = &self.index_file {
            cmd.env("GIT_INDEX_FILE", index);
        }
        // Test builds only (the app refuses `testutil` in a release build): the developer's
        // global and system config must not change what tests see.
        #[cfg(feature = "testutil")]
        cmd.env("GIT_CONFIG_GLOBAL", crate::testutil::empty_git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_SYSTEM");
        cmd
    }

    /// A runner of `program` (an absolute path, found by the caller), logged as `name`, with
    /// this runner's log, tags, Stop and `PATH`.
    pub(crate) fn tool(&self, program: PathBuf, name: &'static str) -> ToolRunner {
        ToolRunner {
            log: self.log.clone(),
            program,
            path: self.path.clone(),
            env: Vec::new(),
            removed: Vec::new(),
            tags: Tags {
                program: name,
                ..self.tags
            },
            stop: self.stop.clone(),
        }
    }
}

/// The spawn path every child of the app's shares (git and the other tools): `program` in
/// `cwd`, with `path` as its `PATH` when known (and found with it), stdin null, killed when
/// dropped, and (Unix) in a session of its own (R15, R41), so without a controlling terminal
/// and leading its own process group. Without the variables an enclosing git process (a
/// hook) exports, which would point it, or the git it runs, at another repository.
fn child_command(
    program: &std::ffi::OsStr,
    path: Option<&OsString>,
    cwd: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(unix)]
    {
        // SAFETY: the closure runs in the forked child before exec and only calls
        // setsid, which is async-signal-safe and touches no memory of the parent.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    for var in INHERITED_REPO_VARS {
        cmd.env_remove(var);
    }
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd
}

/// Most bytes of another program's stdout kept: its answer is parsed (`gh`'s JSON list of
/// up to 1000 pull requests), not only shown.
const TOOL_STDOUT: usize = 32 * 1024 * 1024;

/// Runs another program than git (`gh`) on the same spawn path as git (see
/// [`child_command`]): an absolute path, an argument array, never a shell; no terminal, its
/// own process group, a time limit, the operation's Stop; stdin null or the bytes given,
/// written and then closed. Logged (as its name) and masked like git. Cheap to clone.
#[derive(Clone)]
pub struct ToolRunner {
    log: Arc<CommandLog>,
    program: PathBuf,
    path: Option<OsString>,
    /// Set for every command, beside the inherited environment.
    env: Vec<(String, String)>,
    /// Removed from the inherited environment for every command.
    removed: Vec<&'static str>,
    tags: Tags,
    stop: Option<Arc<Stop>>,
}

impl ToolRunner {
    /// This runner with `PATH` `path` for its children instead.
    pub(crate) fn with_path(mut self, path: Option<OsString>) -> Self {
        self.path = path;
        self
    }

    /// This runner, also setting `var` to `value` for every command.
    pub(crate) fn with_env(mut self, var: &str, value: &str) -> Self {
        self.env.push((var.to_owned(), value.to_owned()));
        self
    }

    /// This runner, giving the git its program runs the protection of the app's own git
    /// children (see [`git_protection_env`]).
    pub(crate) fn with_git_protection(mut self) -> Self {
        self.env.extend(git_protection_env());
        self
    }

    /// This runner, removing `var` from the environment its commands inherit.
    pub(crate) fn without_env(mut self, var: &'static str) -> Self {
        self.removed.push(var);
        self
    }

    /// The program it runs.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Runs the program with `args` in `cwd`, reading `stdin`, giving up at `limit` (it and
    /// what it started are killed: `Ok(Err(..))`); an operation's Stop ends it too. Errs only
    /// when it cannot start, or was stopped before it could.
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        stdin: StdinPolicy,
        limit: TimeLimit,
    ) -> Result<std::result::Result<GitOutput, TimedOut>> {
        self.run_keeping(cwd, args, kind, stdin, limit, Some(TOOL_STDOUT))
            .await
    }

    /// Runs the program as [`ToolRunner::run`] does, with nothing on stdin and its output
    /// (stdout and stderr) discarded, never read: only its exit status is known, and the
    /// command log records the command without any output. For a command whose output must
    /// not reach the app (`gh auth token`).
    pub async fn run_status_only(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        limit: TimeLimit,
    ) -> Result<std::result::Result<GitOutput, TimedOut>> {
        self.run_keeping(cwd, args, kind, StdinPolicy::Null, limit, None)
            .await
    }

    /// `keep_stdout`: most bytes of stdout kept; `None`: stdout and stderr discarded.
    async fn run_keeping(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
        stdin: StdinPolicy,
        limit: TimeLimit,
        keep_stdout: Option<usize>,
    ) -> Result<std::result::Result<GitOutput, TimedOut>> {
        let mut cmd = child_command(self.program.as_os_str(), self.path.as_ref(), Some(cwd));
        cmd.args(args)
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        for var in &self.removed {
            cmd.env_remove(var);
        }
        let exec = Exec {
            log: &self.log,
            tags: self.tags,
            stop: self.stop.as_deref(),
        };
        let run = Run {
            cwd,
            args,
            kind,
            stdin,
            limit,
            keep_stdout,
        };
        exec_limited(exec, cmd, run, &|_| {}, None).await
    }

    /// The environment it sets, beside `PATH` (tests).
    #[cfg(test)]
    fn env(&self) -> &[(String, String)] {
        &self.env
    }
}

/// Where a limited command's record goes, and the Stop that ends it.
struct Exec<'a> {
    log: &'a CommandLog,
    tags: Tags,
    stop: Option<&'a Stop>,
}

/// A limited command's arguments (beside the program, in its `Command`), what it reads on
/// stdin and when it is given up on.
struct Run<'a> {
    cwd: &'a Path,
    args: &'a [&'a str],
    kind: CmdKind,
    stdin: StdinPolicy,
    limit: TimeLimit,
    /// Most bytes of stdout kept (the last ones); `None`: stdout and stderr are discarded,
    /// never read.
    keep_stdout: Option<usize>,
}

/// Runs `cmd` (built by [`child_command`] and the runner's own options) as
/// [`GitRunner::run_with_timeout`] describes, for git and every other program alike: stdin
/// as `run.stdin` says, the output's tail kept, given up on at `run.limit`, on `cancel` or
/// at the Stop, its process group ended then, and logged under `exec.tags`.
async fn exec_limited(
    exec: Exec<'_>,
    mut cmd: Command,
    run: Run<'_>,
    on_stderr: &(dyn Fn(&[u8]) + Send + Sync),
    cancel: Option<&tokio::sync::Notify>,
) -> Result<std::result::Result<GitOutput, TimedOut>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let Run {
        cwd,
        args,
        kind,
        stdin,
        limit,
        keep_stdout,
    } = run;
    if exec.stop.is_some_and(Stop::is_stopped) {
        return Err(stopped_error(""));
    }
    let started = Instant::now();
    if keep_stdout.is_some() {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let input = match stdin {
        StdinPolicy::Null => {
            cmd.stdin(Stdio::null());
            None
        }
        StdinPolicy::Bytes(bytes) => {
            cmd.stdin(Stdio::piped());
            Some(bytes)
        }
    };
    let fail = |e: io::Error| {
        let err = spawn_error_of(exec.tags.program, cwd, e);
        exec.log
            .push(exec.tags, kind, cwd, args, started, -1, b"", &err.message);
        err
    };
    let mut child = cmd.spawn().map_err(fail)?;
    let group = child.id();
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut stdin_pipe = child.stdin.take();
    // When git last wrote anything, for an idle limit.
    let last_output = Mutex::new(tokio::time::Instant::now());
    let output_seen = || {
        *last_output.lock().unwrap_or_else(|e| e.into_inner()) = tokio::time::Instant::now();
    };
    // Outside the reading future, so what was read is still there when it is dropped
    // on timeout.
    let (out, err) = (
        Mutex::new(Tail::keeping(keep_stdout.unwrap_or(0))),
        Mutex::new(Tail::default()),
    );
    let take = |t: &Mutex<Tail>| {
        let keep = lock_tail(t).keep;
        std::mem::replace(&mut *lock_tail(t), Tail::keeping(keep)).into_bytes()
    };
    let read = async {
        let read_out = async {
            if let Some(p) = stdout_pipe.as_mut() {
                let mut buf = [0u8; 8192];
                while let Ok(n @ 1..) = p.read(&mut buf).await {
                    lock_tail(&out).push(&buf[..n]);
                    output_seen();
                }
            }
        };
        let read_err = async {
            if let Some(p) = stderr_pipe.as_mut() {
                let mut buf = [0u8; 8192];
                while let Ok(n @ 1..) = p.read(&mut buf).await {
                    lock_tail(&err).push(&buf[..n]);
                    output_seen();
                    on_stderr(&buf[..n]);
                }
            }
        };
        // Written beside the reads, so a program that answers before it has read all of
        // its input never blocks on a full pipe; then closed, so it sees the end.
        let write_in = async {
            if let (Some(pipe), Some(bytes)) = (stdin_pipe.as_mut(), input.as_deref()) {
                let _ = pipe.write_all(bytes).await;
                let _ = pipe.shutdown().await;
            }
            drop(stdin_pipe.take());
        };
        tokio::join!(read_out, read_err, write_in);
        (take(&out), take(&err))
    };
    let run = async {
        let ((out, err), status) = tokio::join!(read, child.wait());
        (out, err, status)
    };
    let deadline = tokio::time::Instant::now() + limit.total;
    let expired = async {
        loop {
            let idle_due = limit
                .idle
                .map(|idle| *last_output.lock().unwrap_or_else(|e| e.into_inner()) + idle);
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Expired::Total(limit.total);
            }
            match (idle_due, limit.idle) {
                (Some(due), Some(idle)) if now >= due => return Expired::Idle(idle),
                (Some(due), _) => tokio::time::sleep_until(due.min(deadline)).await,
                _ => tokio::time::sleep_until(deadline).await,
            }
        }
    };
    let cancelled = async {
        let stopped = async {
            match exec.stop {
                Some(stop) => stop.stopped().await,
                None => std::future::pending().await,
            }
        };
        match cancel {
            Some(cancel) => tokio::select! {
                () = cancel.notified() => {}
                () = stopped => {}
            },
            None => stopped.await,
        }
    };
    let expired = async {
        tokio::select! {
            hit = expired => hit,
            () = cancelled => Expired::Cancelled,
        }
    };
    let finished = {
        tokio::pin!(run);
        let finished = tokio::select! {
            done = &mut run => Ok(done),
            hit = expired => Err(hit),
        };
        // Cancelled: ask git to stop, so that it removes its lock files, and give it a
        // moment before it is killed with the rest.
        #[cfg(unix)]
        if let Err(Expired::Cancelled) = finished
            && let Some(pid) = group.and_then(|p| libc::pid_t::try_from(p).ok())
        {
            // SAFETY: killpg only sends a signal; a group that is gone is ESRCH.
            unsafe {
                libc::killpg(pid, libc::SIGTERM);
            }
            let _ = tokio::time::timeout(CANCEL_GRACE, &mut run).await;
        }
        finished
    };
    match finished {
        Ok((out, err, Ok(status))) => {
            let code = status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&err).into_owned();
            exec.log
                .push(exec.tags, kind, cwd, args, started, code, &out, &stderr);
            Ok(Ok(GitOutput {
                stdout: out,
                stderr,
                code,
            }))
        }
        Ok((_, _, Err(e))) => Err(fail(e)),
        Err(hit) => {
            #[cfg(unix)]
            if let Some(pid) = group.and_then(|p| libc::pid_t::try_from(p).ok()) {
                // SAFETY: killpg only sends a signal; a group that is gone is ESRCH.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            let _ = group;
            let _ = child.kill().await;
            exec.log
                .push(exec.tags, kind, cwd, args, started, -1, b"", "timed out");
            Ok(Err(TimedOut {
                limit: hit,
                stdout: take(&out),
                stderr: String::from_utf8_lossy(&take(&err)).into_owned(),
            }))
        }
    }
}

/// Ends the process group `group` (git's pid) of a stopped command whose output (not yet
/// polled to its end) is `output`, and returns that output if it ends: SIGTERM, so git
/// removes its lock files, then SIGKILL if it has not ended after [`CANCEL_GRACE`], then up
/// to [`KILLED_GRACE`] for the pipes to close. Elsewhere than on Unix, nothing is returned
/// and git itself is killed when `output` is dropped.
async fn end_group<F: std::future::Future + Unpin>(
    group: Option<u32>,
    output: &mut F,
) -> Option<F::Output> {
    #[cfg(unix)]
    if let Some(pid) = group.and_then(|p| libc::pid_t::try_from(p).ok()) {
        // SAFETY: killpg only sends a signal; a group that is gone is ESRCH.
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
        if let Ok(out) = tokio::time::timeout(CANCEL_GRACE, &mut *output).await {
            return Some(out);
        }
        // SAFETY: as above.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
        return tokio::time::timeout(KILLED_GRACE, output).await.ok();
    }
    let _ = (group, output);
    None
}

/// The `GitFailed` error for a git command that exited with `code`: the first non-empty
/// stderr line as `message` and the full stderr as `detail`.
pub(crate) fn git_failed(args: &[&str], code: i32, stderr: &str) -> CoreError {
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(
            || {
                format!(
                    "git {} exited with code {code}",
                    args.first().unwrap_or(&"")
                )
            },
            str::to_owned,
        );
    CoreError::new(ErrorCode::GitFailed, message).with_detail(stderr)
}

fn spawn_error(cwd: &Path, e: io::Error) -> CoreError {
    spawn_error_of("git", cwd, e)
}

/// Why `program` could not start in `cwd`.
fn spawn_error_of(program: &str, cwd: &Path, e: io::Error) -> CoreError {
    if !cwd.is_dir() {
        CoreError::new(
            ErrorCode::PathMissing,
            format!("folder not found: {}", cwd.display()),
        )
    } else if e.kind() == io::ErrorKind::NotFound {
        let code = if program == "git" {
            ErrorCode::GitNotFound
        } else {
            ErrorCode::Io
        };
        CoreError::new(code, format!("{program} was not found on PATH")).with_detail(e.to_string())
    } else {
        CoreError::new(ErrorCode::Io, format!("could not start {program}"))
            .with_detail(e.to_string())
    }
}

/// The login shell's `PATH`, read at most once; callers that come while it is being read
/// wait for that read.
struct LoginPath(OnceLock<Option<OsString>>);

impl LoginPath {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// The `PATH`, reading it with `resolve` unless it has been read (or is being read).
    fn get(&self, resolve: fn() -> Option<OsString>) -> Option<OsString> {
        self.0.get_or_init(resolve).clone()
    }

    /// Starts reading the `PATH` on a background thread, unless it has been read.
    fn prefetch(&'static self, resolve: fn() -> Option<OsString>) {
        if self.0.get().is_none() {
            std::thread::spawn(move || {
                self.0.get_or_init(resolve);
            });
        }
    }
}

/// The process's login shell `PATH`.
static LOGIN_PATH: LoginPath = LoginPath::new();

/// Starts reading the login shell's `PATH` on a background thread, so that
/// [`GitRunner::detect`] finds it ready instead of waiting for the shell (a few hundred ms
/// with a typical profile). Call first thing at app start; calling it again does nothing.
pub fn prefetch_login_path() {
    LOGIN_PATH.prefetch(resolve_login_path);
}

/// The `PATH` of the user's login shell (`$SHELL -ilc`, printing `PATH` between markers), so
/// git and its helpers are found when the app is launched from a desktop that did not load
/// the shell profile. `None` if `SHELL` is unset, the shell fails, takes over 3 s or prints
/// no marked `PATH`, or not on unix.
pub fn resolve_login_path() -> Option<OsString> {
    #[cfg(unix)]
    {
        let shell = std::env::var_os("SHELL")?;
        login_path_from_shell(Path::new(&shell), LOGIN_SHELL_TIMEOUT)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn login_path_from_shell(shell: &Path, timeout: Duration) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;

    let script = r#"printf '\n__GG_PATH__%s__GG_PATH__' "$PATH""#;
    let out = shell_output(shell, &["-ilc", script], timeout)?;
    let path = between_markers(&out, PATH_MARKER)?;
    (!path.is_empty()).then(|| OsString::from_vec(path.to_vec()))
}

/// What `shell` started with `args` prints on stdout, if it exits successfully within
/// `timeout` (it is killed past it). Stdin is null and stderr is dropped. The output is
/// read on a thread: a process started by the shell's profile may keep stdout open after
/// the shell exits, and reading must not outlive the deadline.
#[cfg(unix)]
pub(crate) fn shell_output(shell: &Path, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::sync::mpsc;

    use std::os::unix::process::CommandExt as _;

    let deadline = Instant::now() + timeout;
    let mut cmd = std::process::Command::new(shell);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A session (and process group) of its own, without a terminal: given up on, the shell
    // is ended with everything its profile started.
    // SAFETY: the closure runs in the child between fork and exec and calls only setsid,
    // which is async-signal-safe and touches no memory of the parent.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().ok()?;

    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                if let Ok(pid) = i32::try_from(child.id()) {
                    // SAFETY: killpg only sends a signal; a group that is gone is ESRCH.
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    // The shell has exited: its output is complete once the reader has it, unless something
    // it started holds stdout open. The reader gets a moment of its own for that, so a shell
    // that ends just before the deadline is not taken for one that printed nothing.
    rx.recv_timeout(
        deadline
            .saturating_duration_since(Instant::now())
            .max(READ_GRACE),
    )
    .ok()
}

/// The bytes between the first two `marker`s in `out`.
#[cfg(unix)]
pub(crate) fn between_markers<'a>(out: &'a [u8], marker: &[u8]) -> Option<&'a [u8]> {
    let find = |hay: &[u8]| hay.windows(marker.len()).position(|w| w == marker);
    let start = find(out)? + marker.len();
    let len = find(&out[start..])?;
    Some(&out[start..start + len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_char_boundaries() {
        // 3 bytes per char: MAX_LOG_TEXT (16384 = 3 * 5461 + 1) falls inside a char, so the
        // cut has to step back to the boundary before it.
        assert_eq!(MAX_LOG_TEXT % 3, 1);
        let t = truncate_text("€".repeat(MAX_LOG_TEXT));
        assert_eq!(t.len(), MAX_LOG_TEXT - 1);
        assert!(t.chars().all(|c| c == '€'));
        assert_eq!(truncate_text("short".to_string()), "short");
    }

    #[test]
    fn tail_keeps_the_last_bytes_from_a_line_start() {
        let mut t = Tail::default();
        t.push(b"short\n");
        assert_eq!(t.into_bytes(), b"short\n");

        let mut t = Tail::default();
        for i in 0..20_000 {
            t.push(format!("line {i}\n").as_bytes());
        }
        let kept = t.into_bytes();
        let text = String::from_utf8(kept).unwrap();
        assert!(
            text.len() <= KEPT_TAIL + DROPPED_MARK.len(),
            "{}",
            text.len()
        );
        assert!(text.starts_with(DROPPED_MARK));
        let first = text[DROPPED_MARK.len()..].lines().next().unwrap();
        assert!(first.starts_with("line "), "starts mid-line: {first:?}");
        assert!(text.ends_with("line 19999\n"));
    }

    /// R15: a command run with a time limit (every network command) runs in a session of
    /// its own, so it has no controlling terminal: ssh cannot open `/dev/tty` and wait there
    /// when the app was started from a terminal. Seen from a shell git starts (an alias),
    /// which shares git's session.
    #[cfg(unix)]
    #[tokio::test]
    async fn limited_commands_run_in_a_session_of_their_own() {
        let runner = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let alias = format!("alias.probe=!echo $$ > '{}'; sleep 3", pidfile.display());
        let cwd = dir.path().to_path_buf();
        let run = tokio::spawn(async move {
            runner
                .run_with_timeout(
                    &cwd,
                    &["-c", &alias, "probe"],
                    CmdKind::Read,
                    TimeLimit::total(Duration::from_secs(20)),
                )
                .await
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let pid = loop {
            if let Some(pid) = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|s| s.trim().parse::<libc::pid_t>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "the alias ran");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        // SAFETY: getsid only reads a process's session id.
        let (theirs, ours) = unsafe { (libc::getsid(pid), libc::getsid(0)) };
        assert!(theirs > 0, "the shell is running");
        assert_ne!(theirs, ours, "git runs in a session of its own");
        let out = run.await.unwrap().unwrap().expect("finished");
        assert_eq!(out.code, 0);
    }

    /// A tool runner of `/bin/sh` (as `gh`), logging to `log`.
    #[cfg(unix)]
    fn sh_tool(log: Arc<CommandLog>, stop: Option<Arc<Stop>>) -> ToolRunner {
        let git = GitRunner {
            log,
            path: None,
            tags: Tags::default(),
            stop,
            version: None,
            index_file: None,
        };
        git.for_repo(3).tool(PathBuf::from("/bin/sh"), "gh")
    }

    /// The git another program runs gets every setting the app gives its own git (each
    /// `-c` of the leading options), plus no hooks, and git's environment. The settings go
    /// into `GIT_CONFIG_PARAMETERS` after whatever was inherited there (the last of a key
    /// wins), and the user's own `GIT_CONFIG_COUNT` settings are left alone (Task 9 review
    /// minor 1).
    #[test]
    fn the_protection_for_another_programs_git_follows_the_apps_own() {
        let env = protection_env_after(Some("'core.fsmonitor=evil' 'user.name=it'\\''s'"));
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        for (k, v) in GIT_ENV {
            assert_eq!(get(k), Some(v), "{k}");
        }
        assert_eq!(get("GIT_CONFIG_COUNT"), None, "the user's own are kept");
        let settings: Vec<&str> = LEADING_ARGS
            .iter()
            .copied()
            .filter(|a| *a != "-c")
            .collect();
        assert_eq!(
            LEADING_ARGS.len(),
            settings.len() * 2,
            "every setting has its -c"
        );
        let mut expected = "'core.fsmonitor=evil' 'user.name=it'\\''s'".to_owned();
        for setting in &settings {
            expected.push_str(&format!(" '{setting}'"));
        }
        expected.push_str(&format!(" 'core.hooksPath={NULL_DEVICE}'"));
        assert_eq!(get("GIT_CONFIG_PARAMETERS"), Some(expected.as_str()));
        let alone = protection_env_after(None);
        let params = alone
            .iter()
            .find(|(n, _)| n == "GIT_CONFIG_PARAMETERS")
            .unwrap();
        assert!(
            params.1.starts_with("'core.quotePath=false'"),
            "{}",
            params.1
        );
        assert_eq!(config_quoted("a'b"), "'a'\\''b'");
    }

    /// What git makes of it (checked against git itself): the app's settings win over an
    /// inherited `GIT_CONFIG_PARAMETERS` and `GIT_CONFIG_COUNT`, and other keys of the
    /// user's stay.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_protection_wins_over_inherited_config_and_keeps_the_rest() {
        let repo = crate::testutil::TestRepo::new();
        let mut tool = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        }
        .tool(PathBuf::from("/bin/sh"), "gh")
        .with_env("GIT_CONFIG_COUNT", "2")
        .with_env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .with_env("GIT_CONFIG_VALUE_0", "from-count")
        .with_env("GIT_CONFIG_KEY_1", "user.name")
        .with_env("GIT_CONFIG_VALUE_1", "Ada");
        tool.env
            .extend(protection_env_after(Some("'core.fsmonitor=from-params'")));
        let out = tool
            .run(
                repo.path(),
                &[
                    "-c",
                    "git config --get core.fsmonitor; git config --get user.name",
                ],
                CmdKind::Read,
                StdinPolicy::Null,
                TimeLimit::total(Duration::from_secs(10)),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.text(), "false\nAda\n", "{}", out.stderr);
    }

    /// A command whose output must not reach the app: discarded, never read, and the log
    /// records the command alone; variables removed are not inherited.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_status_only_command_keeps_no_output_and_loses_the_variables_removed() {
        let log = CommandLog::new();
        let tool = sh_tool(log.clone(), None).without_env("HOME");
        let out = tool
            .run_status_only(
                Path::new("/"),
                &[
                    "-c",
                    "echo SECRET; echo SECRET >&2; [ -z \"$HOME\" ] && exit 3",
                ],
                CmdKind::Read,
                TimeLimit::total(Duration::from_secs(10)),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.code, 3, "HOME was removed");
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
        let records = log.since(0);
        assert_eq!(records.len(), 1);
        assert!(records[0].stdout.is_empty() && records[0].stderr.is_empty());
    }

    /// Another program runs on git's spawn path: no controlling terminal (a session of its
    /// own), stdin null unless bytes are given, which it reads exactly and then sees end;
    /// the variables given are set; its record shows its name, masked like git's.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_runs_without_a_terminal_and_reads_only_what_it_is_given() {
        let log = CommandLog::new();
        let tool = sh_tool(log.clone(), None).with_env("GH_PROMPT_DISABLED", "1");
        assert_eq!(
            tool.env(),
            [("GH_PROMPT_DISABLED".to_owned(), "1".to_owned())]
        );
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!(
            "if (: < /dev/tty) 2>/dev/null; then echo tty; else echo no-tty; fi; \
             echo \"prompt=$GH_PROMPT_DISABLED\"; echo $$ > '{}'; sleep 1; od -An -c",
            pidfile.display()
        );
        let limit = TimeLimit::total(Duration::from_secs(20));
        let args = ["-c", script.as_str(), "https://ada:SECRET@github.com/o/r"];
        let run = |stdin| tool.run(dir.path(), &args, CmdKind::Read, stdin, limit);
        let (null, pid) = tokio::join!(run(StdinPolicy::Null), async {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(pid) = std::fs::read_to_string(&pidfile)
                    .ok()
                    .and_then(|s| s.trim().parse::<libc::pid_t>().ok())
                {
                    // SAFETY: getsid only reads a process's session id.
                    break unsafe { (libc::getsid(pid), libc::getsid(0)) };
                }
                assert!(Instant::now() < deadline, "the script ran");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let (theirs, ours) = pid;
        assert!(theirs > 0, "the script is running");
        assert_ne!(theirs, ours, "it runs in a session of its own");
        let null = null.unwrap().expect("finished");
        assert_eq!(null.code, 0, "{}", null.stderr);
        let lines: Vec<&str> = std::str::from_utf8(&null.stdout).unwrap().lines().collect();
        assert_eq!(
            lines,
            ["no-tty", "prompt=1"],
            "read nothing from a null stdin"
        );

        let body = "line one\n- `two` \u{e9}\n".as_bytes().to_vec();
        let given = run(StdinPolicy::Bytes(body.clone()))
            .await
            .unwrap()
            .unwrap();
        let mut piped = Command::new("od");
        piped
            .args(["-An", "-c"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut od = piped.spawn().unwrap();
        {
            use tokio::io::AsyncWriteExt;
            let mut input = od.stdin.take().unwrap();
            input.write_all(&body).await.unwrap();
        }
        let expected = od.wait_with_output().await.unwrap().stdout;
        let text = String::from_utf8_lossy(&given.stdout).into_owned();
        assert!(
            text.ends_with(&*String::from_utf8_lossy(&expected)),
            "the exact bytes, then the end: {text:?}"
        );

        let records = log.since(0);
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .all(|r| r.program == "gh" && r.repo_id == Some(3))
        );
        assert_eq!(records[0].args[2], "https://***@github.com/o/r");
    }

    /// A tool that does not end within its limit is killed with what it started; a
    /// stopped operation's tool ends at the Stop.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tool_is_given_up_on_at_its_limit_and_at_the_stop() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let gave_up = sh_tool(CommandLog::new(), None)
            .run(
                dir.path(),
                &["-c", "sleep 30"],
                CmdKind::Read,
                StdinPolicy::Null,
                TimeLimit::total(Duration::from_millis(500)),
            )
            .await
            .unwrap()
            .expect_err("timed out");
        assert_eq!(gave_up.limit, Expired::Total(Duration::from_millis(500)));
        assert!(started.elapsed() < Duration::from_secs(10));

        let stop = Stop::new();
        let tool = sh_tool(CommandLog::new(), Some(stop.clone()));
        let path = dir.path().to_path_buf();
        let run = tokio::spawn(async move {
            tool.run(
                &path,
                &["-c", "sleep 30"],
                CmdKind::Write,
                StdinPolicy::Bytes(b"x".to_vec()),
                TimeLimit::total(Duration::from_secs(60)),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.stop();
        let out = run.await.unwrap().unwrap().expect_err("stopped");
        assert_eq!(out.limit, Expired::Cancelled);
    }

    /// M3: the version is read from `git --version`, and one older than 2.29 is a problem
    /// said once, plainly.
    #[test]
    fn git_versions_are_read_and_old_ones_said() {
        assert_eq!(
            parse_git_version("git version 2.54.0 (Apple Git-157)\n"),
            Some((2, 54, 0))
        );
        assert_eq!(
            parse_git_version("git version 2.25.1.windows.1"),
            Some((2, 25, 1))
        );
        assert_eq!(parse_git_version("git version 2.29"), Some((2, 29, 0)));
        assert_eq!(parse_git_version("nonsense"), None);
        assert_eq!(version_problem((2, 29, 0)), None);
        assert_eq!(version_problem((3, 0, 0)), None);
        let said = version_problem((2, 25, 1)).unwrap();
        assert!(
            said.starts_with("Git 2.29 or newer is needed; found 2.25.1."),
            "{said}"
        );
    }

    /// R41: every git child, not only network ones, runs in a session of its own (no
    /// controlling terminal: a hook or a signing program that would wait on `/dev/tty`
    /// fails at once), with stdin null.
    #[cfg(unix)]
    #[tokio::test]
    async fn every_command_runs_in_a_session_of_its_own_without_stdin() {
        let runner = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let alias = format!(
            "alias.probe=!echo $$ > '{}'; sleep 2; cat; echo read-stdin",
            pidfile.display()
        );
        let cwd = dir.path().to_path_buf();
        let run = tokio::spawn(async move {
            runner
                .run_unchecked(&cwd, &["-c", &alias, "probe"], CmdKind::Read)
                .await
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let pid = loop {
            if let Some(pid) = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|s| s.trim().parse::<libc::pid_t>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "the alias ran");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        // SAFETY: getsid only reads a process's session id.
        let (theirs, ours) = unsafe { (libc::getsid(pid), libc::getsid(0)) };
        assert!(theirs > 0, "the shell is running");
        assert_ne!(theirs, ours, "git runs in a session of its own");
        let out = run.await.unwrap().unwrap();
        assert_eq!(out.code, 0);
        // stdin is null: `cat` read nothing and ended.
        assert_eq!(out.text(), "read-stdin\n");
    }

    /// A stopped operation's runner starts nothing more; one stopped while git runs ends it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stopped_runner_ends_its_command_and_starts_no_other() {
        let stop = Stop::new();
        let runner = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: Some(stop.clone()),
            version: None,
            index_file: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let started = Instant::now();
        let run = tokio::spawn({
            let runner = runner.clone();
            let cwd = cwd.clone();
            async move {
                runner
                    .run_unchecked(
                        &cwd,
                        &["-c", "alias.wait=!sleep 30", "wait"],
                        CmdKind::Write,
                    )
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.stop();
        let err = run.await.unwrap().expect_err("stopped");
        assert_eq!(err.message, STOPPED);
        assert!(started.elapsed() < Duration::from_secs(10));
        let logged = runner.log().since(0);
        assert_eq!(logged.last().map(|r| r.exit_code), Some(-1));
        let err = runner
            .run_unchecked(&cwd, &["--version"], CmdKind::Read)
            .await
            .expect_err("not started");
        assert_eq!(err.message, STOPPED);
        assert_eq!(
            runner.log().since(0).len(),
            logged.len(),
            "nothing more ran"
        );
    }

    /// A runner with no log of interest, for the time-limit tests.
    fn plain_runner() -> GitRunner {
        GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        }
    }

    /// Runs the shell `script` (as a git alias, so as the runner runs git) with `limit`.
    async fn run_script(
        script: &str,
        limit: TimeLimit,
    ) -> std::result::Result<GitOutput, TimedOut> {
        let dir = tempfile::tempdir().unwrap();
        let alias = format!("alias.probe=!{script}");
        plain_runner()
            .run_with_timeout(dir.path(), &["-c", &alias, "probe"], CmdKind::Read, limit)
            .await
            .unwrap()
    }

    /// The idle limit counts from the last output: a command that keeps writing (to stderr,
    /// as git's progress) for longer than it runs to its end. Limits generous against a slow
    /// start (Task 6 review: the end-to-end tests were flaky).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_command_that_keeps_writing_is_not_idle() {
        let started = Instant::now();
        let out = run_script(
            "for i in $(seq 30); do echo x >&2; sleep .1; done",
            TimeLimit::idle(Duration::from_secs(2), Duration::from_secs(20)),
        )
        .await
        .expect("finished");
        assert_eq!(out.code, 0);
        assert!(
            started.elapsed() > Duration::from_secs(2),
            "ran past the idle limit"
        );
    }

    /// A command that writes nothing for the idle limit is given up as idle.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_silent_command_is_given_up_as_idle() {
        let started = Instant::now();
        let timed_out = run_script(
            "sleep 30",
            TimeLimit::idle(Duration::from_millis(500), Duration::from_secs(20)),
        )
        .await
        .expect_err("gave up");
        assert_eq!(timed_out.limit, Expired::Idle(Duration::from_millis(500)));
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    /// However much a command keeps writing, it ends at the total limit, with what it wrote
    /// kept.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_command_that_never_ends_stops_at_the_cap() {
        let timed_out = run_script(
            "while :; do echo still here >&2; sleep .1; done",
            TimeLimit::idle(Duration::from_secs(2), Duration::from_secs(3)),
        )
        .await
        .expect_err("gave up");
        assert_eq!(timed_out.limit, Expired::Total(Duration::from_secs(3)));
        assert!(
            timed_out.stderr.contains("still here"),
            "{:?}",
            timed_out.stderr
        );
    }

    /// Output past the tail is read (so git is never blocked on a full pipe) and dropped.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_with_timeout_keeps_only_the_tail_of_large_output() {
        let runner = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        let dir = tempfile::tempdir().unwrap();
        // git prints back a 300 KiB config value given on its command line.
        let big = "x".repeat(300 * 1024);
        let out = runner
            .run_with_timeout(
                dir.path(),
                &["-c", &format!("gg.big={big}"), "config", "--get", "gg.big"],
                CmdKind::Read,
                TimeLimit::total(Duration::from_secs(20)),
            )
            .await
            .unwrap()
            .expect("finished");
        assert_eq!(out.code, 0);
        assert!(out.stdout.len() <= KEPT_TAIL + DROPPED_MARK.len());
        assert!(out.stdout.starts_with(DROPPED_MARK.as_bytes()));
        assert!(
            out.stdout.len() > KEPT_TAIL / 2,
            "kept {}",
            out.stdout.len()
        );
    }

    #[test]
    fn inherited_repo_vars_are_removed() {
        let runner = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        let cmd = runner.command(None, &["status"]);
        let removed: Vec<_> = cmd
            .as_std()
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
        ] {
            assert!(removed.iter().any(|r| r == var), "{var} not removed");
        }
    }

    /// A login-shell PATH without a working git falls back to the inherited PATH.
    #[tokio::test]
    async fn detect_falls_back_to_the_inherited_path() {
        let runner = GitRunner::detect_with_login_path(
            CommandLog::new(),
            Some(OsString::from("/no/such/dir")),
        )
        .await
        .expect("git found on the inherited PATH");
        assert_eq!(runner.path, None);
        let out = runner
            .run(&std::env::temp_dir(), &["--version"], CmdKind::Read)
            .await
            .unwrap();
        assert!(out.text().starts_with("git version"));

        // A login PATH with git is kept.
        let inherited = std::env::var_os("PATH");
        let runner = GitRunner::detect_with_login_path(CommandLog::new(), inherited.clone())
            .await
            .unwrap();
        assert_eq!(runner.path, inherited);
    }

    #[tokio::test]
    async fn timed_out_commands_are_logged() {
        let log = CommandLog::new();
        let runner = GitRunner {
            log: log.clone(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        runner.log_timed_out(
            Path::new("/r"),
            &["fetch", "--all"],
            CmdKind::Write,
            Instant::now(),
        );
        let records = log.since(0);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].args, ["fetch", "--all"]);
        assert_eq!(records[0].exit_code, -1);
        assert_eq!(records[0].stderr, "timed out");
        assert_eq!(records[0].kind, CmdKind::Write);
    }

    #[test]
    fn error_codes_serialize_snake_case() {
        let err = CoreError::new(ErrorCode::NotARepo, "nope").with_detail("d");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "not_a_repo");
        assert_eq!(json["message"], "nope");
        assert_eq!(json["detail"], "d");
        assert_eq!(
            serde_json::to_value(ErrorCode::GitFailed).unwrap(),
            "git_failed"
        );
        assert_eq!(err.to_string(), "nope");
    }

    fn record(log: &CommandLog, tags: Tags, kind: CmdKind, arg: &str) {
        log.push(
            tags,
            kind,
            Path::new("/r"),
            &[arg],
            Instant::now(),
            0,
            b"out",
            "",
        );
    }

    /// Writes have their own ring: 600 reads after a write leave it kept, with ids still
    /// global and increasing; filters by repository and writes only.
    #[test]
    fn writes_are_kept_apart_from_reads() {
        let log = CommandLog::new();
        let op = Tags {
            repo_id: Some(1),
            op_id: Some(7),
            ..Tags::default()
        };
        record(&log, op, CmdKind::Write, "branch");
        record(&log, op, CmdKind::Write, "checkout");
        for i in 0..600 {
            let repo = Tags {
                repo_id: Some(1 + i % 2),
                op_id: None,
                ..Tags::default()
            };
            record(&log, repo, CmdKind::Read, "status");
        }
        record(&log, Tags::default(), CmdKind::Write, "fetch");

        let all = log.since(0);
        assert_eq!(all.len(), 3 + LOG_CAPACITY);
        assert!(all.windows(2).all(|w| w[0].id < w[1].id));
        assert_eq!(all[0].args, ["branch"]);
        assert_eq!(all[0].stdout, "out");
        assert_eq!(all.last().unwrap().id, 603);
        let filter = |since_id, repo_id, writes_only| {
            log.query(LogFilter {
                since_id,
                repo_id,
                writes_only,
            })
        };
        let writes: Vec<u64> = filter(0, None, true).iter().map(|r| r.id).collect();
        assert_eq!(writes, [1, 2, 603]);
        assert_eq!(filter(1, None, true).len(), 2);
        assert_eq!(filter(0, Some(1), true).len(), 2);
        let repo2 = filter(0, Some(2), false);
        assert_eq!(repo2.len(), LOG_CAPACITY / 2);
        assert!(
            repo2
                .iter()
                .all(|r| r.repo_id == Some(2) && r.kind == CmdKind::Read)
        );
        assert_eq!(log.op_writes(7), Some((1, 2)));
        assert_eq!(log.op_writes(8), None);
    }

    /// A credential in a remote URL (`https://user:token@host/…`) never reaches the log:
    /// the userinfo is masked in the arguments and in the output kept.
    #[test]
    fn credentials_in_urls_are_masked_in_the_log() {
        let log = CommandLog::new();
        log.push(
            Tags::default(),
            CmdKind::Write,
            Path::new("/r"),
            &[
                "push",
                "https://ada:ghp_SECRET@github.com/o/r.git",
                "refs/heads/main",
            ],
            Instant::now(),
            128,
            b"To https://x-access-token:SECRET2@example.com/o/r.git\n",
            "fatal: unable to access 'https://SECRET3@host:8443/r/': 403\nssh://git@h/x and git@h:y\n",
        );
        let r = &log.since(0)[0];
        assert_eq!(r.args[1], "https://***@github.com/o/r.git");
        assert_eq!(r.args[2], "refs/heads/main");
        assert_eq!(r.stdout, "To https://***@example.com/o/r.git\n");
        assert_eq!(
            r.stderr,
            "fatal: unable to access 'https://***@host:8443/r/': 403\nssh://***@h/x and git@h:y\n"
        );
        for text in [&r.args[1], &r.stdout, &r.stderr] {
            assert!(!text.contains("SECRET"), "{text}");
        }
    }

    /// The operation a record belongs to is served with its title, once the operation has
    /// one (records logged before it was titled get it too).
    #[test]
    fn records_carry_their_operations_title() {
        let log = CommandLog::new();
        let op = Tags {
            repo_id: Some(1),
            op_id: Some(7),
            ..Tags::default()
        };
        record(&log, op, CmdKind::Read, "rev-parse");
        log.name_op(7, "Merge feature/x into main");
        record(&log, op, CmdKind::Write, "merge");
        record(&log, Tags::default(), CmdKind::Read, "status");
        let titles: Vec<Option<String>> = log.since(0).into_iter().map(|r| r.op_title).collect();
        assert_eq!(
            titles,
            [
                Some("Merge feature/x into main".to_owned()),
                Some("Merge feature/x into main".to_owned()),
                None
            ]
        );
        // Titles of operations long gone are not kept forever.
        for id in 100..(100 + 2 * LOG_CAPACITY as u64 + 10) {
            log.name_op(id, "x");
        }
        assert!(log.lock().op_titles.len() <= 2 * LOG_CAPACITY);
    }

    #[test]
    fn runners_tag_what_they_log() {
        let log = CommandLog::new();
        let runner = GitRunner {
            log: log.clone(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        let op = runner.for_repo(4).for_op(9, Stop::new());
        op.log_timed_out(Path::new("/r"), &["push"], CmdKind::Write, Instant::now());
        runner.log_timed_out(Path::new("/r"), &["fetch"], CmdKind::Write, Instant::now());
        let records = log.since(0);
        assert_eq!((records[0].repo_id, records[0].op_id), (Some(4), Some(9)));
        assert_eq!((records[1].repo_id, records[1].op_id), (None, None));
    }

    #[test]
    fn git_never_waits_for_an_editor() {
        let runner = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
            stop: None,
            version: None,
            index_file: None,
        };
        let cmd = runner.command(None, &["rebase"]);
        let env: Vec<(String, String)> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        for (var, value) in [
            ("GIT_EDITOR", "true"),
            ("GIT_SEQUENCE_EDITOR", "true"),
            ("GIT_MERGE_AUTOEDIT", "no"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ] {
            assert!(
                env.iter().any(|(k, v)| k == var && v == value),
                "{var}={value} missing"
            );
        }
    }

    #[test]
    fn command_record_serializes_camel_case() {
        let rec = CommandRecord {
            id: 1,
            program: "git".into(),
            kind: CmdKind::Write,
            cwd: "/r".into(),
            args: vec!["status".into()],
            started_at_ms: 5,
            duration_ms: 2,
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            repo_id: Some(3),
            op_id: None,
            op_title: None,
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["kind"], "write");
        assert_eq!(json["program"], "git");
        assert_eq!(json["startedAtMs"], 5);
        assert_eq!(json["durationMs"], 2);
        assert_eq!(json["exitCode"], 0);
        assert_eq!(json["repoId"], 3);
        assert_eq!(json["opId"], serde_json::Value::Null);
        assert_eq!(json["opTitle"], serde_json::Value::Null);
        assert_eq!(serde_json::to_value(CmdKind::Read).unwrap(), "read");
    }

    #[cfg(unix)]
    mod login_path {
        use super::super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::time::Instant;

        fn script(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
            let path = dir.path().join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        // The fake shells set PATH and run the command they are given (`-ilc <command>`),
        // as a login shell would. They answer at once, so they get a generous timeout: on a
        // loaded machine (the whole suite runs in parallel) starting a shell can take
        // seconds, and the 3 s of the app made `login_path_is_used` fail once.
        const FAKE_TIMEOUT: Duration = Duration::from_secs(20);

        #[test]
        fn login_path_is_used() {
            let dir = tempfile::tempdir().unwrap();
            let shell = script(&dir, "shell", "PATH=/custom/bin:/usr/bin:/bin\neval \"$2\"");
            assert_eq!(
                login_path_from_shell(&shell, FAKE_TIMEOUT),
                Some(OsString::from("/custom/bin:/usr/bin:/bin"))
            );
        }

        #[test]
        fn login_path_ignores_rc_noise_around_the_path() {
            let dir = tempfile::tempdir().unwrap();
            let shell = script(
                &dir,
                "shell",
                "PATH=/custom/bin:/bin\necho 'welcome!'\neval \"$2\"\necho 'bye'",
            );
            assert_eq!(
                login_path_from_shell(&shell, FAKE_TIMEOUT),
                Some(OsString::from("/custom/bin:/bin"))
            );
        }

        #[test]
        fn login_path_survives_a_banner_without_newline() {
            let dir = tempfile::tempdir().unwrap();
            let shell = script(
                &dir,
                "shell",
                "PATH='/with space/bin:/bin'\nprintf 'banner'\neval \"$2\"\nprintf 'tail'",
            );
            assert_eq!(
                login_path_from_shell(&shell, FAKE_TIMEOUT),
                Some(OsString::from("/with space/bin:/bin"))
            );
        }

        #[test]
        fn login_path_without_markers_is_none() {
            let dir = tempfile::tempdir().unwrap();
            let shell = script(&dir, "shell", "printf %s /custom/bin:/bin");
            assert_eq!(login_path_from_shell(&shell, FAKE_TIMEOUT), None);
        }

        #[test]
        fn login_path_times_out() {
            let dir = tempfile::tempdir().unwrap();
            let shell = script(&dir, "shell", "sleep 10");
            let started = Instant::now();
            assert_eq!(login_path_from_shell(&shell, LOGIN_SHELL_TIMEOUT), None);
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "{:?}",
                started.elapsed()
            );
        }

        /// Given up on, the shell is ended with what its profile started (its process
        /// group), not only the shell itself.
        #[test]
        fn a_shell_given_up_on_is_ended_with_its_children() {
            let dir = tempfile::tempdir().unwrap();
            let pidfile = dir.path().join("child.pid");
            let shell = script(
                &dir,
                "shell",
                &format!("sleep 30 & echo $! > '{}'; wait", pidfile.display()),
            );
            assert_eq!(shell_output(&shell, &[], Duration::from_millis(500)), None);
            let pid: i32 = std::fs::read_to_string(&pidfile)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            // SAFETY: signal 0 only checks that the process exists.
            while unsafe { libc::kill(pid, 0) } == 0 {
                assert!(Instant::now() < deadline, "the profile's child still runs");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        #[test]
        fn login_path_failure_is_none() {
            let dir = tempfile::tempdir().unwrap();
            let failing = script(&dir, "shell", "printf %s /x; exit 1");
            assert_eq!(login_path_from_shell(&failing, FAKE_TIMEOUT), None);
            let empty = script(&dir, "empty", "true");
            assert_eq!(login_path_from_shell(&empty, FAKE_TIMEOUT), None);
            assert_eq!(
                login_path_from_shell(&dir.path().join("missing"), FAKE_TIMEOUT),
                None
            );
        }
    }

    /// `detect` started while a prefetch is still running waits for it and uses its PATH;
    /// the shell runs once. The resolver is a stand-in, so no real shell is started.
    #[tokio::test]
    async fn detect_waits_for_an_in_flight_prefetch() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SLOT: LoginPath = LoginPath::new();
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn slow_shell() -> Option<OsString> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(300));
            Some(OsString::from("/usr/bin:/bin"))
        }

        let started = Instant::now();
        SLOT.prefetch(slow_shell);
        let runner = GitRunner::detect_from(CommandLog::new(), || SLOT.get(slow_shell))
            .await
            .expect("git runs with /usr/bin:/bin");
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "detect did not wait for the prefetch ({:?})",
            started.elapsed()
        );
        assert_eq!(runner.path, Some(OsString::from("/usr/bin:/bin")));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "the shell ran twice");
        SLOT.prefetch(slow_shell);
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "a finished prefetch ran again"
        );
    }
}
