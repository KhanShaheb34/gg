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

/// Number of commands the log keeps; older ones are dropped.
const LOG_CAPACITY: usize = 500;
/// Longest stdout/stderr text kept per log record, in bytes.
const MAX_LOG_TEXT: usize = 16 * 1024;
/// How long the login shell gets to report its `PATH`.
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(3);

/// Environment for every git child: never prompt, never open an editor or pager, never
/// take optional locks, and keep messages in a stable (C) locale.
const GIT_ENV: [(&str, &str); 5] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_EDITOR", "true"),
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
const INHERITED_REPO_VARS: [&str; 3] = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"];

/// Whether a command only reads the repository or may change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CmdKind {
    Read,
    Write,
}

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
}

/// Ring buffer of the last 500 git commands. Ids start at 1 and increase by one.
pub struct CommandLog {
    inner: Mutex<LogInner>,
}

struct LogInner {
    next_id: u64,
    records: VecDeque<CommandRecord>,
}

impl CommandLog {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(LogInner {
                next_id: 1,
                records: VecDeque::with_capacity(LOG_CAPACITY),
            }),
        })
    }

    /// Records with an id greater than `id`, oldest first. `since(0)` returns everything kept.
    pub fn since(&self, id: u64) -> Vec<CommandRecord> {
        let inner = self.lock();
        let start = inner.records.partition_point(|r| r.id <= id);
        inner.records.range(start..).cloned().collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &self,
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
        if inner.records.len() == LOG_CAPACITY {
            inner.records.pop_front();
        }
        inner.records.push_back(CommandRecord {
            id,
            kind,
            cwd: cwd.to_string_lossy().into_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            started_at_ms: now_ms - i64::from(duration_ms),
            duration_ms,
            exit_code,
            stdout,
            stderr: truncate_text(stderr.to_owned()),
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LogInner> {
        // A panic while holding the lock cannot leave the buffer inconsistent; keep using it.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
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

/// Runs git commands. Cheap to clone; clones share the command log.
#[derive(Clone)]
pub struct GitRunner {
    log: Arc<CommandLog>,
    /// `PATH` for git children, from the user's login shell when it could be read.
    path: Option<OsString>,
}

impl GitRunner {
    /// Finds git (using the login shell's `PATH` when available) and checks that it runs.
    /// Fails with `GitNotFound` otherwise. The check itself is not logged.
    pub async fn detect(log: Arc<CommandLog>) -> Result<Self> {
        let path = tokio::task::spawn_blocking(|| cached_login_path().clone())
            .await
            .unwrap_or(None);
        let runner = Self { log, path };
        let mut cmd = runner.command(None, &["--version"]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match cmd.output().await {
            Ok(out) if out.status.success() => Ok(runner),
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
                self.log
                    .push(kind, cwd, args, started, code, &out.stdout, &stderr);
                Ok(GitOutput {
                    stdout: out.stdout,
                    stderr,
                    code,
                })
            }
            Err(e) => {
                let err = spawn_error(cwd, e);
                self.log
                    .push(kind, cwd, args, started, -1, b"", &err.message);
                Err(err)
            }
        }
    }

    /// Starts a read-only git command with stdout and stderr piped, for output too large to
    /// buffer. Must be called inside a Tokio runtime. Not logged until the caller reports
    /// completion with [`GitRunner::log_stream_done`].
    ///
    /// The caller must read stdout and drain stderr concurrently: git blocks once either pipe
    /// is full, so reading one to the end before the other can deadlock. If reading stdout
    /// fails, kill the child so the stderr drain can finish. For `git log`, use
    /// [`crate::git::log::read_log`], which does all of this.
    pub fn spawn_stream(&self, cwd: &Path, args: &[&str]) -> Result<Child> {
        let mut cmd = self.command(Some(cwd), args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.spawn().map_err(|e| spawn_error(cwd, e))
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
        self.log
            .push(CmdKind::Read, cwd, args, started, exit_code, b"", stderr);
    }

    pub fn log(&self) -> &Arc<CommandLog> {
        &self.log
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

fn cached_login_path() -> &'static Option<OsString> {
    static LOGIN_PATH: OnceLock<Option<OsString>> = OnceLock::new();
    LOGIN_PATH.get_or_init(resolve_login_path)
}

/// The `PATH` of the user's login shell (`$SHELL -ilc 'printf %s "$PATH"'`), so git and its
/// helpers are found when the app is launched from a desktop that did not load the shell
/// profile. `None` if `SHELL` is unset, the shell fails or takes over 3 s, or not on unix.
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
        .args(["-ilc", r#"printf %s "$PATH""#])
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
    let out = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?;
    // Profile scripts may print banners first; the PATH is the last non-empty line.
    let line = out
        .split(|b| *b == b'\n')
        .map(<[u8]>::trim_ascii)
        .rfind(|l| !l.is_empty())?;
    Some(OsString::from_vec(line.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_char_boundaries() {
        let s = "é".repeat(MAX_LOG_TEXT); // 2 bytes per char
        let t = truncate_text(s);
        assert!(t.len() <= MAX_LOG_TEXT);
        assert!(t.len() >= MAX_LOG_TEXT - 1);
        assert_eq!(truncate_text("short".to_string()), "short");
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
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["kind"], "write");
        assert_eq!(json["startedAtMs"], 5);
        assert_eq!(json["durationMs"], 2);
        assert_eq!(json["exitCode"], 0);
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

        #[test]
        fn login_path_is_used() {
            let dir = tempfile::tempdir().unwrap();
            let shell = script(&dir, "shell", "printf %s /custom/bin:/usr/bin:/bin");
            assert_eq!(
                login_path_from_shell(&shell, LOGIN_SHELL_TIMEOUT),
                Some(OsString::from("/custom/bin:/usr/bin:/bin"))
            );
        }

        #[test]
        fn login_path_ignores_rc_noise_before_the_path() {
            let dir = tempfile::tempdir().unwrap();
            let shell = script(&dir, "shell", "echo 'welcome!'\nprintf %s /custom/bin:/bin");
            assert_eq!(
                login_path_from_shell(&shell, LOGIN_SHELL_TIMEOUT),
                Some(OsString::from("/custom/bin:/bin"))
            );
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
            assert_eq!(login_path_from_shell(&failing, LOGIN_SHELL_TIMEOUT), None);
            let empty = script(&dir, "empty", "true");
            assert_eq!(login_path_from_shell(&empty, LOGIN_SHELL_TIMEOUT), None);
            assert_eq!(
                login_path_from_shell(&dir.path().join("missing"), LOGIN_SHELL_TIMEOUT),
                None
            );
        }
    }
}
