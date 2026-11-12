//! Types that cross the boundary to the UI. Their JSON shapes are fixed by the Boundary
//! Types section of the plan and mirrored in `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

/// Which refs the graph shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewOptions {
    /// Short names of the branches to show (local or remote); `None` shows all.
    pub branches: Option<Vec<String>>,
    pub show_remotes: bool,
    pub show_tags: bool,
}

/// One worktree of the repository, as `git worktree list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    /// Absolute path as git reports it (symlinks resolved).
    pub path: String,
    /// Checked-out commit; `None` before the first commit (or for a bare main repository).
    pub head: Option<String>,
    /// Short branch name; `None` when detached.
    pub branch: Option<String>,
    /// The main worktree (always listed first).
    pub is_main: bool,
    pub locked: bool,
    /// Git considers it prunable, typically because its directory is gone.
    pub prunable: bool,
}

/// Change counts for one worktree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeStatus {
    /// Tracked paths with staged or unstaged changes, conflicts included.
    pub changed: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

/// How a file changed in a commit. Serialized as its single letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    /// Added.
    A,
    /// Modified.
    M,
    /// Deleted.
    D,
    /// Renamed (`old_path` is set).
    R,
    /// Copied (`old_path` is set).
    C,
    /// Type changed (for example a file became a symlink).
    T,
}

/// One file changed by a commit, compared with its first parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    /// Source path of a rename or copy.
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// Lines added; `None` for binary files.
    pub additions: Option<u32>,
    /// Lines deleted; `None` for binary files.
    pub deletions: Option<u32>,
}

/// Everything the details pane shows for one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    /// Unix seconds.
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    /// Unix seconds.
    pub commit_time: i64,
    /// Full message (subject and body), trailing whitespace removed.
    pub message: String,
    pub files: Vec<FileChange>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn view_options_round_trip_camel_case() {
        let opts = ViewOptions {
            branches: Some(vec!["main".into()]),
            show_remotes: true,
            show_tags: false,
        };
        let v = serde_json::to_value(&opts).unwrap();
        assert_eq!(
            v,
            json!({"branches": ["main"], "showRemotes": true, "showTags": false})
        );
        assert_eq!(serde_json::from_value::<ViewOptions>(v).unwrap(), opts);
        let all: ViewOptions = serde_json::from_value(
            json!({"branches": null, "showRemotes": false, "showTags": true}),
        )
        .unwrap();
        assert_eq!(all.branches, None);
    }

    #[test]
    fn worktree_info_and_status_shapes() {
        let wt = WorktreeInfo {
            path: "/r".into(),
            head: None,
            branch: Some("main".into()),
            is_main: true,
            locked: false,
            prunable: true,
        };
        let v = serde_json::to_value(&wt).unwrap();
        assert_eq!(
            v,
            json!({"path": "/r", "head": null, "branch": "main", "isMain": true,
                   "locked": false, "prunable": true})
        );
        assert_eq!(serde_json::from_value::<WorktreeInfo>(v).unwrap(), wt);

        let st = WorktreeStatus {
            changed: 1,
            untracked: 2,
            conflicted: 3,
        };
        let v = serde_json::to_value(st).unwrap();
        assert_eq!(v, json!({"changed": 1, "untracked": 2, "conflicted": 3}));
        assert_eq!(serde_json::from_value::<WorktreeStatus>(v).unwrap(), st);
    }

    #[test]
    fn file_status_is_one_letter() {
        let all = [
            (FileStatus::A, "A"),
            (FileStatus::M, "M"),
            (FileStatus::D, "D"),
            (FileStatus::R, "R"),
            (FileStatus::C, "C"),
            (FileStatus::T, "T"),
        ];
        for (status, letter) in all {
            assert_eq!(serde_json::to_value(status).unwrap(), letter);
            assert_eq!(
                serde_json::from_value::<FileStatus>(json!(letter)).unwrap(),
                status
            );
        }
    }

    #[test]
    fn commit_details_shape() {
        let d = CommitDetails {
            hash: "h".into(),
            parents: vec!["p".into()],
            author: "A".into(),
            author_email: "a@x".into(),
            author_time: 1,
            committer: "C".into(),
            committer_email: "c@x".into(),
            commit_time: 2,
            message: "m".into(),
            files: vec![FileChange {
                path: "new".into(),
                old_path: Some("old".into()),
                status: FileStatus::R,
                additions: None,
                deletions: Some(0),
            }],
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(
            v,
            json!({
                "hash": "h", "parents": ["p"], "author": "A", "authorEmail": "a@x",
                "authorTime": 1, "committer": "C", "committerEmail": "c@x", "commitTime": 2,
                "message": "m",
                "files": [{"path": "new", "oldPath": "old", "status": "R",
                           "additions": null, "deletions": 0}]
            })
        );
        assert_eq!(serde_json::from_value::<CommitDetails>(v).unwrap(), d);
    }
}
