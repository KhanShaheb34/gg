// Types that cross the boundary to the core. They mirror crates/gg-core/src/dto.rs (and the
// Boundary Types of the plan) exactly; change both together.

export type CoreErrorDto = {
  code:
    | "not_a_repo"
    | "bare_repo"
    | "path_missing"
    | "git_not_found"
    | "git_failed"
    | "unknown_repo"
    | "stale_generation"
    | "bad_request"
    | "io"
    | "bad_name" // a branch or tag name git would not take, or one that is taken
    | "busy"
    | "nothing_to_do";
  message: string;
  detail: string | null;
};

export type ViewOptions = { branches: string[] | null; showRemotes: boolean; showTags: boolean; showStashes: boolean }; // branches null = all
export type WorktreeInfo = {
  path: string;
  head: string | null;
  branch: string | null;
  isMain: boolean;
  locked: boolean;
  prunable: boolean;
};
/** `conflictedPaths`: the conflicted paths, sorted, the first 200 of them. */
export type WorktreeStatus = { changed: number; untracked: number; conflicted: number; conflictedPaths: string[] };
/** `lastError`: why the last fetch failed, in one line; `lastErrorDetail`: git's whole stderr of it. */
export type FetchState = {
  running: boolean;
  lastError: string | null;
  lastErrorDetail: string | null;
  lastAtMs: number | null;
};

export type RepoSnapshot = {
  repoId: number;
  name: string;
  rootPath: string;
  activeWorktree: string;
  generation: number;
  totalRows: number;
  loading: boolean;
  maxLane: number;
  head: { branch: string | null; hash: string | null };
  branches: string[];
  remoteBranches: string[];
  remotes: string[];
  worktrees: WorktreeInfo[];
  status: WorktreeStatus;
  viewOptions: ViewOptions;
  fetch: FetchState;
  /** The operation running on this repository; `queued`: requests waiting behind it. */
  op: { id: number; title: string; startedAtMs: number; queued: number } | null;
  inProgress: InProgress | null;
  /** Grows by one per repository with each snapshot that changes anything and each operation outcome; drop older ones. */
  seq: number;
};

/** A merge, rebase, cherry-pick or revert git stopped in. */
export type InProgress = {
  kind: "merge" | "rebase" | "cherry-pick" | "revert";
  files: ConflictFile[]; // the conflicted files, relative to the worktree, sorted: the first 200
  fileCount: number; // how many are conflicted in all
  step: number | null; // rebase only
  total: number | null; // rebase only
  subject: string | null; // what is being applied: the incoming branch or the commit's subject
  // what comes in (R32): merge, the branch or ref merged; rebase, what it goes onto; cherry-pick/revert, the 8-char hash
  incoming: string | null;
  rebasing: string | null; // rebase only: the branch being rebased
};

/**
 * A conflicted file. `markers`: it still has a conflict-marker line; `undecided`: git kept one
 * side (binary, deleted or added on one side) or it cannot be judged; `resolved`: no marker left.
 */
export type ConflictFile = { path: string; state: "markers" | "undecided" | "resolved" };

export type Edge = { from: number; to: number; color: number; half: "top" | "bottom" | "full" };
export type RefLabel = {
  kind: "branch" | "remote" | "tag" | "stash"; // stash: name "stash@{0}"
  name: string;
  remotes: string[]; // branch only: remotes whose same-named branch is at this commit
  current: boolean; // checked out in the active worktree
  worktreePath: string | null; // checked out in this (any) worktree
  upstream: string | null;
  ahead: number;
  behind: number;
};
export type CommitRow = {
  index: number;
  kind: "commit" | "uncommitted" | "stash";
  hash: string;
  parents: string[];
  author: string;
  email: string;
  time: number; // author time, unix seconds
  subject: string;
  lane: number;
  color: number;
  edges: Edge[];
  labels: RefLabel[];
  isHead: boolean;
  isMerge: boolean;
  detachedWorktrees: string[]; // worktree paths with detached HEAD at this commit
};
export type RowsPage = { generation: number; start: number; rows: CommitRow[] };

export type FileChange = {
  path: string;
  oldPath: string | null;
  status: "A" | "M" | "D" | "R" | "C" | "T";
  additions: number | null;
  deletions: number | null;
};
/** Rows of `generation` whose commit message (subject and body) matches a search. */
export type BodyHits = { generation: number; rows: number[] };

export type CommitDetails = {
  hash: string;
  parents: string[];
  author: string;
  authorEmail: string;
  authorTime: number;
  committer: string;
  committerEmail: string;
  commitTime: number;
  message: string;
  files: FileChange[];
  /** No parents because a shallow clone's history is cut here (it is no root commit). */
  shallowBoundary: boolean;
};

/** Defaults: system, 300 (0 = off), null (the system's default opener). */
export type Settings = { theme: "system" | "light" | "dark"; fetchIntervalSecs: number; editorCommand: string | null };
/** Kept per repository. Default: mergeNoFf true. */
export type RepoPrefs = { mergeNoFf: boolean };
export type Recent = { path: string; name: string; lastOpenedMs: number };
export type AppState = {
  recents: Recent[];
  session: { tabs: string[]; active: string | null };
  settings: Settings;
  launchPath: string | null;
  homeDir: string | null; // the user's home folder, for showing paths as ~/…
  settingsProblem: string | null; // why settings are not being saved (unreadable file); null when they are
};

/**
 * One git command the core ran (`command_log`); `args` excludes the injected `-c` options. The
 * userinfo of URLs in `args`, `stdout` and `stderr` is masked by the core ("https://***@host/…"),
 * as in every message, outcome, event and error it hands the UI (R36).
 */
export type CommandRecord = {
  id: number;
  kind: "read" | "write";
  cwd: string;
  args: string[];
  startedAtMs: number;
  durationMs: number;
  exitCode: number;
  stdout: string;
  stderr: string;
  repoId: number | null; // the repository it ran for
  opId: number | null; // the operation it was part of
  opTitle: string | null; // that operation's title, once it has one
};

export type MergeTarget = { kind: "branch" | "remote" | "commit"; name: string }; // name: branch, "origin/x" or full hash

/** One operation for `run_op`. */
export type OpRequest =
  | { op: "checkout_branch"; branch: string; fastForward: boolean }
  | { op: "checkout_remote"; remoteBranch: string; localName: string }
  | { op: "checkout_commit"; hash: string }
  | { op: "create_branch"; name: string; at: string; checkout: boolean }
  | { op: "rename_branch"; branch: string; newName: string }
  | { op: "delete_branch"; branch: string }
  | { op: "merge"; target: MergeTarget; noFf: boolean; squash: boolean; noCommit: boolean }
  | { op: "rebase"; onto: string } // local or remote branch name
  | { op: "cherry_pick"; hash: string; mainline: number | null; noCommit: boolean }
  | { op: "revert"; hash: string; mainline: number | null }
  | { op: "reset"; hash: string; mode: "soft" | "mixed" | "hard" }
  | { op: "create_tag"; name: string; hash: string; message: string | null }
  | { op: "delete_tag"; name: string; remote: string | null } // remote set = also delete there
  | { op: "push_tag"; name: string; remote: string }
  // to: null = the branch of the same name on the remote (R38); else the branch's upstream there,
  // picked in the Push dialog. An existing upstream is never changed; one is set if there is none.
  // lease: with force, the token of the force_push danger the user confirmed (R11)
  | { op: "push"; branch: string; remote: string; to: string | null; force: boolean; lease: string | null }
  | { op: "pull"; branch: string; mode: "default" | "merge" | "rebase" }
  | { op: "fetch_into_local"; remoteBranch: string; localName: string }
  | { op: "delete_remote_branch"; remote: string; branch: string }
  | { op: "stash_save"; message: string | null; includeUntracked: boolean }
  | { op: "stash_apply" | "stash_pop" | "stash_drop"; stash: string } // stash commit hash
  | { op: "stash_branch"; stash: string; name: string }
  | { op: "continue"; evenWithMarkers: boolean } // R31: true = go ahead though files still have conflict markers
  | { op: "abort" }
  | { op: "skip" }; // leaves out the commit a rebase, cherry-pick or revert stopped on (not a merge)

export type DangerKind = "force_push" | "hard_reset" | "delete_unmerged_branch" | "delete_remote_branch" | "drop_stash";
// token: force_push only, the remote-tracking commit the message was worked out from (null without one);
// send it back as the push's lease to confirm.
export type Danger = { kind: DangerKind; message: string; detail: string | null; token: string | null };
export type OpHint =
  | { kind: "push"; branch: string; remote: string } // to the branch of the same name there
  | { kind: "fetch" }
  | { kind: "pull"; branch: string }
  | { kind: "force_push"; branch: string; remote: string; to: string | null } // where the rejected push went
  | { kind: "choose_pull_mode"; branch: string }
  | { kind: "skip" }; // continuing came out empty: skip that commit

/** How `run_op` went. `stopped`: git stopped on conflicts (not a failure); `needs_confirmation`: nothing ran. */
export type OpOutcome = {
  opId: number;
  status: "done" | "stopped" | "failed" | "needs_confirmation";
  title: string; // "Merge feature/x into staging"
  message: string; // one plain line for the notice
  detail: string | null; // git's own output on failure
  logFrom: number; // command-log ids of the operation's writes (0, 0 if none ran)
  logTo: number;
  dangers: Danger[]; // non-empty only for needs_confirmation
  hints: OpHint[];
  snapshot: RepoSnapshot; // after the operation and its refresh; after pull / fetch_into_local maybe before the log reload (a repo-changed with a higher seq follows)
};

/** `op-progress` event: the latest progress line of a network operation (at most 10 a second). */
export type OpProgress = { repoId: number; opId: number; line: string };
