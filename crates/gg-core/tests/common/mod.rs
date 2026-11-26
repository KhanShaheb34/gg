//! Helpers shared by the operation tests (`ops_framework`, `ops_local`, `ops_network`,
//! `ops_progress`): a manager with a recording sink and a command log, request builders and
//! repository set-ups. The core's git in test builds reads no global or system config
//! (see `gg_core::testutil::empty_git_config`).
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gg_core::dto::{
    Confirmed, DangerKind, HeadInfo, MergeTarget, MergeTargetKind, OpOutcome, OpRequest, OpStatus,
    Part, RepoSnapshot, ResetMode,
};
use gg_core::git::runner::{CmdKind, CommandLog, CommandRecord, GitRunner};
use gg_core::repo::RepoManager;
use gg_core::testutil::{RecordingSink, TestRepo};
use serde_json::Value;

/// The names an outcome's notice sets off (branches, tags, commits, stashes, then paths, each
/// in the order said), headline and line under it alike.
pub fn names(out: &OpOutcome) -> Vec<String> {
    let all = out.notice.title.iter().chain(&out.notice.note);
    let refs = all.clone().filter_map(|p| match p {
        Part::Ref(r) => Some(r.clone()),
        _ => None,
    });
    let paths = all.filter_map(|p| match p {
        Part::Path(p) => Some(p.clone()),
        _ => None,
    });
    refs.chain(paths).collect()
}

/// Generous bound for anything asynchronous.
pub const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Harness {
    pub m: Arc<RepoManager>,
    pub sink: Arc<RecordingSink>,
    pub log: Arc<CommandLog>,
}

pub async fn harness() -> Harness {
    let log = CommandLog::new();
    let runner = GitRunner::detect(log.clone())
        .await
        .expect("git is installed");
    let sink = Arc::new(RecordingSink::default());
    let m = RepoManager::new(runner, sink.clone());
    Harness { m, sink, log }
}

impl Harness {
    /// Opens `path` and waits for its log and status; returns the snapshot.
    pub async fn open(&self, path: &Path) -> RepoSnapshot {
        let snap = self.m.open(path).await.unwrap_or_else(|e| {
            panic!(
                "open {} failed: {:?}: {}\n{}",
                path.display(),
                e.code,
                e.message,
                e.detail.as_deref().unwrap_or("(no detail)")
            )
        });
        let id = snap.repo_id;
        tokio::time::timeout(TIMEOUT, self.m.wait_loaded(id))
            .await
            .expect("load finishes");
        tokio::time::timeout(TIMEOUT, self.m.wait_status(id))
            .await
            .expect("status read");
        tokio::time::timeout(TIMEOUT, self.m.wait_background(id))
            .await
            .expect("background reads end");
        self.m.snapshot(id).await.unwrap()
    }

    /// The `repo-changed` payloads of repository `id` recorded so far, oldest first.
    pub fn changes(&self, id: u32) -> Vec<Value> {
        self.sink
            .events()
            .into_iter()
            .filter(|(n, v)| n == "repo-changed" && v["repoId"] == id)
            .map(|(_, v)| v)
            .collect()
    }

    /// Runs `request` as the UI does when the user confirms the dangers of `kinds`: asked
    /// once with nothing confirmed, and, if the core asks to confirm dangers all of whose
    /// kinds are in `kinds`, again with those dangers (their tokens) confirmed. A request
    /// expecting [`here`] expects HEAD as it is just before. Panics if it errs or takes too
    /// long.
    pub async fn run_confirmed(
        &self,
        id: u32,
        request: OpRequest,
        kinds: &[DangerKind],
    ) -> OpOutcome {
        let request = self.at_head(id, request).await;
        let out = self.run_with(id, request.clone(), &[]).await;
        if out.status != OpStatus::NeedsConfirmation
            || kinds.is_empty()
            || !out.dangers.iter().all(|d| kinds.contains(&d.kind))
        {
            return out;
        }
        let confirmed: Vec<Confirmed> = out.dangers.iter().map(Confirmed::from).collect();
        self.run_with(id, request, &confirmed).await
    }

    /// Runs `request` with exactly `confirmed` (kinds and tokens); panics if it errs or takes
    /// too long. A request expecting [`here`] expects HEAD as it is just before.
    pub async fn run_with(
        &self,
        id: u32,
        request: OpRequest,
        confirmed: &[Confirmed],
    ) -> OpOutcome {
        let request = self.at_head(id, request).await;
        tokio::time::timeout(TIMEOUT, self.m.run_op(id, request, confirmed))
            .await
            .expect("the operation ends")
            .expect("run_op")
    }

    /// Runs `request` with nothing confirmed.
    pub async fn run(&self, id: u32, request: OpRequest) -> OpOutcome {
        self.run_with(id, request, &[]).await
    }

    /// `request`, expecting HEAD as it is now in repository `id`'s worktree where it
    /// expects [`here`], and the active worktree as it is now where it expects
    /// [`here_wt`].
    pub async fn at_head(&self, id: u32, mut request: OpRequest) -> OpRequest {
        let worktree = self.m.snapshot(id).await.expect("open").active_worktree;
        if let Some(expect) = expect_head_mut(&mut request)
            && *expect == here()
        {
            *expect = head_at(Path::new(&worktree));
        }
        if let Some(expect) = request.expect_worktree_mut()
            && *expect == here_wt()
        {
            *expect = worktree;
        }
        request
    }

    /// The write commands logged after record `mark`.
    pub fn writes_since(&self, mark: u64) -> Vec<CommandRecord> {
        self.log
            .since(mark)
            .into_iter()
            .filter(|r| r.kind == CmdKind::Write)
            .collect()
    }

    /// The id of the newest command logged so far (0 if none).
    pub fn last_log_id(&self) -> u64 {
        self.log.since(0).last().map_or(0, |r| r.id)
    }
}

/// Stands for "HEAD as it is when the request is sent" in requests the harness sends (see
/// [`Harness::at_head`]): what the UI would send, having shown the repository as it is.
pub fn here() -> HeadInfo {
    HeadInfo {
        branch: Some("\u{0}here".to_owned()),
        hash: None,
    }
}

/// Stands for "the active worktree as it is when the request is sent" (see
/// [`Harness::at_head`]).
pub fn here_wt() -> String {
    "\u{0}here".to_owned()
}

/// HEAD of the worktree at `path`, as the UI is shown it.
pub fn head_at(path: &Path) -> HeadInfo {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("git runs");
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    HeadInfo {
        branch: git(&["symbolic-ref", "-q", "HEAD"])
            .and_then(|r| r.strip_prefix("refs/heads/").map(str::to_owned)),
        hash: git(&["rev-parse", "-q", "--verify", "HEAD^{commit}"]),
    }
}

/// The HEAD `request` expects, for a request on the branch checked out.
pub fn expect_head_mut(request: &mut OpRequest) -> Option<&mut HeadInfo> {
    match request {
        OpRequest::Reset { expect_head, .. }
        | OpRequest::Merge { expect_head, .. }
        | OpRequest::Rebase { expect_head, .. }
        | OpRequest::CherryPick { expect_head, .. }
        | OpRequest::Revert { expect_head, .. }
        | OpRequest::Pull { expect_head, .. }
        | OpRequest::StashApply { expect_head, .. }
        | OpRequest::StashPop { expect_head, .. }
        | OpRequest::Continue { expect_head, .. }
        | OpRequest::Abort { expect_head, .. }
        | OpRequest::Skip { expect_head, .. }
        | OpRequest::Commit { expect_head, .. } => Some(expect_head),
        _ => None,
    }
}

pub fn continue_op(even_with_markers: bool) -> OpRequest {
    OpRequest::Continue {
        even_with_markers,
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

pub fn abort() -> OpRequest {
    OpRequest::Abort {
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

/// Skip, for the commit `commit` git is stopped on.
pub fn skip(commit: &str) -> OpRequest {
    OpRequest::Skip {
        expect_head: here(),
        commit: commit.to_owned(),
        expect_worktree: here_wt(),
    }
}

pub fn create_branch(name: &str, at: &str) -> OpRequest {
    OpRequest::CreateBranch {
        name: name.to_owned(),
        at: at.to_owned(),
        checkout: false,
        expect_worktree: here_wt(),
    }
}

/// The records of operation `out` within its `logFrom..=logTo`.
pub fn op_records(log: &CommandLog, out: &OpOutcome) -> Vec<CommandRecord> {
    log.since(out.log_from.saturating_sub(1))
        .into_iter()
        .filter(|r| r.id <= out.log_to && r.op_id == Some(out.op_id))
        .collect()
}

pub fn checkout_branch(branch: &str) -> OpRequest {
    OpRequest::CheckoutBranch {
        branch: branch.to_owned(),
        fast_forward: false,
        expect_worktree: here_wt(),
    }
}

pub fn delete_branch(branch: &str) -> OpRequest {
    OpRequest::DeleteBranch {
        branch: branch.to_owned(),
        expect_worktree: here_wt(),
    }
}

/// The branch HEAD is on, or `None` when detached.
pub fn current_branch(repo: &TestRepo) -> Option<String> {
    repo.git_succeeds(&["symbolic-ref", "-q", "HEAD"]).then(|| {
        let full = repo.git(&["symbolic-ref", "HEAD"]);
        full.strip_prefix("refs/heads/").unwrap_or(&full).to_owned()
    })
}

pub fn merge(
    kind: MergeTargetKind,
    name: &str,
    no_ff: bool,
    squash: bool,
    no_commit: bool,
) -> OpRequest {
    OpRequest::Merge {
        target: MergeTarget {
            kind,
            name: name.to_owned(),
        },
        no_ff,
        squash,
        no_commit,
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

pub fn merge_branch(name: &str) -> OpRequest {
    merge(MergeTargetKind::Branch, name, true, false, false)
}

pub fn rebase(onto: &str) -> OpRequest {
    OpRequest::Rebase {
        onto: onto.to_owned(),
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

/// `main` and `feature`, each with one commit of its own after `base`, touching different
/// files. HEAD on `main`. Returns the tip of `feature`.
pub fn diverged(repo: &TestRepo) -> String {
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    let tip = repo.commit("on feature");
    repo.checkout("main");
    repo.commit("on main");
    tip
}

pub fn reset(hash: &str, mode: ResetMode) -> OpRequest {
    OpRequest::Reset {
        hash: hash.to_owned(),
        mode,
        expect_head: here(),
        expect_worktree: here_wt(),
    }
}

/// `main` and `feature` both change `clash.txt` after `base`. HEAD on `main`.
pub fn conflicting(repo: &TestRepo) {
    repo.write("clash.txt", "base\n");
    repo.commit_all("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("clash.txt", "feature\n");
    repo.commit_all("feature side");
    repo.checkout("main");
    repo.write("clash.txt", "main\n");
    repo.commit_all("main side");
}

/// Whether `name` (`MERGE_HEAD`, `rebase-merge`, …) exists in the repository's git dir.
pub fn in_git_dir(repo: &TestRepo, name: &str) -> bool {
    repo.path().join(".git").join(name).exists()
}

/// `request` asked for in the worktree at `path` where it expects [`here_wt`] (for requests
/// sent straight to the manager rather than through the harness).
pub fn in_wt(path: &Path, mut request: OpRequest) -> OpRequest {
    if let Some(expect) = request.expect_worktree_mut()
        && *expect == here_wt()
    {
        *expect = path.to_string_lossy().into_owned();
    }
    request
}
