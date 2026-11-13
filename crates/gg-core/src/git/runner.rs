//! `GitRunner` is the only place the app spawns `git`. Every command gets the same
//! non-interactive environment and leading `-c` options, and lands in the `CommandLog`.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::process::{Child, Command};

use crate::error::{CoreError, ErrorCode, Result};

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
}

/// A command [`GitRunner::run_with_timeout`] gave up on: the limit it ran into and the tail
/// of what it had written by then.
#[derive(Debug, Clone)]
pub struct TimedOut {
    pub limit: Expired,
    pub stdout: Vec<u8>,
    pub stderr: String,
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

    /// The records `filter` selects, oldest first.
    pub fn query(&self, filter: LogFilter) -> Vec<CommandRecord> {
        let inner = self.lock();
        let after = |ring: &VecDeque<CommandRecord>| {
            let start = ring.partition_point(|r| r.id <= filter.since_id);
            ring.range(start..)
                .filter(|r| filter.repo_id.is_none_or(|id| r.repo_id == Some(id)))
                .cloned()
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
        let stdout = match kind {
            CmdKind::Write => truncate_text(String::from_utf8_lossy(stdout).into_owned()),
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
            kind,
            cwd: cwd.to_string_lossy().into_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            started_at_ms: now_ms - i64::from(duration_ms),
            duration_ms,
            exit_code,
            stdout,
            stderr: truncate_text(stderr.to_owned()),
            repo_id: tags.repo_id,
            op_id: tags.op_id,
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

/// The last [`KEPT_TAIL`] bytes written to it, starting at a line when it had to drop some.
#[derive(Default)]
struct Tail {
    bytes: Vec<u8>,
    dropped: bool,
}

impl Tail {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        // Trimmed only once it holds twice the tail, so each byte is moved at most once.
        if self.bytes.len() > 2 * KEPT_TAIL {
            self.trim();
        }
    }

    fn trim(&mut self) {
        if self.bytes.len() <= KEPT_TAIL {
            return;
        }
        let mut cut = self.bytes.len() - KEPT_TAIL;
        // Start at the next line, if one starts early in the tail.
        if let Some(nl) = self.bytes[cut..].iter().position(|&b| b == b'\n')
            && nl < KEPT_TAIL / 2
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tags {
    repo_id: Option<u32>,
    op_id: Option<u64>,
}

/// Runs git commands. Cheap to clone; clones share the command log.
#[derive(Clone)]
pub struct GitRunner {
    log: Arc<CommandLog>,
    /// `PATH` for git children, from the user's login shell when it could be read.
    path: Option<OsString>,
    tags: Tags,
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
            let runner = Self {
                log: log.clone(),
                path: Some(path),
                tags: Tags::default(),
            };
            if runner.check().await.is_ok() {
                return Ok(runner);
            }
        }
        let runner = Self {
            log,
            path: None,
            tags: Tags::default(),
        };
        runner.check().await?;
        Ok(runner)
    }

    /// This runner, logging its commands as run for repository `repo_id`.
    pub fn for_repo(&self, repo_id: u32) -> Self {
        let mut runner = self.clone();
        runner.tags.repo_id = Some(repo_id);
        runner
    }

    /// This runner, logging its commands as part of operation `op_id`.
    pub(crate) fn for_op(&self, op_id: u64) -> Self {
        let mut runner = self.clone();
        runner.tags.op_id = Some(op_id);
        runner
    }

    /// Runs `git --version`; `GitNotFound` unless it succeeds.
    async fn check(&self) -> Result<()> {
        let mut cmd = self.command(None, &["--version"]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match cmd.output().await {
            Ok(out) if out.status.success() => Ok(()),
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
    /// could not be started.
    pub async fn run_unchecked(
        &self,
        cwd: &Path,
        args: &[&str],
        kind: CmdKind,
    ) -> Result<GitOutput> {
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

    /// Like [`GitRunner::run_unchecked`], for long-running commands whose output is shown
    /// rather than parsed (fetch): only the last [`KEPT_TAIL`] bytes of stdout and of stderr
    /// are kept (the pipes are still read to the end), and it gives up when `limit` is
    /// reached. Then git and everything it started (a remote helper, ssh) are killed, the
    /// command is logged as timed out, and the result is `Ok(Err(limit hit))`. On Unix git
    /// runs in its own process group for this; elsewhere only git itself is killed.
    ///
    /// The group's id is git's pid, taken at spawn. On timeout the group is killed even when
    /// git has already exited (a helper it started still holding its output open): a group
    /// id is not reused while the group has members. Then git is waited for. Its own group
    /// also means that, under a terminal, a helper that tries to read the terminal (an ssh
    /// passphrase prompt; git's own prompts are off) is stopped rather than prompting, until
    /// the timeout kills it. In the app there is no terminal and such a prompt fails at once.
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
        use tokio::io::AsyncReadExt;

        let started = Instant::now();
        let mut cmd = self.command(Some(cwd), args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        let fail = |e: io::Error| {
            let err = spawn_error(cwd, e);
            self.log
                .push(self.tags, kind, cwd, args, started, -1, b"", &err.message);
            err
        };
        let mut child = cmd.spawn().map_err(fail)?;
        let group = child.id();
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        // When git last wrote anything, for an idle limit.
        let last_output = Mutex::new(tokio::time::Instant::now());
        let output_seen = || {
            *last_output.lock().unwrap_or_else(|e| e.into_inner()) = tokio::time::Instant::now();
        };
        // Outside the reading future, so what was read is still there when it is dropped
        // on timeout.
        let (out, err) = (Mutex::new(Tail::default()), Mutex::new(Tail::default()));
        let take = |t: &Mutex<Tail>| std::mem::take(&mut *lock_tail(t)).into_bytes();
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
            tokio::join!(read_out, read_err);
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
        let finished = tokio::select! {
            done = run => Ok(done),
            hit = expired => Err(hit),
        };
        match finished {
            Ok((out, err, Ok(status))) => {
                let code = status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&err).into_owned();
                self.log
                    .push(self.tags, kind, cwd, args, started, code, &out, &stderr);
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
                self.log_timed_out(cwd, args, kind, started);
                Ok(Err(TimedOut {
                    limit: hit,
                    stdout: take(&out),
                    stderr: String::from_utf8_lossy(&take(&err)).into_owned(),
                }))
            }
        }
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

    fn command(&self, cwd: Option<&Path>, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(LEADING_ARGS)
            .args(args)
            .envs(GIT_ENV)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        for var in INHERITED_REPO_VARS {
            cmd.env_remove(var);
        }
        // With PATH overridden, the program lookup for "git" also uses this PATH.
        if let Some(path) = &self.path {
            cmd.env("PATH", path);
        }
        // Test builds only (the app refuses `testutil` in a release build): the developer's
        // global and system config must not change what tests see.
        #[cfg(feature = "testutil")]
        cmd.env("GIT_CONFIG_GLOBAL", crate::testutil::empty_git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_SYSTEM");
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }
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
    if !cwd.is_dir() {
        CoreError::new(
            ErrorCode::PathMissing,
            format!("folder not found: {}", cwd.display()),
        )
    } else if e.kind() == io::ErrorKind::NotFound {
        CoreError::new(ErrorCode::GitNotFound, "git was not found on PATH")
            .with_detail(e.to_string())
    } else {
        CoreError::new(ErrorCode::Io, "could not start git").with_detail(e.to_string())
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
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt;
    use std::sync::mpsc;

    let deadline = Instant::now() + timeout;
    let mut child = std::process::Command::new(shell)
        .args(["-ilc", r#"printf '\n__GG_PATH__%s__GG_PATH__' "$PATH""#])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Read on a thread: a process started by the shell profile may keep stdout open after
    // the shell exits, and reading must not outlive the deadline.
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
    let out = rx
        .recv_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .max(READ_GRACE),
        )
        .ok()?;
    let path = between_markers(&out)?;
    (!path.is_empty()).then(|| OsString::from_vec(path.to_vec()))
}

/// The bytes between the first two [`PATH_MARKER`]s in `out`.
#[cfg(unix)]
fn between_markers(out: &[u8]) -> Option<&[u8]> {
    let find = |hay: &[u8]| {
        hay.windows(PATH_MARKER.len())
            .position(|w| w == PATH_MARKER)
    };
    let start = find(out)? + PATH_MARKER.len();
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

    /// Output past the tail is read (so git is never blocked on a full pipe) and dropped.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_with_timeout_keeps_only_the_tail_of_large_output() {
        let runner = GitRunner {
            log: CommandLog::new(),
            path: None,
            tags: Tags::default(),
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
        };
        record(&log, op, CmdKind::Write, "branch");
        record(&log, op, CmdKind::Write, "checkout");
        for i in 0..600 {
            let repo = Tags {
                repo_id: Some(1 + i % 2),
                op_id: None,
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

    #[test]
    fn runners_tag_what_they_log() {
        let log = CommandLog::new();
        let runner = GitRunner {
            log: log.clone(),
            path: None,
            tags: Tags::default(),
        };
        let op = runner.for_repo(4).for_op(9);
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
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["kind"], "write");
        assert_eq!(json["startedAtMs"], 5);
        assert_eq!(json["durationMs"], 2);
        assert_eq!(json["exitCode"], 0);
        assert_eq!(json["repoId"], 3);
        assert_eq!(json["opId"], serde_json::Value::Null);
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
