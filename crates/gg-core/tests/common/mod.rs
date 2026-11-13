//! Helpers shared by the operation tests (`ops_framework`, `ops_local`, `ops_network`,
//! `ops_progress`): a manager with a recording sink and a command log, request builders and
//! repository set-ups. The core's git in test builds reads no global or system config
//! (see `gg_core::testutil::empty_git_config`).
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gg_core::dto::{
    DangerKind, MergeTarget, MergeTargetKind, OpOutcome, OpRequest, RepoSnapshot, ResetMode,
};
use gg_core::git::runner::{CmdKind, CommandLog, CommandRecord, GitRunner};
use gg_core::repo::RepoManager;
use gg_core::testutil::{RecordingSink, TestRepo};
use serde_json::Value;

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
        let snap = self.m.open(path).await.expect("open");
        let id = snap.repo_id;
        tokio::time::timeout(TIMEOUT, self.m.wait_loaded(id))
            .await
            .expect("load finishes");
        tokio::time::timeout(TIMEOUT, self.m.wait_status(id))
            .await
            .expect("status read");
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

    /// Runs `request` with the dangers `confirmed`; panics if it errs or takes too long.
    pub async fn run_confirmed(
        &self,
        id: u32,
        request: OpRequest,
        confirmed: &[DangerKind],
    ) -> OpOutcome {
        tokio::time::timeout(TIMEOUT, self.m.run_op(id, request, confirmed))
            .await
            .expect("the operation ends")
            .expect("run_op")
    }

    /// Runs `request` with nothing confirmed.
    pub async fn run(&self, id: u32, request: OpRequest) -> OpOutcome {
        self.run_confirmed(id, request, &[]).await
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

pub fn create_branch(name: &str, at: &str) -> OpRequest {
    OpRequest::CreateBranch {
        name: name.to_owned(),
        at: at.to_owned(),
        checkout: false,
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
    }
}

pub fn delete_branch(branch: &str) -> OpRequest {
    OpRequest::DeleteBranch {
        branch: branch.to_owned(),
    }
}

/// The branch HEAD is on, or `None` when detached.
pub fn current_branch(repo: &TestRepo) -> Option<String> {
    repo.git_succeeds(&["symbolic-ref", "-q", "HEAD"])
        .then(|| repo.git(&["symbolic-ref", "--short", "HEAD"]))
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
    }
}

pub fn merge_branch(name: &str) -> OpRequest {
    merge(MergeTargetKind::Branch, name, true, false, false)
}

pub fn rebase(onto: &str) -> OpRequest {
    OpRequest::Rebase {
        onto: onto.to_owned(),
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
