//! Reads what git is stopped in: a merge, rebase, cherry-pick or revert, started by the app
//! or outside it. The kind comes from the files git keeps in the worktree's git dir while
//! it is stopped (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `rebase-merge/`,
//! `rebase-apply/`), found with `rev-parse --git-path` so that a linked worktree's own
//! files are the ones read.

use std::fs;
use std::path::{Path, PathBuf};

use crate::dto::{InProgress, InProgressKind};
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};
use crate::git::status::read_conflicted;

/// The state files asked for, in the order `rev-parse` prints their paths.
const STATE_FILES: [&str; 6] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge",
    "rebase-apply",
    "MERGE_MSG",
];

/// Where git keeps its state files for one worktree.
pub(crate) struct StateFiles {
    merge_head: PathBuf,
    cherry_pick_head: PathBuf,
    revert_head: PathBuf,
    rebase_merge: PathBuf,
    rebase_apply: PathBuf,
    merge_msg: PathBuf,
}

impl StateFiles {
    /// What git is stopped in: a rebase wins when several states are there (a merge stopped
    /// inside a rebase is part of the rebase). A `rebase-apply` dir of `git am` is not a
    /// rebase and is not reported.
    pub(crate) fn kind(&self) -> Option<InProgressKind> {
        let applying_mail = self.rebase_apply.join("applying").exists();
        if self.rebase_merge.is_dir() || (self.rebase_apply.is_dir() && !applying_mail) {
            Some(InProgressKind::Rebase)
        } else if self.merge_head.exists() {
            Some(InProgressKind::Merge)
        } else if self.cherry_pick_head.exists() {
            Some(InProgressKind::CherryPick)
        } else if self.revert_head.exists() {
            Some(InProgressKind::Revert)
        } else {
            None
        }
    }

    /// The rebase's step and number of steps, from `rebase-merge/msgnum` and `end`, or
    /// `rebase-apply/next` and `last`.
    fn rebase_step(&self) -> (Option<u32>, Option<u32>) {
        let number = |dir: &Path, name: &str| {
            fs::read_to_string(dir.join(name))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
        };
        if self.rebase_merge.is_dir() {
            (
                number(&self.rebase_merge, "msgnum"),
                number(&self.rebase_merge, "end"),
            )
        } else {
            (
                number(&self.rebase_apply, "next"),
                number(&self.rebase_apply, "last"),
            )
        }
    }

    /// The first line of `MERGE_MSG` that is neither blank nor a comment, if any.
    fn merge_msg_subject(&self) -> Option<String> {
        let bytes = fs::read(&self.merge_msg).ok()?;
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_owned)
    }
}

/// Where the state files of the worktree at `worktree` are (whether they exist or not).
/// `None` if git cannot say (not a repository any more).
pub(crate) async fn state_files(runner: &GitRunner, worktree: &Path) -> Result<Option<StateFiles>> {
    let mut args = vec!["rev-parse"];
    for name in STATE_FILES {
        args.extend(["--git-path", name]);
    }
    let out = runner.run_unchecked(worktree, &args, CmdKind::Read).await?;
    if out.code != 0 {
        return Ok(None);
    }
    let text = out.text();
    // Relative paths are relative to where git ran; an absolute one replaces `worktree`.
    let paths: Vec<PathBuf> = text.lines().map(|l| worktree.join(l)).collect();
    let Ok(
        [
            merge_head,
            cherry_pick_head,
            revert_head,
            rebase_merge,
            rebase_apply,
            merge_msg,
        ],
    ) = <[PathBuf; 6]>::try_from(paths)
    else {
        return Ok(None);
    };
    Ok(Some(StateFiles {
        merge_head,
        cherry_pick_head,
        revert_head,
        rebase_merge,
        rebase_apply,
        merge_msg,
    }))
}

/// What git is stopped in, in the worktree at `worktree`, if anything (see
/// [`StateFiles::kind`]).
pub(crate) async fn read_kind(
    runner: &GitRunner,
    worktree: &Path,
) -> Result<Option<InProgressKind>> {
    Ok(state_files(runner, worktree)
        .await?
        .and_then(|files| files.kind()))
}

/// What git is stopped in, in the worktree at `worktree`, if anything: its kind, the
/// conflicted paths, a rebase's step, and what is being applied.
pub async fn read_in_progress(runner: &GitRunner, worktree: &Path) -> Result<Option<InProgress>> {
    let Some(files) = state_files(runner, worktree).await? else {
        return Ok(None);
    };
    if files.kind().is_none() {
        return Ok(None);
    }
    let conflicted = read_conflicted(runner, worktree).await?;
    describe(runner, worktree, &files, conflicted).await
}

/// Like [`read_in_progress`], with the conflicted paths already read (by the status read).
pub(crate) async fn read_in_progress_with(
    runner: &GitRunner,
    worktree: &Path,
    conflicted: Vec<String>,
) -> Result<Option<InProgress>> {
    match state_files(runner, worktree).await? {
        Some(files) => describe(runner, worktree, &files, conflicted).await,
        None => Ok(None),
    }
}

/// The in-progress state `files` show, with `conflicted` as its conflicted paths. The
/// subject is the first line of `MERGE_MSG` (the merge's or the commit's message), else
/// the subject of the commit being applied.
async fn describe(
    runner: &GitRunner,
    worktree: &Path,
    files: &StateFiles,
    conflicted: Vec<String>,
) -> Result<Option<InProgress>> {
    let Some(kind) = files.kind() else {
        return Ok(None);
    };
    let (step, total) = match kind {
        InProgressKind::Rebase => files.rebase_step(),
        _ => (None, None),
    };
    let subject = match files.merge_msg_subject() {
        Some(subject) => Some(subject),
        None => {
            let head = match kind {
                InProgressKind::Merge => "MERGE_HEAD",
                InProgressKind::Rebase => "REBASE_HEAD",
                InProgressKind::CherryPick => "CHERRY_PICK_HEAD",
                InProgressKind::Revert => "REVERT_HEAD",
            };
            let out = runner
                .run_unchecked(
                    worktree,
                    &["log", "-1", "--no-walk", "--format=%s", head, "--"],
                    CmdKind::Read,
                )
                .await?;
            Some(out.text().trim().to_owned()).filter(|s| out.code == 0 && !s.is_empty())
        }
    };
    Ok(Some(InProgress {
        kind,
        conflicted,
        step,
        total,
        subject,
    }))
}
