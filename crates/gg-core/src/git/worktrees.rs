//! Lists the repository's worktrees.

use std::path::Path;

use crate::dto::WorktreeInfo;
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};

/// All worktrees of the repository containing `cwd`, the main worktree first.
pub async fn read_worktrees(r: &GitRunner, cwd: &Path) -> Result<Vec<WorktreeInfo>> {
    let out = r
        .run(
            cwd,
            &["worktree", "list", "--porcelain", "-z"],
            CmdKind::Read,
        )
        .await?;
    Ok(parse_worktrees(&out.stdout))
}

/// Parses `worktree list --porcelain -z`: NUL-terminated `key[ value]` attributes, each
/// worktree ended by an empty attribute. The first worktree is the main one. Only what git
/// lists is filled in (the state fields stay empty); paths are spelled as the platform does
/// ([`crate::platform::native_path`]).
fn parse_worktrees(out: &[u8]) -> Vec<WorktreeInfo> {
    let mut list = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    for attr in out.split(|&b| b == 0) {
        if attr.is_empty() {
            list.extend(current.take());
            continue;
        }
        let attr = String::from_utf8_lossy(attr);
        let (key, value) = attr.split_once(' ').unwrap_or((&attr, ""));
        if key == "worktree" {
            // A new record also ends one whose terminator is missing.
            list.extend(current.take());
            if !value.is_empty() {
                current = Some(WorktreeInfo {
                    path: crate::platform::native_path(value),
                    is_main: list.is_empty(),
                    ..WorktreeInfo::default()
                });
            }
            continue;
        }
        let Some(wt) = current.as_mut() else {
            continue;
        };
        match key {
            // An unborn branch is reported with the all-zero hash.
            "HEAD" if is_hex(value) && value.bytes().any(|b| b != b'0') => {
                wt.head = Some(value.to_owned());
            }
            "branch" => {
                let name = value.strip_prefix("refs/heads/").unwrap_or(value);
                wt.branch = (!name.is_empty()).then(|| name.to_owned());
            }
            "detached" => wt.branch = None,
            // `locked`, or `locked <reason>` when one was given.
            "locked" => {
                wt.locked = true;
                wt.lock_reason = (!value.is_empty()).then(|| value.to_owned());
            }
            "prunable" => wt.prunable = true,
            "bare" => wt.bare = true,
            _ => {}
        }
    }
    list.extend(current);
    list
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::platform::native_path as native;

    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn parses_main_linked_detached_locked_prunable() {
        let out = format!(
            "worktree /repo/main dir\0HEAD {C}\0branch refs/heads/main\0\0\
             worktree /wt/ünï ✨\0HEAD {C}\0branch refs/heads/feature/x\0locked\0\0\
             worktree /wt/detached\0HEAD {C}\0detached\0locked on usb\0\0\
             worktree /wt/gone\0HEAD {C}\0branch refs/heads/gone\0prunable gitdir file points to non-existent location\0\0"
        );
        let list = parse_worktrees(out.as_bytes());
        assert_eq!(
            list,
            [
                WorktreeInfo {
                    path: native("/repo/main dir"),
                    head: Some(C.into()),
                    branch: Some("main".into()),
                    is_main: true,
                    ..WorktreeInfo::default()
                },
                WorktreeInfo {
                    path: native("/wt/ünï ✨"),
                    head: Some(C.into()),
                    branch: Some("feature/x".into()),
                    is_main: false,
                    locked: true,
                    ..WorktreeInfo::default()
                },
                WorktreeInfo {
                    path: native("/wt/detached"),
                    head: Some(C.into()),
                    branch: None,
                    is_main: false,
                    locked: true,
                    lock_reason: Some("on usb".into()),
                    ..WorktreeInfo::default()
                },
                WorktreeInfo {
                    path: native("/wt/gone"),
                    head: Some(C.into()),
                    branch: Some("gone".into()),
                    is_main: false,
                    prunable: true,
                    ..WorktreeInfo::default()
                },
            ]
        );
    }

    #[test]
    fn unborn_and_bare_have_no_head() {
        let out = format!(
            "worktree /bare.git\0bare\0\0worktree /wt\0HEAD {ZERO}\0branch refs/heads/main\0\0"
        );
        let list = parse_worktrees(out.as_bytes());
        assert_eq!(list.len(), 2);
        assert!(list[0].is_main);
        assert!(list[0].bare && !list[1].bare);
        assert_eq!(
            (list[0].head.as_deref(), list[0].branch.as_deref()),
            (None, None)
        );
        assert_eq!(list[1].head, None);
        assert_eq!(list[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn survives_hostile_input() {
        assert!(parse_worktrees(b"").is_empty());
        assert!(parse_worktrees(b"\0\0\0").is_empty());
        assert!(parse_worktrees(b"HEAD abc\0branch refs/heads/x\0\0").is_empty());
        assert!(parse_worktrees(b"worktree \0\0").is_empty());
        // Truncated: the last record has no terminating empty field.
        let out = format!("worktree /a\0HEAD {C}\0\0worktree /b\0HEAD {C}");
        let list = parse_worktrees(out.as_bytes());
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].path, native("/b"));
        assert!(!list[1].is_main);
        // Unknown attributes are ignored; bad bytes decode lossily.
        let list = parse_worktrees(b"worktree /x\xFF\0future thing\0HEAD zz\0\0");
        assert_eq!(list[0].path, native("/x\u{FFFD}"));
        assert_eq!(list[0].head, None);
    }
}
