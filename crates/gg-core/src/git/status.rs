//! Reads a worktree's changes from `git status --porcelain=v2`: counts, and the files.

use std::path::Path;

use crate::dto::{
    EntryStatus, FileEntry, MAX_CONFLICTED_PATHS, MAX_LISTED_FILES, WorktreeFiles, WorktreeStatus,
};
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};

/// Why a file whose name is not valid UTF-8 cannot be ticked.
pub(crate) const NOT_UTF8: &str = "Its name is not valid UTF-8, which gg cannot hand back to \
                                   git exactly; commit it in a terminal.";

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

/// The changed files of the worktree at `worktree`, sorted by path: the first
/// [`MAX_LISTED_FILES`], and whether there are more. Untracked files are listed one by one;
/// submodules and ignored files not at all.
pub async fn read_files(r: &GitRunner, worktree: &Path) -> Result<WorktreeFiles> {
    let out = r.run(worktree, &STATUS_ARGS, CmdKind::Read).await?;
    let mut files = parse_files(&out.stdout);
    let truncated = files.len() > MAX_LISTED_FILES;
    files.truncate(MAX_LISTED_FILES);
    let merge_message = read_merge_message(r, worktree).await?;
    Ok(WorktreeFiles {
        files,
        truncated,
        merge_message,
    })
}

/// The message git prepared for the merge stopped in `worktree` (its `MERGE_MSG`), without
/// its `#` comment lines, trimmed; `None` when no merge is stopped there, or it is empty.
async fn read_merge_message(r: &GitRunner, worktree: &Path) -> Result<Option<String>> {
    let out = r
        .run(
            worktree,
            &[
                "rev-parse",
                "--git-path",
                "MERGE_HEAD",
                "--git-path",
                "MERGE_MSG",
            ],
            CmdKind::Read,
        )
        .await?;
    let text = out.text();
    let mut lines = text.lines();
    let (Some(head), Some(msg)) = (lines.next(), lines.next()) else {
        return Ok(None);
    };
    if !worktree.join(head).is_file() {
        return Ok(None);
    }
    let Ok(raw) = std::fs::read(worktree.join(msg)) else {
        return Ok(None);
    };
    let comment = comment_string(r, worktree).await?;
    Ok(
        Some(merge_message_text(&String::from_utf8_lossy(&raw), &comment))
            .filter(|m| !m.is_empty()),
    )
}

/// What starts a comment line in the repository's messages: `core.commentString`, else
/// `core.commentChar`, else `#` (also for `auto`, which git settles only while it writes a
/// message of its own, keeping `#` for a merge's).
async fn comment_string(r: &GitRunner, worktree: &Path) -> Result<String> {
    for key in ["core.commentString", "core.commentChar"] {
        let out = r
            .run_unchecked(worktree, &["config", "--get", key], CmdKind::Read)
            .await?;
        let value = out.text();
        let value = value.trim_end_matches(['\n', '\r']);
        if out.code == 0 && !value.is_empty() {
            return Ok(if value == "auto" { "#" } else { value }.to_owned());
        }
    }
    Ok("#".to_owned())
}

/// `MERGE_MSG`'s text as the commit panel offers it: lines starting with `comment` (the
/// repository's comment string) removed, trimmed.
fn merge_message_text(raw: &str, comment: &str) -> String {
    raw.lines()
        .filter(|l| !l.starts_with(comment))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Every changed file in porcelain v2 output `out`, sorted by path, one entry per path (a
/// path whose deletion is staged and which is back in the working tree as an untracked file
/// is one entry: modified, staged and unstaged).
pub(crate) fn parse_files(out: &[u8]) -> Vec<FileEntry> {
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    // A name that is not valid UTF-8 is shown as best it can be, which is not its name: it
    // cannot be handed back to git to commit (P3-R17).
    let named = |mut entry: FileEntry, raw: &[&[u8]]| {
        if raw.iter().any(|r| std::str::from_utf8(r).is_err()) {
            entry.committable = false;
            entry.reason = Some(NOT_UTF8.to_owned());
        }
        entry
    };
    let mut files: Vec<FileEntry> = Vec::new();
    let mut fields = out.split(|&b| b == 0);
    while let Some(entry) = fields.next() {
        let (kind, rest) = match entry {
            [kind, b' ', rest @ ..] => (*kind, rest),
            _ => continue,
        };
        match kind {
            // `1 XY sub mH mI mW hH hI <path>`
            b'1' => {
                let mut parts = rest.splitn(8, |&b| b == b' ');
                let (Some(&[x, y]), Some(path)) = (parts.next(), parts.nth(6)) else {
                    continue;
                };
                files.push(named(
                    FileEntry {
                        path: text(path),
                        orig_path: None,
                        status: changed(x, y),
                        staged: x != b'.',
                        unstaged: y != b'.',
                        committable: true,
                        reason: None,
                    },
                    &[path],
                ));
            }
            // `2 XY sub mH mI mW hH hI Xscore <path>`, then `<origPath>` as the next field.
            b'2' => {
                let mut parts = rest.splitn(9, |&b| b == b' ');
                let (Some(&[x, y]), Some(path)) = (parts.next(), parts.nth(7)) else {
                    continue;
                };
                let orig_raw = fields.next().filter(|o| !o.is_empty());
                let status = if x == b'C' || y == b'C' {
                    EntryStatus::C
                } else {
                    EntryStatus::R
                };
                files.push(named(
                    FileEntry {
                        path: text(path),
                        orig_path: orig_raw.map(text),
                        status,
                        staged: x != b'.',
                        unstaged: y != b'.',
                        committable: true,
                        reason: None,
                    },
                    &[path, orig_raw.unwrap_or_default()],
                ));
            }
            // `u XY sub m1 m2 m3 mW h1 h2 h3 <path>`
            b'u' => {
                let Some(path) = rest.splitn(10, |&b| b == b' ').nth(9) else {
                    continue;
                };
                files.push(named(
                    FileEntry {
                        path: text(path),
                        orig_path: None,
                        status: EntryStatus::U,
                        staged: false,
                        unstaged: true,
                        committable: true,
                        reason: None,
                    },
                    &[path],
                ));
            }
            b'?' => files.push(named(
                FileEntry {
                    path: text(rest),
                    orig_path: None,
                    status: EntryStatus::Untracked,
                    staged: false,
                    unstaged: true,
                    committable: true,
                    reason: None,
                },
                &[rest],
            )),
            _ => {}
        }
    }
    files.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then((a.status == EntryStatus::Untracked).cmp(&(b.status == EntryStatus::Untracked)))
    });
    // A staged deletion whose file is back (untracked): one path, changed.
    files.dedup_by(|later, first| {
        let same = later.path == first.path && later.status == EntryStatus::Untracked;
        if same {
            first.status = EntryStatus::M;
            first.unstaged = true;
        }
        same
    });
    files
}

/// The status of a changed (`1`) entry with index letter `x` and working-tree letter `y`:
/// how the working tree differs from HEAD, which is what a commit of it records.
fn changed(x: u8, y: u8) -> EntryStatus {
    match (x, y) {
        // Added to the index, deleted from the working tree since: what a commit of it does
        // is drop that entry (HEAD never had it).
        (b'A', b'D') => EntryStatus::D,
        (b'A', _) | (_, b'A') => EntryStatus::A,
        (b'D', _) | (_, b'D') => EntryStatus::D,
        (b'T', _) | (_, b'T') => EntryStatus::T,
        _ => EntryStatus::M,
    }
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

    #[test]
    fn a_merge_message_loses_its_comment_lines() {
        let raw = "Merge branch 'feature'\n\n# Conflicts:\n#\tclash.txt\n";
        assert_eq!(merge_message_text(raw, "#"), "Merge branch 'feature'");
        assert_eq!(merge_message_text("# only comments\n\n", "#"), "");
        assert_eq!(
            merge_message_text("Merge x\n# kept\n; gone\n", ";"),
            "Merge x\n# kept"
        );
    }

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

    fn entry(path: &str, status: EntryStatus, staged: bool, unstaged: bool) -> FileEntry {
        FileEntry {
            path: path.into(),
            orig_path: None,
            status,
            staged,
            unstaged,
            committable: true,
            reason: None,
        }
    }

    #[test]
    fn a_name_that_is_not_utf8_is_listed_as_not_committable() {
        let out =
            b"? caf\xe9.txt\x00? fine.txt\x002 R. N... 100644 100644 100644 a a R100 new\x00ol\xffd\x00";
        let files = parse_files(out);
        let committable: Vec<(bool, bool)> = files
            .iter()
            .map(|f| (f.committable, f.reason.is_some()))
            .collect();
        assert_eq!(
            committable,
            [(false, true), (true, false), (false, true)],
            "{files:?}"
        );
        assert_eq!(files[0].reason.as_deref(), Some(NOT_UTF8));
    }

    #[test]
    fn files_are_listed_with_their_status_sorted() {
        let out = [
            "1 .M N... 100644 100644 100644 aaaa aaaa z modified\0",
            "1 A. N... 000000 100644 100644 0000 aaaa added\0",
            "1 AM N... 000000 100644 100644 0000 aaaa added then edited\0",
            "1 .D N... 100644 100644 000000 aaaa aaaa deleted here\0",
            "1 D. N... 100644 000000 000000 aaaa 0000 deleted staged\0",
            "1 MM N... 100644 100644 100644 aaaa bbbb both\0",
            "1 .T N... 100644 100644 120000 aaaa aaaa link now\0",
            "1 .A N... 000000 000000 100644 0000 0000 intent\0",
            RENAME,
            "2 C. N... 100644 100644 100644 aaaa aaaa C75 copy\0copied from\0",
            UNMERGED,
            "? -dash \u{fc}\nline\0",
            "! ignored\0",
        ]
        .concat();
        let files = parse_files(out.as_bytes());
        let renamed = FileEntry {
            orig_path: Some("old name.txt".into()),
            ..entry("new name.txt", EntryStatus::R, true, false)
        };
        let copied = FileEntry {
            orig_path: Some("copied from".into()),
            ..entry("copy", EntryStatus::C, true, false)
        };
        assert_eq!(
            files,
            [
                entry("-dash \u{fc}\nline", EntryStatus::Untracked, false, true),
                entry("added", EntryStatus::A, true, false),
                entry("added then edited", EntryStatus::A, true, true),
                entry("both", EntryStatus::M, true, true),
                entry("c.txt", EntryStatus::U, false, true),
                copied,
                entry("deleted here", EntryStatus::D, false, true),
                entry("deleted staged", EntryStatus::D, true, false),
                entry("intent", EntryStatus::A, false, true),
                entry("link now", EntryStatus::T, false, true),
                renamed,
                entry("z modified", EntryStatus::M, false, true),
            ]
        );
    }

    #[test]
    fn a_staged_deletion_whose_file_is_back_is_one_entry() {
        let out = [
            "? back\0? other\0",
            "1 D. N... 100644 000000 000000 aaaa 0000 back\0",
        ]
        .concat();
        assert_eq!(
            parse_files(out.as_bytes()),
            [
                entry("back", EntryStatus::M, true, true),
                entry("other", EntryStatus::Untracked, false, true),
            ]
        );
    }

    #[test]
    fn the_file_list_survives_hostile_input() {
        assert!(parse_files(b"").is_empty());
        assert!(parse_files(b"\0\0# branch.oid abc\0").is_empty());
        assert!(parse_files(b"1 .M short\0u UU x\0").is_empty());
        // A rename without its original path keeps what there is.
        let files = parse_files(b"2 R. N... x y z a b R100 new\0");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].orig_path, None);
    }
}
