//! Reads what git is stopped in: a merge, rebase, cherry-pick or revert, started by the app
//! or outside it. The kind comes from the files git keeps in the worktree's git dir while
//! it is stopped (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `rebase-merge/`,
//! `rebase-apply/`, and `sequencer/` for a cherry-pick or revert of several commits whose
//! current one was committed by hand), found with `rev-parse --git-path` so that a linked
//! worktree's own files are the ones read. `git am` is not reported: the app cannot continue
//! or abort it.

use std::fs;
use std::path::{Path, PathBuf};

use crate::dto::{InProgress, InProgressKind};
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};
use crate::git::status::read_conflicted;

/// The state files asked for, in the order `rev-parse` prints their paths.
const STATE_FILES: [&str; 7] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge",
    "rebase-apply",
    "MERGE_MSG",
    "sequencer",
];

/// Where git keeps its state files for one worktree.
pub(crate) struct StateFiles {
    merge_head: PathBuf,
    cherry_pick_head: PathBuf,
    revert_head: PathBuf,
    rebase_merge: PathBuf,
    rebase_apply: PathBuf,
    merge_msg: PathBuf,
    sequencer: PathBuf,
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
            self.sequence_kind()
        }
    }

    /// The kind of the sequencer's current command, for a cherry-pick or revert of several
    /// commits: its `todo` lists `pick <commit> <subject>` (or `revert …`) lines, the
    /// current one first. `None` without a sequence (or a command of another kind).
    fn sequence_kind(&self) -> Option<InProgressKind> {
        let bytes = fs::read(self.sequencer.join("todo")).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))?;
        match line.split(' ').next()? {
            "pick" | "p" => Some(InProgressKind::CherryPick),
            "revert" => Some(InProgressKind::Revert),
            _ => None,
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
            sequencer,
        ],
    ) = <[PathBuf; 7]>::try_from(paths)
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
        sequencer,
    }))
}

/// Most paths given to one `git diff --check`, so the command line stays short.
const CHECK_CHUNK: usize = 100;

/// What `git diff --check` says of a line left from a conflict.
const LEFTOVER: &str = ": leftover conflict marker";

/// Those of `paths` (conflicted, relative to the worktree at `worktree`) whose working-tree
/// file still has conflict markers, as `git diff --check` finds them: not resolved yet.
/// When git's report cannot be read for a group of paths, each of them counts as unresolved.
pub(crate) async fn with_conflict_markers(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[String],
) -> Result<Vec<String>> {
    let mut unresolved = Vec::new();
    for chunk in paths.chunks(CHECK_CHUNK) {
        // Literal: a path like `[ab].txt` must not match other files as a glob.
        let mut args = vec![
            "--literal-pathspecs",
            "diff",
            "--check",
            "--no-ext-diff",
            "--",
        ];
        args.extend(chunk.iter().map(String::as_str));
        let out = runner.run_unchecked(worktree, &args, CmdKind::Read).await?;
        // 0: nothing found; 2: problems found (listed); anything else: git failed.
        let found = match out.code {
            0 => Some(Vec::new()),
            2 => marked_paths(&out.text(), chunk),
            _ => None,
        }
        .unwrap_or_else(|| chunk.to_vec());
        unresolved.extend(found);
    }
    unresolved.sort();
    unresolved.dedup();
    Ok(unresolved)
}

/// The paths of `paths` that `report` (`git diff --check`'s) says have a leftover conflict
/// marker (`<path>:<line>: leftover conflict marker`). `None` if a marker is reported for a
/// file that is not recognised among `paths` (a quoted name, say).
fn marked_paths(report: &str, paths: &[String]) -> Option<Vec<String>> {
    let mut marked = Vec::new();
    for line in report.lines().filter(|l| l.ends_with(LEFTOVER)) {
        let head = &line[..line.len() - LEFTOVER.len()];
        // The longest path that is followed by `:<digits>`.
        let path = paths
            .iter()
            .filter(|p| {
                head.strip_prefix(p.as_str())
                    .and_then(|rest| rest.strip_prefix(':'))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .max_by_key(|p| p.len())?;
        marked.push(path.clone());
    }
    Some(marked)
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
/// conflicted paths and those not resolved yet, a rebase's step, and what is being applied.
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
/// the subject of the commit being applied; none for a cherry-pick or revert sequence whose
/// current commit was committed by hand.
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
    let picking = files.cherry_pick_head.exists() || files.revert_head.exists();
    let sequence_only =
        matches!(kind, InProgressKind::CherryPick | InProgressKind::Revert) && !picking;
    let subject = match files.merge_msg_subject() {
        // Only the sequencer is left: the commit it was on was committed by hand, and its
        // current line names that commit, not one still to apply. Unknown.
        _ if sequence_only => None,
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
    let unresolved = with_conflict_markers(runner, worktree, &conflicted).await?;
    Ok(Some(InProgress {
        kind,
        conflicted,
        unresolved,
        step,
        total,
        subject,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_reports_are_read_against_the_paths_asked_about() {
        let paths = [
            "a.txt".to_owned(),
            "a.txt:2".to_owned(),
            "b c.txt".to_owned(),
        ];
        let report = "a.txt:2:3: leftover conflict marker\n\
                      b c.txt:4: trailing whitespace.\n\
                      +x   \n\
                      b c.txt:9: leftover conflict marker\n";
        assert_eq!(
            marked_paths(report, &paths),
            Some(vec!["a.txt:2".to_owned(), "b c.txt".to_owned()])
        );
        assert_eq!(marked_paths("", &paths), Some(vec![]));
        // A name git quoted is not one asked about: nothing can be told.
        let quoted = "\"q\\\"x.txt\":1: leftover conflict marker\n";
        assert_eq!(marked_paths(quoted, &paths), None);
    }
}
