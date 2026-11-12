//! Counts a worktree's changes from `git status --porcelain=v2`.

use std::path::Path;

use crate::dto::WorktreeStatus;
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};

/// Change counts for the worktree at `worktree` (untracked files counted one by one).
pub async fn read_status(r: &GitRunner, worktree: &Path) -> Result<WorktreeStatus> {
    let out = r
        .run(
            worktree,
            &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
            CmdKind::Read,
        )
        .await?;
    Ok(parse_status(&out.stdout))
}

/// Counts porcelain v2 entries: `1` (changed), `2` (renamed or copied; followed by an extra
/// NUL-terminated original path), `u` (unmerged: changed and conflicted), `?` (untracked).
fn parse_status(out: &[u8]) -> WorktreeStatus {
    let mut s = WorktreeStatus::default();
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
        } else if entry.starts_with(b"? ") {
            s.untracked = s.untracked.saturating_add(1);
        }
    }
    s
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
        assert_eq!(parse_status(out.as_bytes()), counts(3, 2, 1));
    }

    #[test]
    fn rename_original_path_is_not_an_entry() {
        // The original path of a rename looks like anything, even like an entry header.
        let out = "2 R. N... 100644 100644 100644 aaaa aaaa R100 new\0? not untracked\0";
        assert_eq!(parse_status(out.as_bytes()), counts(1, 0, 0));
        let out = "2 R. N... 100644 100644 100644 aaaa aaaa R100 new\0u x\0? real\0";
        assert_eq!(parse_status(out.as_bytes()), counts(1, 1, 0));
    }

    #[test]
    fn survives_hostile_input() {
        assert_eq!(parse_status(b""), counts(0, 0, 0));
        assert_eq!(parse_status(b"\0\0\0"), counts(0, 0, 0));
        assert_eq!(parse_status(b"# branch.oid abc\0"), counts(0, 0, 0));
        assert_eq!(parse_status(b"\xFF\xFE\0zzz\0"), counts(0, 0, 0));
        // Truncated: a rename entry with no original path field, an entry with no NUL.
        assert_eq!(
            parse_status(b"2 R. N... x y z a b R100 new\0"),
            counts(1, 0, 0)
        );
        assert_eq!(parse_status(b"? partial"), counts(0, 1, 0));
    }
}
