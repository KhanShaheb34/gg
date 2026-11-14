//! Counts a worktree's changes from `git status --porcelain=v2`.

use std::path::Path;

use crate::dto::{MAX_CONFLICTED_PATHS, WorktreeStatus};
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};

/// Arguments of the status command.
pub(crate) const STATUS_ARGS: [&str; 5] = [
    "status",
    "--porcelain=v2",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=all",
];

/// An unmerged (conflicted) path and git's two-letter code for it (`UU` both modified, `AA`
/// both added, `DU` deleted by us, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmerged {
    pub path: String,
    pub code: [u8; 2],
}

impl Unmerged {
    /// Both sides have the file as text git merged line by line: it may hold conflict
    /// markers. Otherwise git kept one side (or none) and left no markers.
    pub fn both_sides_merged(&self) -> bool {
        matches!(&self.code, b"UU" | b"AA")
    }
}

/// The paths of `unmerged`.
pub fn paths(unmerged: &[Unmerged]) -> Vec<String> {
    unmerged.iter().map(|u| u.path.clone()).collect()
}

/// Change counts for the worktree at `worktree` (untracked files counted one by one).
/// Submodules are ignored entirely, so git never looks inside them or reads their config.
pub async fn read_status(r: &GitRunner, worktree: &Path) -> Result<WorktreeStatus> {
    Ok(read_status_and_conflicts(r, worktree).await?.0)
}

/// Like [`read_status`], with the conflicted (unmerged) paths, relative to the worktree,
/// sorted.
pub async fn read_status_and_conflicts(
    r: &GitRunner,
    worktree: &Path,
) -> Result<(WorktreeStatus, Vec<Unmerged>)> {
    let out = r.run(worktree, &STATUS_ARGS, CmdKind::Read).await?;
    Ok(parse_status(&out.stdout))
}

/// The conflicted paths of the worktree at `worktree`, sorted, without looking at untracked
/// files (cheaper than [`read_status`] on a big tree).
pub(crate) async fn read_conflicted(r: &GitRunner, worktree: &Path) -> Result<Vec<Unmerged>> {
    let args = [
        "status",
        "--porcelain=v2",
        "-z",
        "--untracked-files=no",
        "--ignore-submodules=all",
    ];
    let out = r.run(worktree, &args, CmdKind::Read).await?;
    Ok(parse_status(&out.stdout).1)
}

/// Counts porcelain v2 entries: `1` (changed), `2` (renamed or copied; followed by an extra
/// NUL-terminated original path), `u` (unmerged: changed and conflicted), `?` (untracked).
/// Also returns the unmerged entries' paths, sorted (all of them; the status names the first
/// [`MAX_CONFLICTED_PATHS`]).
fn parse_status(out: &[u8]) -> (WorktreeStatus, Vec<Unmerged>) {
    let mut s = WorktreeStatus::default();
    let mut conflicted = Vec::new();
    let mut fields = out.split(|&b| b == 0);
    while let Some(entry) = fields.next() {
        if entry.starts_with(b"1 ") {
            s.changed = s.changed.saturating_add(1);
        } else if entry.starts_with(b"2 ") {
            s.changed = s.changed.saturating_add(1);
            fields.next(); // original path
        } else if entry.starts_with(b"u ") {
            s.changed = s.changed.saturating_add(1);
            s.conflicted = s.conflicted.saturating_add(1);
            // `u XY sub m1 m2 m3 mW h1 h2 h3 <path>`: the path is the rest, spaces and all.
            let mut parts = entry.splitn(11, |&b| b == b' ');
            let code = parts.nth(1).unwrap_or_default();
            if let (Some(path), &[x, y]) = (parts.nth(8), code) {
                conflicted.push(Unmerged {
                    path: String::from_utf8_lossy(path).into_owned(),
                    code: [x, y],
                });
            }
        } else if entry.starts_with(b"? ") {
            s.untracked = s.untracked.saturating_add(1);
        }
    }
    conflicted.sort_by(|a, b| a.path.cmp(&b.path));
    s.conflicted_paths = conflicted
        .iter()
        .take(MAX_CONFLICTED_PATHS)
        .map(|u| u.path.clone())
        .collect();
    (s, conflicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDINARY: &str = "1 .M N... 100644 100644 100644 aaaa aaaa file one.txt\0";
    const RENAME: &str =
        "2 R. N... 100644 100644 100644 aaaa aaaa R100 new name.txt\0old name.txt\0";
    const UNMERGED: &str = "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc c.txt\0";

    fn counts(changed: u32, untracked: u32, conflicted: u32) -> WorktreeStatus {
        WorktreeStatus {
            changed,
            untracked,
            conflicted,
            conflicted_paths: Vec::new(),
        }
    }

    /// The counts of `status`, its paths left out.
    fn only_counts(status: WorktreeStatus) -> WorktreeStatus {
        WorktreeStatus {
            conflicted_paths: Vec::new(),
            ..status
        }
    }

    #[test]
    fn counts_each_entry_kind() {
        let out = [
            ORDINARY,
            RENAME,
            UNMERGED,
            "? new 1.txt\0",
            "? dir/ü 2.txt\0",
            "! ignored\0",
        ]
        .concat();
        let (status, _) = parse_status(out.as_bytes());
        assert_eq!(status.conflicted_paths, ["c.txt"]);
        assert_eq!(only_counts(status), counts(3, 2, 1));
    }

    #[test]
    fn conflicted_paths_are_kept_whole_and_sorted() {
        let out = [
            "u UU N... 100644 100644 100644 100644 a b c z last.txt\0",
            UNMERGED,
            "u DU N... 100644 000000 100644 100644 a 0 c dir/a  b\u{fc}.txt\0",
            "u UU N... 100644 100644 100644 100644 a b c\0",
            ORDINARY,
        ]
        .concat();
        let (status, conflicted) = parse_status(out.as_bytes());
        assert_eq!(status.conflicted, 4);
        assert_eq!(
            paths(&conflicted),
            ["c.txt", "dir/a  b\u{fc}.txt", "z last.txt"]
        );
        assert_eq!(status.conflicted_paths, paths(&conflicted));
        assert_eq!(&conflicted[1].code, b"DU");
        assert!(conflicted[0].both_sides_merged() && !conflicted[1].both_sides_merged());
    }

    #[test]
    fn rename_original_path_is_not_an_entry() {
        // The original path of a rename looks like anything, even like an entry header.
        let out = "2 R. N... 100644 100644 100644 aaaa aaaa R100 new\0? not untracked\0";
        assert_eq!(parse_status(out.as_bytes()).0, counts(1, 0, 0));
        let out = "2 R. N... 100644 100644 100644 aaaa aaaa R100 new\0u x\0? real\0";
        assert_eq!(parse_status(out.as_bytes()).0, counts(1, 1, 0));
    }

    #[test]
    fn survives_hostile_input() {
        assert_eq!(parse_status(b"").0, counts(0, 0, 0));
        assert_eq!(parse_status(b"\0\0\0").0, counts(0, 0, 0));
        assert_eq!(parse_status(b"# branch.oid abc\0").0, counts(0, 0, 0));
        assert_eq!(parse_status(b"\xFF\xFE\0zzz\0").0, counts(0, 0, 0));
        // Truncated: a rename entry with no original path field, an entry with no NUL.
        assert_eq!(
            parse_status(b"2 R. N... x y z a b R100 new\0").0,
            counts(1, 0, 0)
        );
        assert_eq!(parse_status(b"? partial").0, counts(0, 1, 0));
    }
}
