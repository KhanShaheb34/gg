//! Types that cross the boundary to the UI. Their JSON shapes are fixed by the Boundary
//! Types section of the plan and mirrored in `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

pub use crate::graph::Edge;

/// Which refs the graph shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewOptions {
    /// Short names of the branches to show (local or remote); `None` shows all.
    pub branches: Option<Vec<String>>,
    pub show_remotes: bool,
    pub show_tags: bool,
    /// Missing in requests from before stashes were shown: then true.
    #[serde(default = "yes")]
    pub show_stashes: bool,
}

fn yes() -> bool {
    true
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeStatus {
    /// Tracked paths with staged or unstaged changes, conflicts included.
    pub changed: u32,
    pub untracked: u32,
    pub conflicted: u32,
    /// The conflicted paths, relative to the worktree, sorted: the first
    /// [`MAX_CONFLICTED_PATHS`] of them.
    pub conflicted_paths: Vec<String>,
}

/// Most conflicted paths a [`WorktreeStatus`] names.
pub const MAX_CONFLICTED_PATHS: usize = 200;

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
    /// The commit's parents are missing because the repository is a shallow clone and this
    /// is where its history was cut (`parents` is then empty, though it is no root commit).
    pub shallow_boundary: bool,
}

/// Kind of a ref. Serialized lowercase (`"branch"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    /// Local branch (`refs/heads/`).
    Branch,
    /// Remote-tracking branch (`refs/remotes/`).
    Remote,
    /// Tag (`refs/tags/`).
    Tag,
    /// Stash entry (`stash@{0}`); only ever a label, never read as a ref.
    Stash,
}

/// What HEAD points at in a worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadInfo {
    /// Short name of the checked-out branch; `None` when detached.
    pub branch: Option<String>,
    /// The checked-out commit; `None` before the branch's first commit.
    pub hash: Option<String>,
}

/// Progress of `git fetch` for one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchState {
    pub running: bool,
    /// Why the last fetch failed, in one line: git's first `fatal:` or `error:` line (with
    /// several remotes, the lines of those that fetched come first), else its last line;
    /// `None` after a success.
    pub last_error: Option<String>,
    /// git's stderr of the last failed (or timed-out) fetch, its last 8 KiB at most; `None`
    /// after a success or when git said nothing.
    pub last_error_detail: Option<String>,
    /// When the last fetch finished, unix milliseconds.
    pub last_at_ms: Option<i64>,
}

/// Everything the UI shows about an open repository, apart from its rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSnapshot {
    pub repo_id: u32,
    /// File name of `root_path`.
    pub name: String,
    /// The main worktree.
    pub root_path: String,
    /// The worktree the repository was opened from; HEAD, status and the uncommitted row
    /// refer to it.
    pub active_worktree: String,
    /// Changes whenever rows may have changed; `rows()` with another generation fails.
    pub generation: u64,
    pub total_rows: u32,
    /// The log is still being read.
    pub loading: bool,
    /// Highest lane index used by the rows laid out so far.
    pub max_lane: u16,
    pub head: HeadInfo,
    /// Local branch short names, sorted.
    pub branches: Vec<String>,
    /// Remote-tracking branch short names (`origin/main`), sorted.
    pub remote_branches: Vec<String>,
    pub remotes: Vec<String>,
    pub worktrees: Vec<WorktreeInfo>,
    /// Status of the active worktree.
    pub status: WorktreeStatus,
    pub view_options: ViewOptions,
    pub fetch: FetchState,
    /// The operation running on this repository, if any.
    pub op: Option<RunningOp>,
    /// A merge, rebase, cherry-pick or revert git stopped in, if any.
    pub in_progress: Option<InProgress>,
    /// Grows by one, per repository, with each snapshot that changes anything (every
    /// `repo-changed`) and with every operation's outcome; a plain read repeats the latest.
    /// The UI drops a snapshot whose `seq` is below the one it has.
    pub seq: u64,
}

/// The operation a repository is busy with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningOp {
    pub id: u64,
    pub title: String,
    /// When it started running (not when it was asked for), unix milliseconds.
    pub started_at_ms: i64,
    /// Requests waiting behind it.
    pub queued: u32,
}

/// What git is in the middle of. Serialized as `"merge"`, `"rebase"`, `"cherry-pick"`,
/// `"revert"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InProgressKind {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

/// A merge, rebase, cherry-pick or revert that git stopped in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InProgress {
    pub kind: InProgressKind,
    /// Conflicted paths relative to the worktree, sorted.
    pub conflicted: Vec<String>,
    /// The conflicted paths that still have conflict markers (`git diff --check`), sorted:
    /// not resolved yet. The others are resolved, though unmerged until they are staged.
    pub unresolved: Vec<String>,
    /// Rebase only: the step git stopped at, and how many there are.
    pub step: Option<u32>,
    pub total: Option<u32>,
    /// What is being applied: the incoming branch or the commit's subject.
    pub subject: Option<String>,
}

/// A ref shown on a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    pub kind: RefKind,
    /// Short name: `main`, `origin/main`, `v1.0`.
    pub name: String,
    /// Branch only: remotes whose same-named branch is at this commit.
    pub remotes: Vec<String>,
    /// Checked out in the active worktree.
    pub current: bool,
    /// Path of a worktree that has this branch checked out.
    pub worktree_path: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// Whether a row is a commit or the synthetic row for uncommitted changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowKind {
    Commit,
    Uncommitted,
    /// A stash entry.
    Stash,
}

/// One row of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRow {
    pub index: u32,
    pub kind: RowKind,
    /// Full hash; `"UNCOMMITTED"` for the uncommitted row.
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, unix seconds.
    pub time: i64,
    pub subject: String,
    pub lane: u16,
    pub color: u16,
    pub edges: Vec<Edge>,
    pub labels: Vec<RefLabel>,
    /// The commit the active worktree's HEAD points at.
    pub is_head: bool,
    pub is_merge: bool,
    /// Paths of worktrees whose detached HEAD is at this commit.
    pub detached_worktrees: Vec<String>,
}

/// Rows of one generation whose commit message matches a search (see `search_bodies`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyHits {
    pub generation: u64,
    /// Ascending row indices.
    pub rows: Vec<u32>,
}

/// A window of rows of one generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsPage {
    pub generation: u64,
    pub start: u32,
    pub rows: Vec<CommitRow>,
}

/// Color theme. Serialized lowercase (`"system"`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

/// User settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    /// Seconds between background fetches; 0 turns them off, otherwise 30..=86400.
    pub fetch_interval_secs: u32,
    /// Program (and arguments) that opens files; `None` uses the system's default opener.
    pub editor_command: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            fetch_interval_secs: 300,
            editor_command: None,
        }
    }
}

/// Preferences kept per repository (by its main worktree's path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoPrefs {
    /// Merges create a merge commit even when a fast-forward is possible.
    pub merge_no_ff: bool,
}

impl Default for RepoPrefs {
    fn default() -> Self {
        Self { merge_no_ff: true }
    }
}

/// What a merge takes in. Serialized lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeTargetKind {
    Branch,
    Remote,
    Commit,
}

/// A merge's source: a branch name, a remote branch (`origin/x`) or a full commit hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MergeTarget {
    pub kind: MergeTargetKind,
    pub name: String,
}

/// `git reset` mode. Serialized lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

/// How a pull integrates: as the user's git config says, by merging or by rebasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullMode {
    Default,
    Merge,
    Rebase,
}

/// One operation the user asked for. Serialized with its name in `op`
/// (`{"op": "create_branch", "name": ..}`); field names camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OpRequest {
    CheckoutBranch {
        branch: String,
        fast_forward: bool,
    },
    CheckoutRemote {
        remote_branch: String,
        local_name: String,
    },
    CheckoutCommit {
        hash: String,
    },
    CreateBranch {
        name: String,
        /// The commit the branch starts at (a full hash, like every commit field here).
        at: String,
        checkout: bool,
    },
    RenameBranch {
        branch: String,
        new_name: String,
    },
    DeleteBranch {
        branch: String,
    },
    Merge {
        target: MergeTarget,
        no_ff: bool,
        squash: bool,
        no_commit: bool,
    },
    Rebase {
        /// A local or remote branch name.
        onto: String,
    },
    CherryPick {
        hash: String,
        mainline: Option<u32>,
        no_commit: bool,
    },
    Revert {
        hash: String,
        mainline: Option<u32>,
    },
    Reset {
        hash: String,
        mode: ResetMode,
    },
    CreateTag {
        name: String,
        hash: String,
        /// Set: an annotated tag with this message.
        message: Option<String>,
    },
    DeleteTag {
        name: String,
        /// Set: also delete it on this remote.
        remote: Option<String>,
    },
    PushTag {
        name: String,
        remote: String,
    },
    Push {
        branch: String,
        remote: String,
        set_upstream: bool,
        force: bool,
        /// With `force`: the `token` of the `force_push` danger the user confirmed. The
        /// push runs only if it is still the remote-tracking commit, and git refuses it if
        /// the remote branch is anywhere else (`--force-with-lease=<ref>:<lease>`).
        lease: Option<String>,
    },
    Pull {
        branch: String,
        mode: PullMode,
    },
    FetchIntoLocal {
        remote_branch: String,
        local_name: String,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    StashSave {
        message: Option<String>,
        include_untracked: bool,
    },
    /// `stash` is the stash's commit hash.
    StashApply {
        stash: String,
    },
    StashPop {
        stash: String,
    },
    StashDrop {
        stash: String,
    },
    StashBranch {
        stash: String,
        name: String,
    },
    Continue,
    Abort,
}

/// A dangerous effect an operation runs only once confirmed. Serialized snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DangerKind {
    ForcePush,
    HardReset,
    DeleteUnmergedBranch,
    DeleteRemoteBranch,
    DropStash,
}

/// What would be lost, said plainly, for the confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Danger {
    pub kind: DangerKind,
    pub message: String,
    pub detail: Option<String>,
    /// `force_push` only: the remote-tracking commit the message was worked out from
    /// (`null` when there is no remote-tracking branch). Confirming means sending it back as
    /// the push's `lease`; if it no longer matches, the core asks again.
    pub token: Option<String>,
}

/// A follow-up the outcome's notice offers. Serialized with its name in `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OpHint {
    Push {
        branch: String,
        remote: String,
        set_upstream: bool,
    },
    Fetch,
    Pull {
        branch: String,
    },
    ForcePush {
        branch: String,
        remote: String,
    },
    ChoosePullMode {
        branch: String,
    },
}

/// How an operation ended. Serialized snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpStatus {
    Done,
    /// git stopped on conflicts (not a failure); `snapshot.inProgress` is set.
    Stopped,
    Failed,
    /// Nothing ran: the dangers listed need confirming first.
    NeedsConfirmation,
}

/// The result of `run_op`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpOutcome {
    pub op_id: u64,
    pub status: OpStatus,
    /// What the operation is, for people: "Create branch feature/x".
    pub title: String,
    /// One plain line for the notice.
    pub message: String,
    /// git's own output, on failure.
    pub detail: Option<String>,
    /// Command-log ids of the operation's first and last write (0, 0 if none ran).
    pub log_from: u64,
    pub log_to: u64,
    /// Non-empty only for `needs_confirmation`.
    pub dangers: Vec<Danger>,
    pub hints: Vec<OpHint>,
    /// The repository after the operation and its refresh. After a pull or a fetch into a
    /// local branch it may still be the state before the log reload has caught up; a
    /// `repo-changed` with a higher `seq` follows.
    pub snapshot: RepoSnapshot,
}

/// A recently opened repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    /// The main worktree.
    pub path: String,
    /// File name of `path`.
    pub name: String,
    /// Unix milliseconds.
    pub last_opened_ms: i64,
}

/// The open tabs, to restore on the next start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Root paths of the open repositories, in tab order.
    pub tabs: Vec<String>,
    /// Root path of the active tab.
    pub active: Option<String>,
}

/// Everything the UI needs at start, apart from repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    /// Most recent first.
    pub recents: Vec<Recent>,
    pub session: Session,
    pub settings: Settings,
    /// A folder the app was asked to open at launch (command line or `GG_OPEN`).
    pub launch_path: Option<String>,
    /// The user's home folder, so the UI can show paths as `~/...`.
    pub home_dir: Option<String>,
    /// Why settings and the session are not being saved (the settings file exists but could
    /// not be read); `None` when they are. `reset_settings` moves the file aside.
    pub settings_problem: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeHalf;
    use serde_json::{Value, json};

    #[test]
    fn app_state_shape() {
        let state = AppState {
            recents: vec![Recent {
                path: "/r".into(),
                name: "r".into(),
                last_opened_ms: 5,
            }],
            session: Session {
                tabs: vec!["/r".into()],
                active: None,
            },
            settings: Settings::default(),
            launch_path: Some("/l".into()),
            home_dir: Some("/h".into()),
            settings_problem: None,
        };
        let v = serde_json::to_value(&state).unwrap();
        assert_eq!(
            v,
            json!({
                "recents": [{"path": "/r", "name": "r", "lastOpenedMs": 5}],
                "session": {"tabs": ["/r"], "active": null},
                "settings": {"theme": "system", "fetchIntervalSecs": 300, "editorCommand": null},
                "launchPath": "/l",
                "homeDir": "/h",
                "settingsProblem": null
            })
        );
        assert_eq!(serde_json::from_value::<AppState>(v).unwrap(), state);
        assert_eq!(serde_json::to_value(Theme::Dark).unwrap(), "dark");
    }

    #[test]
    fn view_options_round_trip_camel_case() {
        let opts = ViewOptions {
            branches: Some(vec!["main".into()]),
            show_remotes: true,
            show_tags: false,
            show_stashes: false,
        };
        let v = serde_json::to_value(&opts).unwrap();
        assert_eq!(
            v,
            json!({"branches": ["main"], "showRemotes": true, "showTags": false,
                   "showStashes": false})
        );
        assert_eq!(serde_json::from_value::<ViewOptions>(v).unwrap(), opts);
        // Without `showStashes` (a UI from before stashes): shown.
        let all: ViewOptions = serde_json::from_value(
            json!({"branches": null, "showRemotes": false, "showTags": true}),
        )
        .unwrap();
        assert_eq!(all.branches, None);
        assert!(all.show_stashes);
    }

    /// Every request parses from the JSON the UI sends, and unknown fields are refused.
    #[test]
    fn op_requests_parse_from_the_boundary_shapes() {
        let cases = [
            json!({"op": "checkout_branch", "branch": "b", "fastForward": true}),
            json!({"op": "checkout_remote", "remoteBranch": "origin/b", "localName": "b"}),
            json!({"op": "checkout_commit", "hash": "h"}),
            json!({"op": "create_branch", "name": "n", "at": "h", "checkout": false}),
            json!({"op": "rename_branch", "branch": "b", "newName": "c"}),
            json!({"op": "delete_branch", "branch": "b"}),
            json!({"op": "merge", "target": {"kind": "remote", "name": "origin/x"},
                   "noFf": true, "squash": false, "noCommit": false}),
            json!({"op": "rebase", "onto": "main"}),
            json!({"op": "cherry_pick", "hash": "h", "mainline": 1, "noCommit": true}),
            json!({"op": "revert", "hash": "h", "mainline": null}),
            json!({"op": "reset", "hash": "h", "mode": "hard"}),
            json!({"op": "create_tag", "name": "v1", "hash": "h", "message": null}),
            json!({"op": "delete_tag", "name": "v1", "remote": "origin"}),
            json!({"op": "push_tag", "name": "v1", "remote": "origin"}),
            json!({"op": "push", "branch": "b", "remote": "origin", "setUpstream": true,
                   "force": true, "lease": "abc"}),
            json!({"op": "pull", "branch": "b", "mode": "default"}),
            json!({"op": "fetch_into_local", "remoteBranch": "origin/b", "localName": "b"}),
            json!({"op": "delete_remote_branch", "remote": "origin", "branch": "b"}),
            json!({"op": "stash_save", "message": "m", "includeUntracked": true}),
            json!({"op": "stash_apply", "stash": "h"}),
            json!({"op": "stash_pop", "stash": "h"}),
            json!({"op": "stash_drop", "stash": "h"}),
            json!({"op": "stash_branch", "stash": "h", "name": "n"}),
            json!({"op": "continue"}),
            json!({"op": "abort"}),
        ];
        for case in cases {
            let request: OpRequest =
                serde_json::from_value(case.clone()).unwrap_or_else(|e| panic!("{case}: {e}"));
            assert_eq!(serde_json::to_value(&request).unwrap(), case);
        }
        assert_eq!(
            serde_json::from_value::<OpRequest>(
                json!({"op": "reset", "hash": "h", "mode": "keep"})
            )
            .ok(),
            None
        );
        assert_eq!(
            serde_json::from_value::<OpRequest>(
                json!({"op": "delete_branch", "branch": "b", "force": true})
            )
            .ok(),
            None,
            "unknown fields are refused"
        );
        assert_eq!(
            serde_json::from_value::<OpRequest>(json!({"op": "gc"})).ok(),
            None
        );
    }

    #[test]
    fn op_outcome_shape() {
        let outcome = OpOutcome {
            op_id: 3,
            status: OpStatus::NeedsConfirmation,
            title: "Delete branch x".into(),
            message: "m".into(),
            detail: None,
            log_from: 0,
            log_to: 0,
            dangers: vec![Danger {
                kind: DangerKind::DeleteUnmergedBranch,
                message: "x has 2 commits".into(),
                detail: None,
                token: None,
            }],
            hints: vec![
                OpHint::Push {
                    branch: "b".into(),
                    remote: "origin".into(),
                    set_upstream: true,
                },
                OpHint::Fetch,
                OpHint::Pull { branch: "b".into() },
                OpHint::ForcePush {
                    branch: "b".into(),
                    remote: "origin".into(),
                },
                OpHint::ChoosePullMode { branch: "b".into() },
            ],
            snapshot: RepoSnapshot {
                repo_id: 1,
                name: "r".into(),
                root_path: "/r".into(),
                active_worktree: "/r".into(),
                generation: 1,
                total_rows: 0,
                loading: false,
                max_lane: 0,
                head: HeadInfo::default(),
                branches: vec![],
                remote_branches: vec![],
                remotes: vec![],
                worktrees: vec![],
                status: WorktreeStatus::default(),
                view_options: ViewOptions {
                    branches: None,
                    show_remotes: true,
                    show_tags: true,
                    show_stashes: true,
                },
                fetch: FetchState::default(),
                op: None,
                in_progress: None,
                seq: 0,
            },
        };
        let v = serde_json::to_value(&outcome).unwrap();
        assert_eq!(v["opId"], 3);
        assert_eq!(v["status"], "needs_confirmation");
        assert_eq!(v["logFrom"], 0);
        assert_eq!(v["logTo"], 0);
        assert_eq!(v["detail"], Value::Null);
        assert_eq!(
            v["dangers"],
            json!([{"kind": "delete_unmerged_branch", "message": "x has 2 commits", "detail": null,
                    "token": null}])
        );
        assert_eq!(
            v["hints"],
            json!([
                {"kind": "push", "branch": "b", "remote": "origin", "setUpstream": true},
                {"kind": "fetch"},
                {"kind": "pull", "branch": "b"},
                {"kind": "force_push", "branch": "b", "remote": "origin"},
                {"kind": "choose_pull_mode", "branch": "b"}
            ])
        );
        assert_eq!(v["snapshot"]["op"], Value::Null);
        assert_eq!(serde_json::from_value::<OpOutcome>(v).unwrap(), outcome);
        for (status, name) in [
            (OpStatus::Done, "done"),
            (OpStatus::Stopped, "stopped"),
            (OpStatus::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), name);
        }
        for (kind, name) in [
            (DangerKind::ForcePush, "force_push"),
            (DangerKind::HardReset, "hard_reset"),
            (DangerKind::DeleteRemoteBranch, "delete_remote_branch"),
            (DangerKind::DropStash, "drop_stash"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
        for (kind, name) in [
            (InProgressKind::Merge, "merge"),
            (InProgressKind::Rebase, "rebase"),
            (InProgressKind::Revert, "revert"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
        assert_eq!(serde_json::to_value(RefKind::Stash).unwrap(), "stash");
        assert_eq!(serde_json::to_value(RowKind::Stash).unwrap(), "stash");
        assert_eq!(
            serde_json::to_value(RepoPrefs::default()).unwrap(),
            json!({"mergeNoFf": true})
        );
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
            conflicted_paths: vec!["a b.txt".into()],
        };
        let v = serde_json::to_value(&st).unwrap();
        assert_eq!(
            v,
            json!({"changed": 1, "untracked": 2, "conflicted": 3, "conflictedPaths": ["a b.txt"]})
        );
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
            shallow_boundary: false,
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
                "message": "m", "shallowBoundary": false,
                "files": [{"path": "new", "oldPath": "old", "status": "R",
                           "additions": null, "deletions": 0}]
            })
        );
        assert_eq!(serde_json::from_value::<CommitDetails>(v).unwrap(), d);
    }

    #[test]
    fn repo_snapshot_shape() {
        let snap = RepoSnapshot {
            repo_id: 7,
            name: "r".into(),
            root_path: "/r".into(),
            active_worktree: "/w".into(),
            generation: 3,
            total_rows: 10,
            loading: true,
            max_lane: 2,
            head: HeadInfo {
                branch: None,
                hash: Some("h".into()),
            },
            branches: vec!["main".into()],
            remote_branches: vec!["origin/main".into()],
            remotes: vec!["origin".into()],
            worktrees: vec![],
            status: WorktreeStatus::default(),
            view_options: ViewOptions {
                branches: None,
                show_remotes: true,
                show_tags: true,
                show_stashes: true,
            },
            fetch: FetchState::default(),
            op: Some(RunningOp {
                id: 4,
                title: "Create branch x".into(),
                started_at_ms: 9,
                queued: 1,
            }),
            in_progress: Some(InProgress {
                kind: InProgressKind::CherryPick,
                conflicted: vec!["a b.txt".into()],
                unresolved: vec![],
                step: None,
                total: None,
                subject: Some("fix".into()),
            }),
            seq: 12,
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(
            v,
            json!({
                "repoId": 7, "name": "r", "rootPath": "/r", "activeWorktree": "/w",
                "generation": 3, "totalRows": 10, "loading": true, "maxLane": 2,
                "head": {"branch": null, "hash": "h"},
                "branches": ["main"], "remoteBranches": ["origin/main"], "remotes": ["origin"],
                "worktrees": [], "status": {"changed": 0, "untracked": 0, "conflicted": 0,
                           "conflictedPaths": []},
                "viewOptions": {"branches": null, "showRemotes": true, "showTags": true,
                                "showStashes": true},
                "fetch": {"running": false, "lastError": null, "lastErrorDetail": null, "lastAtMs": null},
                "op": {"id": 4, "title": "Create branch x", "startedAtMs": 9, "queued": 1},
                "inProgress": {"kind": "cherry-pick", "conflicted": ["a b.txt"], "unresolved": [],
                               "step": null,
                               "total": null, "subject": "fix"},
                "seq": 12
            })
        );
        assert_eq!(serde_json::from_value::<RepoSnapshot>(v).unwrap(), snap);
    }

    #[test]
    fn rows_page_shape() {
        let page = RowsPage {
            generation: 2,
            start: 5,
            rows: vec![CommitRow {
                index: 5,
                kind: RowKind::Uncommitted,
                hash: "UNCOMMITTED".into(),
                parents: vec!["p".into()],
                author: "A".into(),
                email: "a@x".into(),
                time: 9,
                subject: "s".into(),
                lane: 1,
                color: 4,
                edges: vec![Edge {
                    from: 1,
                    to: 0,
                    color: 4,
                    half: EdgeHalf::Bottom,
                }],
                labels: vec![RefLabel {
                    kind: RefKind::Branch,
                    name: "main".into(),
                    remotes: vec!["origin".into()],
                    current: true,
                    worktree_path: Some("/r".into()),
                    upstream: Some("origin/main".into()),
                    ahead: 1,
                    behind: 2,
                }],
                is_head: false,
                is_merge: true,
                detached_worktrees: vec!["/d".into()],
            }],
        };
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            json!({
                "generation": 2, "start": 5,
                "rows": [{
                    "index": 5, "kind": "uncommitted", "hash": "UNCOMMITTED", "parents": ["p"],
                    "author": "A", "email": "a@x", "time": 9, "subject": "s",
                    "lane": 1, "color": 4,
                    "edges": [{"from": 1, "to": 0, "color": 4, "half": "bottom"}],
                    "labels": [{"kind": "branch", "name": "main", "remotes": ["origin"],
                                "current": true, "worktreePath": "/r",
                                "upstream": "origin/main", "ahead": 1, "behind": 2}],
                    "isHead": false, "isMerge": true, "detachedWorktrees": ["/d"]
                }]
            })
        );
        assert_eq!(serde_json::to_value(RowKind::Commit).unwrap(), "commit");
        assert_eq!(serde_json::to_value(RefKind::Tag).unwrap(), "tag");
    }
}
