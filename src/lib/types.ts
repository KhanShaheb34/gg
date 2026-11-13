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
    | "io";
  message: string;
  detail: string | null;
};

export type ViewOptions = { branches: string[] | null; showRemotes: boolean; showTags: boolean }; // branches null = all
export type WorktreeInfo = {
  path: string;
  head: string | null;
  branch: string | null;
  isMain: boolean;
  locked: boolean;
  prunable: boolean;
};
export type WorktreeStatus = { changed: number; untracked: number; conflicted: number };
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
};

export type Edge = { from: number; to: number; color: number; half: "top" | "bottom" | "full" };
export type RefLabel = {
  kind: "branch" | "remote" | "tag";
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
  kind: "commit" | "uncommitted";
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

export type Settings = { theme: "system" | "light" | "dark"; fetchIntervalSecs: number }; // default system, 300; 0 = off
export type Recent = { path: string; name: string; lastOpenedMs: number };
export type AppState = {
  recents: Recent[];
  session: { tabs: string[]; active: string | null };
  settings: Settings;
  launchPath: string | null;
  homeDir: string | null; // the user's home folder, for showing paths as ~/…
  settingsProblem: string | null; // why settings are not being saved (unreadable file); null when they are
};

/** One git command the core ran (`command_log`); `args` excludes the injected `-c` options. */
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
};
