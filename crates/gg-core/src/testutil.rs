//! Throwaway git repositories for tests (compiled for tests and behind the `testutil` feature).
//!
//! Every repository lives in its own temp directory and runs git with an empty global config
//! and no system config, so hooks, signing or templates on the host cannot affect tests.
//! Commits get fixed, increasing dates, so the same steps always produce the same hashes.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::watch;

use crate::EventSink;

/// Date of the first commit in every `TestRepo` (unix seconds).
const FIRST_COMMIT_TIME: i64 = 1_700_000_000;
/// Seconds between consecutive commits.
const COMMIT_TIME_STEP: i64 = 60;

/// Host variables that would override the repository, identity or dates of our commands.
const HOST_GIT_VARS: [&str; 9] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// A repository folder with this name (`TestRepo::new_in_subdir(UNWATCHABLE)`) cannot be
/// watched: `watch_git_dir` fails for it, as it would for a file system without change
/// notifications.
pub const UNWATCHABLE: &str = "gg-unwatchable";

/// Whether `git_dir` belongs to an [`UNWATCHABLE`] test repository.
pub fn is_unwatchable(git_dir: &Path) -> bool {
    git_dir.components().any(|c| c.as_os_str() == UNWATCHABLE)
}

/// A git repository in a temp directory, deleted on drop. The repository is a subdirectory
/// of `dir`; worktrees are created next to it.
pub struct TestRepo {
    pub dir: TempDir,
    root: PathBuf,
    global_config: PathBuf,
    /// Commits created so far; drives commit dates and unique file names.
    commits: AtomicI64,
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl TestRepo {
    /// An empty repository on branch `main` with identity `Test <test@example.com>` and
    /// commit signing off.
    pub fn new() -> Self {
        Self::new_in_subdir("repo")
    }

    /// Like [`TestRepo::new`], with the repository in a directory named `name` (which may
    /// contain spaces or non-ASCII characters).
    pub fn new_in_subdir(name: &str) -> Self {
        let repo = Self::in_new_tempdir(name);
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    fn in_new_tempdir(name: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("gg-test-")
            .tempdir()
            .expect("create temp dir");
        let root = dir.path().join(name);
        fs::create_dir_all(&root).expect("create repo dir");
        let global_config = dir.path().join("empty-gitconfig");
        fs::write(&global_config, "").expect("write empty global git config");
        Self {
            dir,
            root,
            global_config,
            commits: AtomicI64::new(0),
        }
    }

    /// The repository's working directory (the git dir itself for a bare repository).
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Runs git in the repository and returns trimmed stdout. Panics if git fails.
    pub fn git(&self, args: &[&str]) -> String {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.git_with(&args, &[])
    }

    /// Writes a unique file and commits it; returns the full commit hash.
    pub fn commit(&self, msg: &str) -> String {
        self.commit_with(msg, &[])
    }

    /// Like [`TestRepo::commit`], with `author <email>` as the author (the committer stays
    /// `Test`).
    pub fn commit_as(&self, msg: &str, author: &str, email: &str) -> String {
        self.commit_with(
            msg,
            &[("GIT_AUTHOR_NAME", author), ("GIT_AUTHOR_EMAIL", email)],
        )
    }

    /// Writes `contents` to `rel` (relative to the repository), creating parent directories.
    pub fn write(&self, rel: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, contents).expect("write file");
    }

    /// Stages every change in the working tree and commits it; returns the commit hash.
    pub fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.commit_staged(msg, &[])
    }

    pub fn branch(&self, name: &str) {
        self.git(&["branch", name]);
    }

    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    /// Merges `name` into the current branch with `--no-ff`; returns the merge commit hash.
    pub fn merge(&self, name: &str) -> String {
        let (_, date) = self.next_commit_slot();
        self.git_with(
            &[
                OsStr::new("merge"),
                OsStr::new("-q"),
                OsStr::new("--no-ff"),
                OsStr::new("--no-edit"),
                OsStr::new(name),
            ],
            &[("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)],
        );
        self.git(&["rev-parse", "HEAD"])
    }

    /// Checks out `branch` (created at HEAD if missing) in a new linked worktree next to
    /// the repository; returns the worktree path.
    pub fn add_worktree(&self, branch: &str) -> PathBuf {
        let path = self
            .dir
            .path()
            .join(format!("wt-{}", branch.replace('/', "-")));
        let branch_ref = format!("refs/heads/{branch}");
        let mut args = vec![OsStr::new("worktree"), OsStr::new("add"), OsStr::new("-q")];
        if self.git_succeeds(&["rev-parse", "--verify", "--quiet", &branch_ref]) {
            args.extend([path.as_os_str(), OsStr::new(branch)]);
        } else {
            args.extend([OsStr::new("-b"), OsStr::new(branch), path.as_os_str()]);
        }
        self.git_with(&args, &[]);
        path
    }

    /// Appends `n` commits (`bulk 0`, `bulk 1`, ...) to the current branch in one
    /// `git fast-import` run, much faster than `n` calls to [`TestRepo::commit`]. Each commit
    /// keeps its parent's tree. Resets the index and working tree to the new tip (discarding
    /// uncommitted changes). Returns the last commit's hash.
    pub fn commit_many(&self, n: usize) -> String {
        let branch = self.git(&["symbolic-ref", "HEAD"]);
        let mut parent = self
            .git_succeeds(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .then(|| self.git(&["rev-parse", "HEAD"]));
        let mut script = String::new();
        for i in 0..n {
            let (_, date) = self.next_commit_slot();
            let msg = format!("bulk {i}");
            let _ = writeln!(script, "commit {branch}\nmark :{}", i + 1);
            let _ = writeln!(script, "author Test <test@example.com> {date}");
            let _ = writeln!(script, "committer Test <test@example.com> {date}");
            let _ = writeln!(script, "data {}\n{msg}", msg.len());
            if let Some(p) = parent.take() {
                let _ = writeln!(script, "from {p}");
            }
            script.push('\n');
        }
        let mut child = self
            .command(&[OsStr::new("fast-import"), OsStr::new("--quiet")], &[])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("start git fast-import");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(script.as_bytes())
            .expect("write fast-import script");
        assert!(child.wait().expect("wait for fast-import").success());
        // fast-import updates the branch but not the index or working tree.
        self.git(&["reset", "-q", "--hard"]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Creates a bare repository, adds it as remote `origin`, pushes all branches and tags
    /// to it and fetches. Returns the bare repository, which owns its own temp directory:
    /// keep it alive for as long as the remote is used.
    #[must_use]
    pub fn clone_as_remote(&self) -> TestRepo {
        let origin = Self::in_new_tempdir("origin.git");
        origin.git(&["init", "-q", "--bare", "-b", "main"]);
        self.git_with(
            &[
                OsStr::new("remote"),
                OsStr::new("add"),
                OsStr::new("origin"),
                origin.path().as_os_str(),
            ],
            &[],
        );
        self.git(&["push", "-q", "origin", "--all"]);
        self.git(&["push", "-q", "origin", "--tags"]);
        self.git(&["fetch", "-q", "origin"]);
        origin
    }

    /// A clone of `remote` (a repository made by [`TestRepo::clone_as_remote`]), as another
    /// user's: its own temp directory, the same identity, and commit dates and file names
    /// that do not collide with the original's.
    pub fn clone_of(remote: &TestRepo) -> TestRepo {
        let repo = Self::in_new_tempdir("clone");
        repo.commits.store(1000, Ordering::SeqCst);
        repo.git_with(
            &[
                OsStr::new("clone"),
                OsStr::new("-q"),
                remote.path().as_os_str(),
                OsStr::new("."),
            ],
            &[],
        );
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    fn commit_with(&self, msg: &str, env: &[(&str, &str)]) -> String {
        let n = self.commits.load(Ordering::SeqCst);
        let file = format!("file-{n}.txt");
        fs::write(self.root.join(&file), format!("{msg}\n")).expect("write commit file");
        self.git(&["add", "--", &file]);
        self.commit_staged(msg, env)
    }

    /// Commits the index with the next commit date; returns the commit hash.
    fn commit_staged(&self, msg: &str, env: &[(&str, &str)]) -> String {
        let (_, date) = self.next_commit_slot();
        let mut env = env.to_vec();
        env.extend([
            ("GIT_AUTHOR_DATE", date.as_str()),
            ("GIT_COMMITTER_DATE", date.as_str()),
        ]);
        self.git_with(
            &[
                OsStr::new("commit"),
                OsStr::new("-q"),
                OsStr::new("-m"),
                OsStr::new(msg),
            ],
            &env,
        );
        self.git(&["rev-parse", "HEAD"])
    }

    /// Takes the next commit slot: its index and its date in git's `<unix> <tz>` format.
    fn next_commit_slot(&self) -> (i64, String) {
        let n = self.commits.fetch_add(1, Ordering::SeqCst);
        let date = format!("{} +0000", FIRST_COMMIT_TIME + n * COMMIT_TIME_STEP);
        (n, date)
    }

    fn command(&self, args: &[&OsStr], env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.root).stdin(Stdio::null());
        for var in HOST_GIT_VARS {
            cmd.env_remove(var);
        }
        cmd.env("GIT_CONFIG_GLOBAL", &self.global_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("LC_ALL", "C")
            .envs(env.iter().copied());
        cmd
    }

    fn git_with(&self, args: &[&OsStr], env: &[(&str, &str)]) -> String {
        let out = self
            .command(args, env)
            .output()
            .unwrap_or_else(|e| panic!("could not run git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            self.root.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Runs git in the repository with output discarded; returns whether it exited 0.
    /// For steps expected to fail, such as a conflicting merge.
    pub fn git_succeeds(&self, args: &[&str]) -> bool {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.command(&args, &[])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// What git's global config is in tests, so that no test reads the developer's (the
/// core's runner uses it in test builds; see `GitRunner`): the platform's null device,
/// which reads as empty. No file is made, so there is nothing another user could put
/// config into.
pub fn empty_git_config() -> &'static Path {
    Path::new(if cfg!(windows) { "NUL" } else { "/dev/null" })
}

/// An [`EventSink`] that records every event, for tests.
pub struct RecordingSink {
    events: Mutex<Vec<(String, Value)>>,
    /// Number of events recorded so far; lets waiters sleep until the next one.
    count: watch::Sender<usize>,
}

impl Default for RecordingSink {
    fn default() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            count: watch::channel(0).0,
        }
    }
}

impl RecordingSink {
    /// Every `(name, payload)` emitted so far, oldest first.
    pub fn events(&self) -> Vec<(String, Value)> {
        self.lock().clone()
    }

    /// The first event (already recorded or still to come) matching `pred`, or `None` if
    /// none arrives within `timeout`.
    pub async fn wait_for(
        &self,
        timeout: Duration,
        pred: impl Fn(&str, &Value) -> bool,
    ) -> Option<(String, Value)> {
        let mut seen = self.count.subscribe();
        let find = async {
            loop {
                seen.borrow_and_update();
                if let Some(e) = self.lock().iter().find(|(n, v)| pred(n, v)) {
                    return e.clone();
                }
                // The sender lives as long as `self`, so this only waits for the next event.
                let _ = seen.changed().await;
            }
        };
        tokio::time::timeout(timeout, find).await.ok()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(String, Value)>> {
        self.events.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, name: &str, payload: Value) {
        self.lock().push((name.to_owned(), payload));
        self.count.send_modify(|n| *n += 1);
    }
}
