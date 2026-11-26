//! Finding the AI tools and running one for a prompt.
//!
//! A tool is found as `gh` is: on the login shell's `PATH` (read again when the user asks gg
//! to check again, so a tool installed since is found), in an absolute folder, never inside
//! the repository; with a tool `PATH` set (tests, the devserver) only in its first folder,
//! and a test build without one finds none, so tests never run a real tool. On Windows a
//! program (`claude.exe`) is taken first; an npm shim (`codex.cmd`) is run too: every
//! argument gg gives these tools is a fixed word, a checked model name or a path of gg's, so
//! `cmd.exe` has nothing of the user's or the repository's to read in it (Rust's standard
//! library quotes them for it, or refuses).
//!
//! Its version is asked only when the settings ask (`<program> --version`, kept until the
//! program file changes), never at start.
//!
//! A generation is an operation of its own in the command log ("Write a commit message with
//! Claude Code (haiku)"): git's reads, then the tool, whose prompt (on stdin) is never logged,
//! nor its answer (a read keeps no stdout); of Codex's stderr, which repeats the prompt, only
//! its settings and error lines are kept. Cancel ends git or the tool with everything it
//! started, as Stop does for an operation.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use super::context::{self, Nothing};
use super::prompt::{self, PrFields};
use super::tools::{ADAPTERS, Adapter, Exit, adapter};
use crate::dto::{
    AiFailureKind, AiOutcome, AiTask, AiTool, AiToolChoice, AiToolInfo, AiTools, EntryStatus,
    Settings,
};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::{CmdKind, Expired, STOPPED, StdinPolicy, Stop, TimeLimit};
use crate::repo::RepoManager;

/// How long a tool may take to answer (Orca gives 60 s; a pull request's details with Codex
/// can take longer).
const ANSWER_LIMIT: Duration = Duration::from_secs(90);
/// How long `<program> --version` may take.
const VERSION_LIMIT: Duration = Duration::from_secs(10);
/// Most bytes of a tool's own output kept as a failure's detail.
const DETAIL_MAX: usize = 4000;
/// Cancels asked for before their generation started are forgotten after this long.
const EARLY_CANCEL_TTL: Duration = Duration::from_secs(60);

/// The AI tools' state in the core: generations running (by the UI's request id, with their
/// Stop), versions asked, and the login `PATH` as last read for them.
pub(crate) struct AiState {
    running: Mutex<HashMap<String, (Arc<Stop>, std::time::Instant)>>,
    /// Per tool: the program, its file's size and time, and the version it said.
    versions: Mutex<HashMap<AiTool, (PathBuf, Option<(u64, SystemTime)>, Option<String>)>>,
    /// The login shell's `PATH` read again on "Check again" (`None`: the one read at start).
    login_path: Mutex<Option<Option<OsString>>>,
    limit_ms: AtomicU64,
}

impl Default for AiState {
    fn default() -> Self {
        Self {
            running: Mutex::default(),
            versions: Mutex::default(),
            login_path: Mutex::default(),
            limit_ms: AtomicU64::new(ANSWER_LIMIT.as_millis() as u64),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A tool found: the program, and the `PATH` to run it with (`None`: git's).
struct Found {
    program: PathBuf,
    path: Option<OsString>,
}

impl RepoManager {
    /// The `PATH` tools are looked for on and run with, and whether it is a tool `PATH`
    /// (first folder only); `None` when none may be looked at (a test build without one).
    fn ai_search_path(&self) -> Option<(Option<OsString>, bool)> {
        let tool_path = lock(&self.tool_path).clone();
        if let Some(tool_path) = tool_path {
            return Some((Some(tool_path), true));
        }
        if cfg!(any(test, feature = "testutil")) {
            return None;
        }
        let reread = lock(&self.ai.login_path).clone();
        Some((reread.unwrap_or_else(|| self.login_path()), false))
    }

    /// Where `a`'s program is, never inside `inside` (the repository).
    fn find_ai_tool(&self, a: &dyn Adapter, inside: Option<&Path>) -> Option<Found> {
        let (path, first_only) = self.ai_search_path()?;
        let search = match (&path, first_only) {
            (Some(p), true) => Some(std::env::split_paths(p).next()?.into_os_string()),
            (p, _) => p.clone(),
        };
        let program = runnable(
            crate::platform::find_program(a.program(), search.as_deref()),
            cfg!(windows),
        )?;
        if let Some(root) = inside {
            let real = dunce::canonicalize(&program).unwrap_or_else(|_| program.clone());
            let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_owned());
            if real.starts_with(&root) || program.starts_with(&root) {
                return None;
            }
        }
        // Run with the PATH it was found on, so what it starts (node) is found the same way.
        let run_path = if first_only || lock(&self.ai.login_path).is_some() {
            path
        } else {
            None
        };
        Some(Found {
            program,
            path: run_path,
        })
    }

    /// The tools, as found now: `versions` also asks each its version (when its program
    /// changed since it was last asked); `recheck` reads the login shell's `PATH` again first.
    pub async fn ai_tools(&self, settings: &Settings, versions: bool, recheck: bool) -> AiTools {
        if recheck && lock(&self.tool_path).is_none() && !cfg!(any(test, feature = "testutil")) {
            let path = tokio::task::spawn_blocking(crate::git::runner::resolve_login_path)
                .await
                .unwrap_or(None);
            if path.is_some() {
                *lock(&self.ai.login_path) = Some(path);
            }
        }
        let mut tools = Vec::new();
        for a in ADAPTERS {
            let found = self.find_ai_tool(a, None);
            let version = match (&found, versions) {
                (Some(f), true) => self.ai_version(a, f).await,
                (Some(f), false) => lock(&self.ai.versions)
                    .get(&a.tool())
                    .filter(|(p, stamp, _)| *p == f.program && *stamp == stamp_of(&f.program))
                    .and_then(|(_, _, v)| v.clone()),
                (None, _) => None,
            };
            tools.push(AiToolInfo {
                tool: a.tool(),
                name: a.name().to_owned(),
                provider: a.provider().to_owned(),
                installed: found.is_some(),
                version,
                path: found.map(|f| f.program.to_string_lossy().into_owned()),
                install_url: a.install_url().to_owned(),
                sign_in: a.sign_in().to_owned(),
                default_model: a.default_model().to_owned(),
                models: a.models().iter().map(|m| (*m).to_owned()).collect(),
            });
        }
        let chosen = chosen_tool(settings.ai_tool, |t| {
            tools.iter().any(|i| i.tool == t && i.installed)
        });
        AiTools { tools, chosen }
    }

    /// `a`'s version (`<program> --version`), asked again only when its program changed.
    async fn ai_version(&self, a: &dyn Adapter, found: &Found) -> Option<String> {
        let stamp = stamp_of(&found.program);
        if let Some((p, s, v)) = lock(&self.ai.versions).get(&a.tool())
            && *p == found.program
            && *s == stamp
            && stamp.is_some()
        {
            return v.clone();
        }
        let mut runner = self.runner.tool(found.program.clone(), a.program());
        if found.path.is_some() {
            runner = runner.with_path(found.path.clone());
        }
        let cwd = std::env::temp_dir();
        let out = runner
            .run(
                &cwd,
                &["--version"],
                CmdKind::Read,
                StdinPolicy::Null,
                TimeLimit::total(VERSION_LIMIT),
            )
            .await;
        let version = match out {
            Ok(Ok(out)) if out.code == 0 => a.version_of(&out.text()),
            _ => None,
        };
        lock(&self.ai.versions).insert(
            a.tool(),
            (found.program.clone(), stamp, version.clone()),
        );
        version
    }

    /// Cancels generation `request_id`: what it runs ends, with all it started. Asked before
    /// the generation started (the requests crossed), it is cancelled as it starts.
    pub fn ai_cancel(&self, request_id: &str) -> bool {
        let mut running = lock(&self.ai.running);
        running.retain(|_, (stop, at)| !stop.is_stopped() || at.elapsed() < EARLY_CANCEL_TTL);
        match running.get(request_id) {
            Some((stop, _)) => {
                stop.stop();
                true
            }
            None => {
                let stop = Stop::new();
                stop.stop();
                running.insert(request_id.to_owned(), (stop, std::time::Instant::now()));
                false
            }
        }
    }

    /// How long a tool may take to answer (tests only; normally 90 s).
    #[cfg(any(test, feature = "testutil"))]
    pub fn set_ai_timeout(&self, limit: Duration) {
        self.ai
            .limit_ms
            .store(limit.as_millis() as u64, Ordering::Relaxed);
    }

    /// Writes what `task` asks for with the tool `settings` choose, in repository `repo_id`;
    /// `request_id` is the UI's name for it, for [`RepoManager::ai_cancel`]. A failure the
    /// user can act on (no tool, signed out, a limit, nothing to describe, …) is an outcome,
    /// not an error; errors are for a request that is wrong (an unknown repository, a branch
    /// that is not there).
    pub async fn ai_generate(
        &self,
        repo_id: u32,
        request_id: &str,
        task: AiTask,
        settings: &Settings,
    ) -> Result<AiOutcome> {
        let repo = self.get(repo_id)?;
        let log_from = repo.runner.log().next_id();
        let failed = |tool: Option<AiTool>, kind, message: String, detail: String| {
            Ok(AiOutcome::Failed {
                tool,
                kind,
                message,
                detail,
                log_from,
            })
        };
        // The tool: the one chosen, or the first installed.
        let order: Vec<&dyn Adapter> = match settings.ai_tool {
            AiToolChoice::Auto => ADAPTERS.to_vec(),
            AiToolChoice::Claude => vec![adapter(AiTool::Claude)],
            AiToolChoice::Codex => vec![adapter(AiTool::Codex)],
        };
        let Some((a, found)) = order
            .iter()
            .find_map(|a| Some((*a, self.find_ai_tool(*a, Some(&repo.root))?)))
        else {
            let message = match settings.ai_tool {
                AiToolChoice::Auto => {
                    "Neither Claude Code nor Codex was found on this computer.".to_owned()
                }
                _ => format!("{} was not found on this computer.", order[0].name()),
            };
            let tool = (settings.ai_tool != AiToolChoice::Auto).then(|| order[0].tool());
            return failed(tool, AiFailureKind::NoTool, message, String::new());
        };
        let tool = a.tool();
        let model = settings
            .ai_models
            .get(tool.id())
            .map_or(a.default_model(), String::as_str)
            .to_owned();
        if let Err(m) = super::tools::check_model(&model) {
            return failed(Some(tool), AiFailureKind::BadModel, m, String::new());
        }

        // Registered before anything runs: a cancel that came first is seen here.
        let stop = {
            let mut running = lock(&self.ai.running);
            match running.get(request_id) {
                Some((stop, _)) if stop.is_stopped() => {
                    running.remove(request_id);
                    return failed(
                        Some(tool),
                        AiFailureKind::Cancelled,
                        "Cancelled.".to_owned(),
                        String::new(),
                    );
                }
                Some(_) => {
                    return Err(CoreError::new(
                        ErrorCode::BadRequest,
                        "That generation is running already.",
                    ));
                }
                None => {
                    let stop = Stop::new();
                    running.insert(
                        request_id.to_owned(),
                        (stop.clone(), std::time::Instant::now()),
                    );
                    stop
                }
            }
        };
        let outcome = self
            .generate_with(&repo, a, &found, &model, task, settings, stop, log_from)
            .await;
        lock(&self.ai.running).remove(request_id);
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    async fn generate_with(
        &self,
        repo: &crate::repo::Repo,
        a: &dyn Adapter,
        found: &Found,
        model: &str,
        task: AiTask,
        settings: &Settings,
        stop: Arc<Stop>,
        log_from: u64,
    ) -> Result<AiOutcome> {
        let tool = a.tool();
        let failed = |kind, message: String, detail: String| {
            Ok(AiOutcome::Failed {
                tool: Some(tool),
                kind,
                message,
                detail,
                log_from,
            })
        };
        let cancelled = || failed(AiFailureKind::Cancelled, "Cancelled.".to_owned(), String::new());
        let shown_model = if model.is_empty() {
            String::new()
        } else {
            format!(" ({model})")
        };
        let op_id = self.next_op_id();
        let runner = repo.runner.for_op(op_id, stop.clone());
        let what = match &task {
            AiTask::Commit { .. } => "a commit message",
            AiTask::Pr { .. } => "a pull request\u{2019}s title and description",
        };
        runner
            .log()
            .name_op(op_id, &format!("Write {what} with {}{shown_model}", a.name()));

        // What it is written from.
        let gathered = match &task {
            AiTask::Commit { path, paths } => {
                let files = self.worktree_files(repo.identity.repo_id, Path::new(path)).await?;
                let ticked: std::collections::BTreeSet<&str> =
                    paths.iter().map(String::as_str).collect();
                let entries: Vec<&crate::dto::FileEntry> = files
                    .files
                    .iter()
                    .filter(|e| e.status != EntryStatus::U)
                    .filter(|e| {
                        ticked.contains(e.path.as_str())
                            || (e.status == EntryStatus::R
                                && e.orig_path.as_deref().is_some_and(|o| ticked.contains(o)))
                    })
                    .collect();
                let worktree = crate::repo::canonical(Path::new(path));
                context::commit_context(&runner, &worktree, &entries)
                    .await
                    .map(|c| c.map(|c| prompt::commit_prompt(settings.ai_commit_prompt.as_deref(), &c)))
            }
            AiTask::Pr {
                branch,
                base,
                base_remote,
                title,
                body,
                draft,
            } => context::pr_context(&runner, &repo.root, branch, base, base_remote.as_deref())
                .await
                .map(|c| {
                    c.map(|mut c| {
                        c.title = title.clone();
                        c.body = body.clone();
                        c.draft = *draft;
                        prompt::pr_prompt(settings.ai_pr_prompt.as_deref(), &c)
                    })
                }),
        };
        let prompt_text = match gathered {
            Ok(Ok(p)) => p,
            Ok(Err(Nothing(message))) => {
                return failed(AiFailureKind::Nothing, message, String::new());
            }
            Err(e) if e.message == STOPPED || stop.is_stopped() => return cancelled(),
            Err(e) => return Err(e),
        };
        if stop.is_stopped() {
            return cancelled();
        }

        // Its own folder, never the repository; removed when done.
        let scratch = Scratch::new()?;
        let run = a.invocation(model, &prompt_text, scratch.path());
        let mut tool_runner = runner
            .tool(found.program.clone(), a.program())
            .with_git_protection()
            // Started from inside a Claude Code session, gg's environment says so; the
            // `claude` gg runs is a session of its own.
            .without_env("CLAUDECODE")
            .without_env("CLAUDE_CODE_ENTRYPOINT")
            .logging_stderr(a.log_scrub());
        if found.path.is_some() {
            tool_runner = tool_runner.with_path(found.path.clone());
        }
        let args: Vec<&str> = run.args.iter().map(String::as_str).collect();
        let limit = Duration::from_millis(self.ai.limit_ms.load(Ordering::Relaxed));
        let ran = tool_runner
            .run(
                scratch.path(),
                &args,
                CmdKind::Read,
                StdinPolicy::Bytes(run.stdin),
                TimeLimit::total(limit),
            )
            .await;
        let out = match ran {
            Ok(Ok(out)) => out,
            Ok(Err(gave_up)) => {
                return match gave_up.limit {
                    Expired::Cancelled => cancelled(),
                    _ => failed(
                        AiFailureKind::TimedOut,
                        format!(
                            "{} did not answer within {} s.",
                            a.name(),
                            limit.as_secs().max(1)
                        ),
                        String::new(),
                    ),
                };
            }
            Err(e) if e.message == STOPPED => return cancelled(),
            Err(e) if e.message.ends_with("was not found on PATH") => {
                return failed(
                    AiFailureKind::NoTool,
                    format!("{} was not found on this computer.", a.name()),
                    String::new(),
                );
            }
            Err(e) => {
                return failed(
                    AiFailureKind::Failed,
                    format!("{} could not be started: {}", a.name(), e.message),
                    e.detail.unwrap_or_default(),
                );
            }
        };
        let exit = Exit {
            code: out.code,
            stdout: out.text(),
            stderr: out.stderr.clone(),
        };
        if out.code != 0 {
            let detail = failure_detail(a, &exit);
            let line = a.failure_line(&exit);
            return match a.classify(&exit) {
                Some((kind, said)) => failed(kind, failure_message(a, kind, &said, model), detail),
                None => failed(
                    AiFailureKind::Failed,
                    match line {
                        Some(line) => format!("{} failed: {}", a.name(), sentence(&line)),
                        None => format!("{} stopped with exit code {}.", a.name(), out.code),
                    },
                    detail,
                ),
            };
        }
        let answer = run
            .answer_file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(exit.stdout);
        match task {
            AiTask::Commit { .. } => {
                match prompt::commit_message(&prompt::clean_answer(&answer)) {
                    Some(message) => Ok(AiOutcome::Done {
                        tool,
                        model: model.to_owned(),
                        message: Some(message),
                        title: None,
                        body: None,
                        draft: None,
                        log_from,
                    }),
                    None => failed(
                        AiFailureKind::Empty,
                        format!("{} returned an empty message.", a.name()),
                        String::new(),
                    ),
                }
            }
            AiTask::Pr {
                title, body, draft, ..
            } => {
                let current = PrFields { title, body, draft };
                match prompt::pr_fields(&answer, &current) {
                    Some(f) => Ok(AiOutcome::Done {
                        tool,
                        model: model.to_owned(),
                        message: None,
                        title: Some(f.title),
                        body: Some(f.body),
                        draft: Some(f.draft),
                        log_from,
                    }),
                    None => failed(
                        AiFailureKind::Empty,
                        format!(
                            "{}\u{2019}s answer had no title or description gg could read.",
                            a.name()
                        ),
                        cut(&answer, DETAIL_MAX),
                    ),
                }
            }
        }
    }
}

/// The program a lookup found that gg runs as an AI tool: a program; on Windows (`windows`)
/// also a `.cmd` or `.bat` shim, as npm installs them (see the module's docs).
fn runnable(found: crate::platform::Lookup, windows: bool) -> Option<PathBuf> {
    match found {
        crate::platform::Lookup::Program(p) => Some(p),
        crate::platform::Lookup::ScriptOnly(p) if windows => Some(p),
        _ => None,
    }
}

/// The tool settings choose, of those `installed` says are there: the one named, or the
/// first in gg's order.
pub(crate) fn chosen_tool(choice: AiToolChoice, installed: impl Fn(AiTool) -> bool) -> Option<AiTool> {
    let order = match choice {
        AiToolChoice::Auto => vec![AiTool::Claude, AiTool::Codex],
        AiToolChoice::Claude => vec![AiTool::Claude],
        AiToolChoice::Codex => vec![AiTool::Codex],
    };
    order.into_iter().find(|t| installed(*t))
}

/// What the user is told for a failure of `kind` the tool explained with `said`.
fn failure_message(a: &dyn Adapter, kind: AiFailureKind, said: &str, model: &str) -> String {
    let name = a.name();
    match kind {
        AiFailureKind::SignedOut => format!(
            "{name} is not signed in. Run {} in a terminal to sign in, then try again.",
            a.sign_in()
        ),
        AiFailureKind::Limited if said.is_empty() => {
            format!("{name} has reached a usage or rate limit. Try again later.")
        }
        AiFailureKind::Limited => format!("{name} has reached a limit: {}", sentence(said)),
        AiFailureKind::BadModel if model.is_empty() => {
            format!("{name} could not use its default model: {}", sentence(said))
        }
        AiFailureKind::BadModel => format!(
            "{name} does not know the model \u{201c}{model}\u{201d}, or your account cannot use it. Choose another in Settings \u{2192} AI."
        ),
        _ => format!("{name} failed: {}", sentence(said)),
    }
}

/// `line` ending in one full stop.
fn sentence(line: &str) -> String {
    let line = line.trim().trim_end_matches('.');
    format!("{line}.")
}

/// The tool's own output for a failure's details: its error line and what the log keeps of
/// its stderr (never the prompt Codex repeats there).
fn failure_detail(a: &dyn Adapter, exit: &Exit) -> String {
    let mut parts = Vec::new();
    if let Some(line) = a.failure_line(exit) {
        parts.push(line);
    }
    let kept = (a.log_scrub())(&exit.stderr);
    let kept = kept.trim();
    if !kept.is_empty() && !parts.iter().any(|p| p == kept) {
        parts.push(kept.to_owned());
    }
    cut(&parts.join("\n"), DETAIL_MAX)
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut at = max;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    format!("{}\u{2026}", &text[..at])
}

/// A file's size and time, to tell when a program was replaced.
fn stamp_of(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// A folder of gg's for one run of a tool, in the system's temporary folder (only the user
/// can enter it, on Unix); removed with what is in it when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        for _ in 0..100 {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!("gg-ai-{}-{n}-{nanos}", std::process::id()));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&dir) {
                Ok(()) => return Ok(Self(dunce::canonicalize(&dir).unwrap_or(dir))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(CoreError::new(
                        ErrorCode::Io,
                        format!("Could not make a temporary folder in {}.", base.display()),
                    )
                    .with_detail(e.to_string()));
                }
            }
        }
        Err(CoreError::new(
            ErrorCode::Io,
            format!("Could not make a temporary folder in {}.", base.display()),
        ))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_takes_the_first_installed() {
        let only = |t: AiTool| move |x: AiTool| x == t;
        assert_eq!(chosen_tool(AiToolChoice::Auto, |_| true), Some(AiTool::Claude));
        assert_eq!(
            chosen_tool(AiToolChoice::Auto, only(AiTool::Codex)),
            Some(AiTool::Codex)
        );
        assert_eq!(chosen_tool(AiToolChoice::Auto, |_| false), None);
        assert_eq!(chosen_tool(AiToolChoice::Codex, only(AiTool::Claude)), None);
        assert_eq!(
            chosen_tool(AiToolChoice::Claude, only(AiTool::Claude)),
            Some(AiTool::Claude)
        );
    }

    /// On Windows a tool installed by npm is a shim (`codex.cmd`), run through cmd.exe with
    /// gg's fixed arguments; a program (`claude.exe`) comes first. Elsewhere only programs.
    #[test]
    fn windows_shims_are_run_and_programs_come_first() {
        use crate::platform::{Lookup, windows_lookup};
        let path = r"C:\Users\me\AppData\Roaming\npm;C:\Users\me\.local\bin";
        let shim_only =
            |f: &str| f == r"C:\Users\me\AppData\Roaming\npm\codex.cmd";
        let found = windows_lookup("codex", path, None, &shim_only);
        assert_eq!(
            found,
            Lookup::ScriptOnly(PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\codex.cmd"))
        );
        assert_eq!(
            runnable(found.clone(), true),
            Some(PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\codex.cmd"))
        );
        assert_eq!(runnable(found, false), None);
        let both = |f: &str| {
            f == r"C:\Users\me\AppData\Roaming\npm\claude.cmd"
                || f == r"C:\Users\me\.local\bin\claude.exe"
        };
        assert_eq!(
            runnable(windows_lookup("claude", path, None, &both), true),
            Some(PathBuf::from(r"C:\Users\me\.local\bin\claude.exe"))
        );
        assert_eq!(runnable(Lookup::Missing, true), None);
    }

    #[test]
    fn failures_read_as_sentences() {
        let claude = adapter(AiTool::Claude);
        assert_eq!(
            failure_message(claude, AiFailureKind::SignedOut, "Not logged in", "haiku"),
            "Claude Code is not signed in. Run claude in a terminal to sign in, then try again."
        );
        assert_eq!(
            failure_message(
                adapter(AiTool::Codex),
                AiFailureKind::Limited,
                "You've hit your usage limit.",
                ""
            ),
            "Codex has reached a limit: You've hit your usage limit."
        );
        assert!(failure_message(claude, AiFailureKind::BadModel, "x", "opus-9").contains("\u{201c}opus-9\u{201d}"));
        assert_eq!(cut("äbc", 1), "\u{2026}");
    }

    #[test]
    fn a_scratch_folder_goes_when_dropped() {
        let scratch = Scratch::new().unwrap();
        let path = scratch.path().to_owned();
        assert!(path.is_dir());
        std::fs::write(path.join("answer.txt"), "x").unwrap();
        drop(scratch);
        assert!(!path.exists());
    }
}
