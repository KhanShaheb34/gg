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
  field?: InputField; // the form field it is about, when the core knows (create worktree)
};

/** A field of the form a request came from, which a refusal or error is about. */
export type InputField = "location" | "name";

export type ViewOptions = { branches: string[] | null; showRemotes: boolean; showTags: boolean; showStashes: boolean }; // branches null = all
export type WorktreeInfo = {
  path: string;
  head: string | null;
  branch: string | null;
  isMain: boolean;
  locked: boolean;
  prunable: boolean;
  active: boolean; // every action runs in the active worktree
  status: WorktreeStatus | null; // null until read; the active one's is the snapshot's `status`
  upstream: { name: string; ahead: number; behind: number } | null; // null = no upstream (or detached)
  unpushed: number | null; // commits of its HEAD on no remote-tracking branch; null = unknown
  inProgress: InProgress["kind"] | null;
  lockReason: string | null; // when locked with a reason
  pr: PullRequest | null; // its branch's open pull request, once loaded
};
export type PullRequest = { number: number; title: string; url: string; state: "open" | "draft"; head: string; base: string };
/**
 * The repository's open pull requests (loaded through `gh` with a fetch or a refresh the user
 * asks for, at most once a minute). `ready` with reason `failed`: the last reload failed and the
 * list shown is as of `loadedAtMs` (dim it by age). `partial`: 1000 were listed, some markers
 * may be missing.
 */
export type PrState = {
  status: "unknown" | "loading" | "ready" | "unavailable";
  reason: PrUnavailable | null;
  loadedAtMs: number | null;
  partial: boolean;
};
export type PrUnavailable = "no_gh" | "signed_out" | "not_github" | "failed";
/** What `pr_suggest` offers for a new pull request; `existing` asked of gh for that branch. */
export type PrSuggestion = {
  title: string;
  body: string;
  base: string;
  baseRemote: string | null; // the remote `base` is on: the repository the pull request goes to (null: no remote)
  existing: PullRequest | null;
  compareUrl: string | null;
  /** The base repository, "owner/name" (null: its remote names none). */
  repo: string | null;
  /** Where `pr_create` pushes the branch ("fork", "fork/x") and whether it must first: worked out as it will (null: not clear). */
  pushRemote: string | null;
  pushTo: string | null;
  needsPush: boolean;
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
  /** The uncommitted row's subject for `status` ("Uncommitted changes (3)"); rows read before a count changed keep the old one. */
  uncommittedSubject: string;
  viewOptions: ViewOptions;
  fetch: FetchState;
  /** The operation running on this repository; `queued`: requests waiting behind it. */
  op: { id: number; title: string; startedAtMs: number; queued: number } | null;
  prs: PrState;
  inProgress: InProgress | null;
  /** Grows by one per repository with each snapshot that changes anything and each operation outcome; drop older ones. */
  seq: number;
};

/**
 * What `open_repo` answers: the repository's snapshot. A folder in a linked worktree makes
 * that worktree active; `openedWorktree` is that worktree when it could not be made active
 * (an operation runs or waits in the repository): the tab goes on in `activeWorktree`.
 */
export type OpenedRepo = RepoSnapshot & { openedWorktree: string | null };

/** A merge, rebase, cherry-pick or revert git stopped in. */
export type InProgress = {
  kind: "merge" | "rebase" | "cherry-pick" | "revert";
  files: ConflictFile[]; // the conflicted files, relative to the worktree, sorted: the first 200
  fileCount: number; // how many are conflicted in all
  step: number | null; // rebase only
  total: number | null; // rebase only
  subject: string | null; // what is being applied: the incoming branch or the commit's subject
  // what comes in: merge, the branch or ref merged; rebase, what it goes onto; cherry-pick/revert, the 8-char hash
  incoming: string | null;
  rebasing: string | null; // rebase only: the branch being rebased
  // rebase, cherry-pick or revert: the commit git stopped on, in full (what a skip is for)
  commit: string | null;
  // that commit has no changes left (nothing conflicted, everything as HEAD has it): continuing
  // would be refused as empty; skip it, or abort
  empty: boolean;
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
  pr: PullRequest | null; // branch and remote branch: its open pull request (in full: the marker shows and opens it)
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

/**
 * Defaults: system, 300 (0 = off), null (the system's default opener), null (the platform's terminal), "../{repo}.worktrees/{branch}".
 * `worktreePattern`: where new worktrees go, relative to the main worktree; {repo} {branch} filled in, must contain {branch}
 * and lead outside the main worktree (a suggestion that would land inside a worktree or the git dir uses the default).
 */
export type Settings = {
  theme: "system" | "light" | "dark";
  fetchIntervalSecs: number;
  editorCommand: string | null;
  /** Opens a terminal in a folder: `{path}` stands for it, else it is added last; null = the platform's terminal. */
  terminalCommand: string | null;
  /** Where new worktrees go by default, relative to the main worktree: {repo}, {branch}. */
  worktreePattern: string;
  /** How dates are shown (see format.ts `DateFormat`). */
  dateFormat: "standard" | "relative" | "iso" | "locale";
};
/** What `open_external` opens: a worktree (`path`; the active one when omitted) or an `https://` page. */
export type ExternalRequest = { target: "editor" | "terminal" | "reveal"; path?: string } | { target: "url"; url: string };
/**
 * Kept per repository. Defaults: mergeNoFf true, sidePanel true. `activeWorktree`: the worktree
 * remembered as active (null = the main one); set by the core. `sidePanel`: the side panel
 * shows while the window is wide enough to dock it.
 */
export type RepoPrefs = { mergeNoFf: boolean; activeWorktree: string | null; sidePanel: boolean; prBase: string | null };
export type Recent = { path: string; name: string; lastOpenedMs: number };
export type AppState = {
  recents: Recent[];
  session: { tabs: string[]; active: string | null };
  settings: Settings;
  launchPath: string | null;
  homeDir: string | null; // the user's home folder, for showing paths as ~/…
  settingsProblem: string | null; // why settings are not being saved (unreadable file); null when they are
  settingsFile: string; // the file settings are kept in (it may not exist until something is saved)
  gitProblem: string | null; // the git found is too old (before 2.29), said once; null when it is not
  cli: CliState; // the `gg` terminal command
  version: string; // gg's own ("0.1.0")
};

/** The `gg` terminal command (`install_cli`, `uninstall_cli`). */
export type CliState = {
  installed: string | null; // the launcher the app wrote, first on the login PATH; null when none
  shadowedBy: string | null; // what the login shell runs for `gg` instead ("an alias for …"); null when nothing or not known
  candidates: string[]; // folders it can be written to, best first (on PATH, writable, not in a repository)
};

/**
 * One git command the core ran (`command_log`); `args` excludes the injected `-c` options. The
 * userinfo of URLs in `args`, `stdout` and `stderr` is masked by the core ("https://***@host/…"),
 * as in every message, outcome, event and error it hands the UI.
 */
export type CommandRecord = {
  id: number;
  program: string; // "git", or another tool the core ran ("gh")
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

/** What HEAD points at: the branch (null when detached) and the commit (null before the first). */
export type HeadInfo = RepoSnapshot["head"];

/**
 * One operation for `run_op`. A request on the branch checked out carries `expectHead`: HEAD as
 * the UI showed it when the user asked; the core refuses it, running nothing, if another
 * branch is checked out by its turn (reset, continue, abort and skip: also if the commit moved).
 * Every request that runs in a working directory (all but push, push_tag, delete_tag,
 * delete_remote_branch and fetch_into_local) carries `expectWorktree`: the `activeWorktree` of
 * the snapshot it was asked from; the core refuses it if another worktree is active by its turn.
 */
export type OpRequest =
  | { op: "checkout_branch"; branch: string; fastForward: boolean; expectWorktree: string }
  | { op: "checkout_remote"; remoteBranch: string; localName: string; expectWorktree: string }
  | { op: "checkout_commit"; hash: string; expectWorktree: string }
  | { op: "create_branch"; name: string; at: string; checkout: boolean; expectWorktree: string }
  | { op: "rename_branch"; branch: string; newName: string; expectWorktree: string }
  | { op: "delete_branch"; branch: string; expectWorktree: string }
  | { op: "merge"; target: MergeTarget; noFf: boolean; squash: boolean; noCommit: boolean; expectHead: HeadInfo; expectWorktree: string }
  | { op: "rebase"; onto: string; expectHead: HeadInfo; expectWorktree: string } // onto: local or remote branch name
  | { op: "cherry_pick"; hash: string; mainline: number | null; noCommit: boolean; expectHead: HeadInfo; expectWorktree: string }
  | { op: "revert"; hash: string; mainline: number | null; expectHead: HeadInfo; expectWorktree: string }
  | { op: "reset"; hash: string; mode: "soft" | "mixed" | "hard"; expectHead: HeadInfo; expectWorktree: string }
  | { op: "create_tag"; name: string; hash: string; message: string | null; expectWorktree: string }
  | { op: "delete_tag"; name: string; remote: string | null } // remote set = also delete there
  | { op: "push_tag"; name: string; remote: string }
  // to: null = the branch of the same name on the remote; else the branch's upstream there,
  // picked in the Push dialog. An existing upstream is never changed; one is set if there is none.
  // force: with lease, the force_push danger's token once confirmed
  | { op: "push"; branch: string; remote: string; to: string | null; force: boolean }
  | { op: "pull"; branch: string; mode: "default" | "merge" | "rebase"; expectHead: HeadInfo; expectWorktree: string }
  | { op: "fetch_into_local"; remoteBranch: string; localName: string }
  | { op: "delete_remote_branch"; remote: string; branch: string }
  | { op: "stash_save"; message: string | null; includeUntracked: boolean; expectWorktree: string }
  | { op: "stash_apply" | "stash_pop"; stash: string; expectHead: HeadInfo; expectWorktree: string } // stash commit hash
  | { op: "stash_drop"; stash: string; expectWorktree: string }
  | { op: "stash_branch"; stash: string; name: string; expectWorktree: string }
  | { op: "continue"; evenWithMarkers: boolean; expectHead: HeadInfo; expectWorktree: string } // R31: true = go ahead though files still have conflict markers
  | { op: "abort"; expectHead: HeadInfo; expectWorktree: string } // always the abort danger first
  // leaves out the commit a rebase, cherry-pick or revert stopped on (not a merge): `commit`, else
  // refused; a rebase's skip with other uncommitted changes asks first (skip_discards)
  | { op: "skip"; expectHead: HeadInfo; commit: string; expectWorktree: string }
  // The worktree operations run from the main worktree and name their target (no expectWorktree).
  // path: absolute or relative to the main worktree; a new or empty folder; the new worktree is not made active
  | { op: "worktree_add"; path: string; branch: WorktreeBranch }
  // path: a linked worktree git lists (re-checked in its turn); its work is confirmed first; the active one: main becomes active first
  | {
      op: "worktree_remove";
      path: string;
      deleteBranch: boolean;
      /** The branch and commit the dialog showed: the core deletes nothing if the worktree there is on others now. */
      expectHead: HeadInfo;
    }
  | { op: "worktree_prune" }
  // Commits exactly `paths` (as worktree_files lists them; either side of a rename stands for both) as they
  // are in the working tree in its turn, in a fresh temporary index: other changes, staged or not, stay as
  // they are. push: then pushes the branch to the same-named branch on its remote. While a merge with
  // no conflicts left is in progress, it concludes the merge, and `paths` must be every changed file.
  // empty: no paths, and a commit that changes nothing (git commit --allow-empty); refused while a merge
  // is in progress. Empty paths without it: nothing_to_do; blank message: bad_request.
  | { op: "commit"; paths: string[]; message: string; push: boolean; empty: boolean; expectHead: HeadInfo; expectWorktree: string }
  // base: a branch on baseRemote, the repository it goes to (null: the one pr_suggest names); every gh command names it
  | { op: "remove_index_lock"; expectWorktree: string } // the active worktree's stale index.lock, once confirmed
  | {
      op: "pr_create";
      branch: string;
      base: string;
      baseRemote: string | null;
      title: string;
      body: string;
      draft: boolean;
      /** The dialog promised GitHub's compare page (no form): the core only pushes, and never runs gh. */
      comparePage: boolean;
    };

/** What a new worktree checks out: an existing local branch, a new branch at a commit (full hash), or a commit, detached. */
export type WorktreeBranch = { existing: string } | { create: string; at: string } | { detach: string };
/**
 * One changed file of a worktree (`worktree_files`): `status` is how the working tree differs from HEAD
 * ("?" untracked, "U" conflicted; "D" also for a file added to the index and deleted since); `origPath`
 * the other side of a rename or copy; `staged`: the index differs from HEAD; `unstaged`: the working
 * tree differs from the index. `committable` false (with `reason`): it cannot be ticked, its name not
 * being valid UTF-8; the core refuses a commit that ticks it.
 */
export type FileEntry = {
  path: string;
  origPath: string | null;
  status: "M" | "A" | "D" | "R" | "C" | "T" | "U" | "?";
  staged: boolean;
  unstaged: boolean;
  committable: boolean;
  reason: string | null;
};
/** `worktree_files`: the changed files sorted by path, the first 5000; `truncated` when there are more. */
export type WorktreeFiles = {
  files: FileEntry[];
  truncated: boolean;
  mergeMessage: string | null; // while a merge is stopped there: git's message for it, comment lines removed
  /** Where Commit & Push sends the branch: git's push remote for it, as Create pull request pushes it (null: detached, or not clear). */
  pushRemote: string | null;
};
/** `worktree_default_path`: the first free folder the pattern gives; `exists`: it is there, empty. */
export type WorktreePath = { path: string; exists: boolean };

export type DangerKind =
  | "force_push"
  | "hard_reset"
  | "delete_unmerged_branch"
  | "delete_remote_branch"
  | "drop_stash"
  | "abort"
  | "skip_discards"
  | "remove_worktree_with_work" // red: changed or untracked files go (--force), or a detached HEAD's commits
  | "remove_worktree_ignored_files" // an ordinary confirmation, not red: only ignored files, which git deletes
  | "delete_unmerged_branch_with_worktree" // red: the branch deleted after its worktree
  | "prune_worktrees" // an ordinary confirmation: the stale entries prune forgets
  | "prune_worktrees_loses_commits" // red: a stale detached entry's commits on no branch, tag or remote go
  | "remove_index_lock"; // red: a stale index.lock; another git or an editor may still be using the repository
/**
 * token: the state the message was worked out from; confirming sends it back with the
 * kind (`Confirmed`), and the core asks again if the state has changed since.
 */
export type Danger = { kind: DangerKind; message: string; detail: string | null; token: string | null };
/** A danger the user confirmed, as `run_op`'s `confirmed` lists it. */
export type Confirmed = { kind: DangerKind; token: string | null };
export type OpHint =
  | { kind: "push"; branch: string; remote: string } // to the branch of the same name there
  | { kind: "fetch" }
  | { kind: "pull"; branch: string }
  | { kind: "force_push"; branch: string; remote: string; to: string | null } // where the rejected push went
  | { kind: "choose_pull_mode"; branch: string }
  | { kind: "skip"; commit: string } // continuing came out empty: skip that commit
  | { kind: "make_active"; worktree: string } // the branch needed is checked out in that worktree
  | { kind: "open_url"; url: string; label: string } // an https page: the pull request, or the compare page
  | { kind: "remove_index_lock"; path: string; worktree: string }; // a stale staging-area lock of that worktree

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
  created: string | null; // worktree_add: the worktree it made, as the snapshot lists it
  committed: string | null; // commit: the commit it made (full hash), whatever came after it
  field: InputField | null; // a refusal about one field of the form it came from (worktree_add)
  snapshot: RepoSnapshot; // after the operation and its refresh; after pull / fetch_into_local maybe before the log reload (a repo-changed with a higher seq follows)
};

/** `op-progress` event: the latest progress line of a network operation (at most 10 a second). */
export type OpProgress = { repoId: number; opId: number; line: string };
