//! Reads what git is stopped in: a merge, rebase, cherry-pick or revert, started by the app
//! or outside it. The kind comes from the files git keeps in the worktree's git dir while
//! it is stopped (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `rebase-merge/`,
//! `rebase-apply/`, and `sequencer/` for a cherry-pick or revert of several commits whose
//! current one was committed by hand), found with `rev-parse --git-path` so that a linked
//! worktree's own files are the ones read. `git am` is not reported: the app cannot continue
//! or abort it.

use std::fs;
use std::path::{Path, PathBuf};

use std::collections::HashMap;

use crate::dto::{ConflictFile, ConflictState, InProgress, InProgressKind, MAX_CONFLICTED_PATHS};
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};
use crate::git::status::{Unmerged, read_conflicted};

/// The state files asked for, in the order `rev-parse` prints their paths.
const STATE_FILES: [&str; 8] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge",
    "rebase-apply",
    "MERGE_MSG",
    "sequencer",
    "REBASE_HEAD",
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
    rebase_head: PathBuf,
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

    /// The commit that comes in to what is stopped (`kind`): `MERGE_HEAD`'s first, the
    /// rebase's `onto`, `CHERRY_PICK_HEAD` or `REVERT_HEAD`. `None` when git keeps none (a
    /// sequence whose current commit was committed by hand) or it cannot be read.
    fn incoming_commit(&self, kind: InProgressKind) -> Option<String> {
        let path = match kind {
            InProgressKind::Merge => self.merge_head.clone(),
            InProgressKind::Rebase if self.rebase_merge.is_dir() => self.rebase_merge.join("onto"),
            InProgressKind::Rebase => self.rebase_apply.join("onto"),
            InProgressKind::CherryPick => self.cherry_pick_head.clone(),
            InProgressKind::Revert => self.revert_head.clone(),
        };
        full_hash_in(&path)
    }

    /// The commit a rebase, cherry-pick or revert (`kind`) is stopped on, not committed yet:
    /// `REBASE_HEAD`, `CHERRY_PICK_HEAD` or `REVERT_HEAD`. `None` for a merge, when git keeps
    /// none (a sequence whose current commit was committed by hand), or when a rebase stopped
    /// after committing it (`edit`: `rebase-merge/amend`), which is not one to skip.
    pub(crate) fn stopped_commit(&self, kind: InProgressKind) -> Option<String> {
        match kind {
            InProgressKind::Merge => None,
            InProgressKind::Rebase if self.rebase_merge.join("amend").exists() => None,
            InProgressKind::Rebase => full_hash_in(&self.rebase_head),
            InProgressKind::CherryPick => full_hash_in(&self.cherry_pick_head),
            InProgressKind::Revert => full_hash_in(&self.revert_head),
        }
    }

    /// The branch a rebase works on (`head-name`, "refs/heads/<branch>"), if it is one.
    pub(crate) fn rebase_head_name(&self) -> Option<String> {
        let dir = if self.rebase_merge.is_dir() {
            &self.rebase_merge
        } else {
            &self.rebase_apply
        };
        let bytes = fs::read(dir.join("head-name")).ok()?;
        String::from_utf8_lossy(&bytes)
            .trim()
            .strip_prefix("refs/heads/")
            .map(str::to_owned)
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

/// The full hash on the first line of the file at `path`, if it has one.
fn full_hash_in(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let hash = String::from_utf8_lossy(&bytes)
        .lines()
        .next()?
        .trim()
        .to_owned();
    let hex = hash.bytes().all(|b| b.is_ascii_hexdigit());
    (hex && (hash.len() == 40 || hash.len() == 64)).then_some(hash)
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
            rebase_head,
        ],
    ) = <[PathBuf; 8]>::try_from(paths)
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
        rebase_head,
    }))
}

/// Most paths given to one `git check-attr`, so the command line stays short.
const ATTR_CHUNK: usize = 100;

/// Largest file read to look for conflict markers; a larger one is undecided.
const MAX_JUDGED_BYTES: u64 = 8 * 1024 * 1024;

/// git's default length of a conflict marker's run.
const DEFAULT_MARKER_SIZE: usize = 7;

/// How far each of `unmerged` (relative to the worktree at `worktree`) is resolved, in the
/// same order. Only a file both sides changed as text can hold conflict markers: it has
/// markers while a line starts with exactly the file's `conflict-marker-size` (default 7)
/// of `<` or `>` (then a space or the line's end); `=======` alone is no marker (a Markdown
/// underline looks the same). A file git kept one side of (binary, deleted or added on one
/// side, a `merge` attribute that does not merge text), or one that cannot be read, is over
/// 8 MiB, lies past [`JUDGE_BUDGET`] bytes of reading, or is reached through a folder that
/// leads out of the worktree, is undecided. A file unchanged since it was last judged is not
/// read again. The files are read, never written, off the async workers.
pub(crate) async fn judge(
    runner: &GitRunner,
    worktree: &Path,
    unmerged: &[Unmerged],
) -> Result<Vec<ConflictFile>> {
    let text: Vec<&str> = unmerged
        .iter()
        .filter(|u| u.both_sides_merged())
        .map(|u| u.path.as_str())
        .collect();
    let attrs = attributes(runner, worktree, &text).await?;
    // What to read: the path and its marker size, for files that can hold markers.
    let to_read: Vec<(String, usize)> = unmerged
        .iter()
        .filter_map(|u| match attrs.get(u.path.as_str()) {
            Some(attr) if u.both_sides_merged() && attr.merges_text => {
                Some((u.path.clone(), attr.marker_size))
            }
            _ => None,
        })
        .collect();
    let root = worktree.to_path_buf();
    let read = tokio::task::spawn_blocking(move || {
        let files: Vec<(&str, usize)> = to_read.iter().map(|(p, n)| (p.as_str(), *n)).collect();
        let states = judge_files(&root, &files, JUDGE_BUDGET, &JUDGED);
        to_read
            .into_iter()
            .map(|(p, _)| p)
            .zip(states)
            .collect::<HashMap<String, ConflictState>>()
    })
    .await
    .unwrap_or_default();
    Ok(unmerged
        .iter()
        .map(|u| ConflictFile {
            path: u.path.clone(),
            state: read
                .get(&u.path)
                .copied()
                .unwrap_or(ConflictState::Undecided),
        })
        .collect())
}

/// Most bytes one judging reads (files taken from [`JUDGED`] cost nothing): it runs on every
/// status read while git is stopped.
const JUDGE_BUDGET: u64 = 32 * 1024 * 1024;

/// What a file was judged, by its path, while it stays the same file of the same size and
/// time (and marker size).
#[derive(Default)]
struct JudgeCache(std::sync::Mutex<HashMap<PathBuf, (FileKey, ConflictState)>>);

/// Most files [`JUDGED`] remembers; past that it starts over.
const MAX_JUDGED_FILES: usize = 4096;

/// Files judged so far, for every repository.
static JUDGED: std::sync::LazyLock<JudgeCache> = std::sync::LazyLock::new(JudgeCache::default);

/// What tells one version of a file from another without reading it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileKey {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    ino: u64,
    marker_size: usize,
}

/// The state of each of `files` (paths relative to `root`, with their marker sizes), reading
/// at most `budget` bytes in all.
fn judge_files(
    root: &Path,
    files: &[(&str, usize)],
    budget: u64,
    cache: &JudgeCache,
) -> Vec<ConflictState> {
    let root = dunce::canonicalize(root).ok();
    let mut left = budget;
    files
        .iter()
        .map(|&(rel, size)| {
            let Some(root) = &root else {
                return ConflictState::Undecided;
            };
            let Some(path) = inside(root, rel) else {
                return ConflictState::Undecided;
            };
            let Ok(meta) = fs::symlink_metadata(&path) else {
                return ConflictState::Undecided; // gone: deleted to resolve it, or unreadable
            };
            if !meta.is_file() || meta.len() > MAX_JUDGED_BYTES {
                return ConflictState::Undecided;
            }
            let key = FileKey {
                len: meta.len(),
                modified: meta.modified().ok(),
                #[cfg(unix)]
                ino: std::os::unix::fs::MetadataExt::ino(&meta),
                marker_size: size,
            };
            let mut known = cache.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((k, state)) = known.get(&path)
                && *k == key
            {
                return *state;
            }
            drop(known);
            if meta.len() > left {
                return ConflictState::Undecided;
            }
            left -= meta.len();
            let state = match fs::read(&path) {
                Ok(bytes) if bytes.contains(&0) => ConflictState::Undecided,
                Ok(bytes) if has_markers(&bytes, size) => ConflictState::Markers,
                Ok(_) => ConflictState::Resolved,
                Err(_) => return ConflictState::Undecided,
            };
            known = cache.0.lock().unwrap_or_else(|e| e.into_inner());
            if known.len() >= MAX_JUDGED_FILES {
                known.clear();
            }
            known.insert(path, (key, state));
            state
        })
        .collect()
}

/// `rel` under `root` (canonical) with its folder resolved, if that folder is still inside
/// `root`: a folder replaced by a link to elsewhere is not read through.
fn inside(root: &Path, rel: &str) -> Option<PathBuf> {
    let path = root.join(rel);
    let name = path.file_name()?.to_owned();
    let parent = dunce::canonicalize(path.parent()?).ok()?;
    parent.starts_with(root).then(|| parent.join(name))
}

/// What a file's attributes say about its conflicts.
struct ConflictAttrs {
    /// git merges it as text (its `merge` attribute is not unset or `binary`).
    merges_text: bool,
    marker_size: usize,
}

/// The conflict attributes of each of `paths` (`git check-attr -z`), by path.
async fn attributes<'a>(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[&'a str],
) -> Result<HashMap<&'a str, ConflictAttrs>> {
    let mut found = HashMap::new();
    for chunk in paths.chunks(ATTR_CHUNK) {
        let mut args = vec!["check-attr", "-z", "merge", "conflict-marker-size", "--"];
        args.extend(chunk.iter().copied());
        let out = runner.run_unchecked(worktree, &args, CmdKind::Read).await?;
        if out.code != 0 {
            continue; // unknown: each stays undecided
        }
        let mut attrs: HashMap<&str, ConflictAttrs> = chunk
            .iter()
            .map(|p| {
                let attr = ConflictAttrs {
                    merges_text: true,
                    marker_size: DEFAULT_MARKER_SIZE,
                };
                (*p, attr)
            })
            .collect();
        // `<path> NUL <attribute> NUL <value> NUL`, for each path and attribute asked for.
        let text = out.text();
        let fields: Vec<&str> = text.split('\0').collect();
        for [path, name, value] in fields.as_chunks::<3>().0 {
            let Some(attr) = attrs.get_mut(*path) else {
                continue;
            };
            match *name {
                "merge" => attr.merges_text = !matches!(*value, "unset" | "binary"),
                "conflict-marker-size" => {
                    if let Ok(n @ 1..=1024) = value.parse::<usize>() {
                        attr.marker_size = n;
                    }
                }
                _ => {}
            }
        }
        found.extend(attrs);
    }
    Ok(found)
}

/// Whether a line of `text` is a conflict's opening or closing marker: exactly `size` of `<`
/// or `>` at its start, then a space, a tab or the line's end.
fn has_markers(text: &[u8], size: usize) -> bool {
    text.split(|&b| b == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(&first) = line.first() else {
            return false;
        };
        if first != b'<' && first != b'>' {
            return false;
        }
        let run = line.iter().take_while(|&&b| b == first).count();
        run == size && matches!(line.get(size), None | Some(b' ' | b'\t'))
    })
}

/// The commit coming in to what git is stopped in at `worktree` (see
/// [`StateFiles::incoming_commit`]), if anything is stopped.
pub(crate) async fn incoming_commit(runner: &GitRunner, worktree: &Path) -> Result<Option<String>> {
    Ok(state_files(runner, worktree)
        .await?
        .and_then(|files| files.kind().and_then(|kind| files.incoming_commit(kind))))
}

/// How the app named the commit coming in to what it started (a merge's target, a rebase's
/// `onto`): that commit and the name.
pub type KnownIncoming = (String, String);

/// A name for commit `hash`: the first local branch at it, else remote branch (not a
/// remote's `HEAD`), else tag (annotated ones too), else its first 8 characters.
async fn name_of(runner: &GitRunner, worktree: &Path, hash: &str) -> Result<String> {
    let points_at = format!("--points-at={hash}");
    let out = runner
        .run_unchecked(
            worktree,
            &[
                "for-each-ref",
                &points_at,
                "--format=%(refname)",
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ],
            CmdKind::Read,
        )
        .await?;
    let text = if out.code == 0 {
        out.text()
    } else {
        String::new()
    };
    let refs: Vec<&str> = text.lines().collect();
    let first = |prefix: &str| {
        refs.iter()
            .filter_map(|r| r.strip_prefix(prefix))
            .find(|name| !(prefix == "refs/remotes/" && name.ends_with("/HEAD")))
            .map(str::to_owned)
    };
    Ok(first("refs/heads/")
        .or_else(|| first("refs/remotes/"))
        .or_else(|| first("refs/tags/"))
        .unwrap_or_else(|| hash.get(..8).unwrap_or(hash).to_owned()))
}

/// Whether the index and the tracked files of the worktree at `worktree` are as HEAD has
/// them (`diff --quiet HEAD`, which writes nothing with optional locks off).
async fn same_as_head(runner: &GitRunner, worktree: &Path) -> Result<bool> {
    let out = runner
        .run_unchecked(worktree, &["diff", "--quiet", "HEAD", "--"], CmdKind::Read)
        .await?;
    Ok(out.code == 0)
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
    describe(runner, worktree, &files, conflicted, None).await
}

/// Like [`read_in_progress`], with the conflicted paths already read (by the status read),
/// and what the app knows of the commit coming in.
pub(crate) async fn read_in_progress_with(
    runner: &GitRunner,
    worktree: &Path,
    conflicted: Vec<Unmerged>,
    known: Option<&KnownIncoming>,
) -> Result<Option<InProgress>> {
    match state_files(runner, worktree).await? {
        Some(files) => describe(runner, worktree, &files, conflicted, known).await,
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
    conflicted: Vec<Unmerged>,
    known: Option<&KnownIncoming>,
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
    // The files named (the first 200) are judged; the rest only counted.
    let file_count = u32::try_from(conflicted.len()).unwrap_or(u32::MAX);
    let named = &conflicted[..conflicted.len().min(MAX_CONFLICTED_PATHS)];
    let judged = judge(runner, worktree, named).await?;
    let incoming = match files.incoming_commit(kind) {
        None => None,
        Some(hash) => Some(match kind {
            InProgressKind::Merge | InProgressKind::Rebase => match known {
                Some((commit, name)) if *commit == hash => name.clone(),
                _ => name_of(runner, worktree, &hash).await?,
            },
            _ => hash[..8].to_owned(),
        }),
    };
    let rebasing = match kind {
        InProgressKind::Rebase => files.rebase_head_name(),
        _ => None,
    };
    let commit = files.stopped_commit(kind);
    let empty = commit.is_some() && conflicted.is_empty() && same_as_head(runner, worktree).await?;
    Ok(Some(InProgress {
        kind,
        files: judged,
        file_count,
        step,
        total,
        subject,
        incoming,
        rebasing,
        commit,
        empty,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap().join("wt");
        fs::create_dir_all(&root).unwrap();
        (dir, root)
    }

    #[test]
    fn files_past_the_read_budget_are_undecided() {
        let (_dir, root) = worktree();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(root.join(name), "resolved text\n").unwrap();
        }
        let files: Vec<(&str, usize)> = vec![("a.txt", 7), ("b.txt", 7), ("c.txt", 7)];
        let cache = JudgeCache::default();
        // Room for two of the three 14-byte files.
        let states = judge_files(&root, &files, 30, &cache);
        assert_eq!(
            states,
            [
                ConflictState::Resolved,
                ConflictState::Resolved,
                ConflictState::Undecided
            ]
        );
    }

    #[test]
    fn a_file_unchanged_since_the_last_look_is_not_read_again() {
        let (_dir, root) = worktree();
        let path = root.join("a.txt");
        fs::write(&path, "<<<<<<< ours\n").unwrap();
        let files = [("a.txt", 7)];
        let cache = JudgeCache::default();
        assert_eq!(
            judge_files(&root, &files, 1 << 20, &cache),
            [ConflictState::Markers]
        );
        // Same size, same time, same file: taken from the cache (the budget cannot read it).
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, "resolved!!!!\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(
            judge_files(&root, &files, 0, &cache),
            [ConflictState::Markers]
        );
        // Changed (its time moves on): read again.
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified + std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            judge_files(&root, &files, 1 << 20, &cache),
            [ConflictState::Resolved]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_reached_through_a_linked_folder_outside_the_worktree_is_not_read() {
        let (dir, root) = worktree();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("a.txt"), "no markers here\n").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("src")).unwrap();
        fs::create_dir_all(root.join("in")).unwrap();
        fs::write(root.join("in/b.txt"), "no markers here\n").unwrap();
        let files = [("src/a.txt", 7), ("in/b.txt", 7)];
        assert_eq!(
            judge_files(&root, &files, 1 << 20, &JudgeCache::default()),
            [ConflictState::Undecided, ConflictState::Resolved]
        );
    }

    #[test]
    fn markers_are_whole_runs_at_a_line_start() {
        assert!(has_markers(b"a\n<<<<<<< HEAD\nb\n", 7));
        assert!(has_markers(b">>>>>>> feature\r\n", 7));
        assert!(
            has_markers(b"x\n<<<<<<<", 7),
            "at the end, without a newline"
        );
        assert!(has_markers(b"<<<<<<<\tours\n", 7));
        assert!(!has_markers(b"Title\n=======\n", 7), "an underline");
        assert!(!has_markers(b"||||||| base\n", 7));
        assert!(!has_markers(b"<<<<<<<< eight\n", 7));
        assert!(!has_markers(b" <<<<<<< indented\n", 7));
        assert!(!has_markers(b"<<<<<<<x\n", 7));
        assert!(!has_markers(b"", 7));
        assert!(has_markers(b"<<<<<<<<<< x\n", 10));
        assert!(!has_markers(b"<<<<<<< x\n", 10));
    }
}
