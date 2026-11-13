# Plan 1 — Foundation, Speed Proof, Read-Only Graph — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Tauri desktop app that opens git repositories (tabs, recents), draws a fast, polished, auto-refreshing commit graph with filters, search and commit details, and proves the speed targets on a 100,000-commit repository.

**Architecture:** A pure-Rust library crate (`gg-core`) owns all git work, graph layout, state, file watching and persistence, and exposes one entry point: `Core::dispatch(cmd, args) -> JSON`. Two thin transports call it: the Tauri app (one `invoke_core` command + events) and a dev-only HTTP server (`gg-devserver`) so the same UI runs in a normal browser where agents and Playwright can drive it. The Svelte UI holds no git logic; it pulls row windows on demand and renders only visible rows, with graph lines on a canvas.

**Tech Stack:** Rust 1.98, Tauri 2, tokio, serde, notify, axum (dev server only) · Svelte 5 + TypeScript + Vite, plain CSS custom properties · pnpm 10 · vitest, Playwright · system `git` 2.54.

**Spec:** `docs/superpowers/specs/2026-11-12-git-graph-desktop-app-design.md` — read sections 1–8 before starting any task.

**Roadmap (later plans, written when reached):** Plan 2 — actions, dialogs, operation queue, command-log panel, conflict banner, stash rows and stash actions (spec 5.2, 5.3, 5.6, 5.7). Plan 3 — worktree panel, active-worktree switching, commit/push/delete, working-tree file watching, pull requests, settings screen, other platforms (spec 5.4, 5.5, 5.8 settings, phase 5).

## Global Constraints

- Git is invoked only through `GitRunner`, as an argument array, never through a shell. Machine-readable output only (`--porcelain`, `-z`, explicit `--format`).
- Every git child process gets: `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, `GIT_EDITOR=true`, `GIT_PAGER=cat`, `LC_ALL=C`, stdin closed, and leading args `-c core.quotePath=false -c color.ui=false`.
- The UI never builds git commands and never parses git output. It calls `ipc.ts` wrappers only.
- No macOS-only API without a fallback. Paths are handled as paths (spaces, unicode).
- All git output is decoded lossily (`String::from_utf8_lossy`); no reader may panic on any input.
- JSON across the boundary is camelCase (`#[serde(rename_all = "camelCase")]`); enums are lowercase strings.
- Speed targets (spec §7): first screen of graph < 1 s after launch; 20k commits fully loaded < 2 s; 60 fps scrolling with no blank rows; click feedback < 100 ms; refresh after on-disk change visible < 1 s with no scroll jump; idle CPU ~0; memory < 150 MB with a 20k-commit repo; bundle < 20 MB.
- Frontend: Svelte 5 runes, TypeScript strict, no UI component library, no CSS framework. Theme via CSS custom properties on `:root[data-theme]`.
- Tests accompany every task; `cargo test --workspace`, `pnpm test` and `pnpm e2e` must pass before each commit that claims a task done.
- Commits: small, conventional-commit subjects (`feat:`, `test:`, `chore:`…). **Never add any AI attribution** — no `Co-Authored-By`, no session links, no "generated with" lines.
- Work happens on branch `dev`. Do not push.

## Review Focus

1. **Empty repository (no commits yet)** — opens normally and shows an empty-state message; no error, no spinner forever. *(Tests: Task 5, Task 9.)*
2. **Folder that is not a repository, or a recent whose path no longer exists** — a plain message; the app stays usable and the recent can be removed. *(Tests: Task 5, Task 8.)*
3. **Odd bytes and names** — non-UTF-8 author/subject, 5,000-character subject, branch names with slashes, unicode and emoji, repo path with spaces — nothing panics, text is shown lossily, labels stay intact. *(Tests: Task 3.)*
4. **App launched from Finder** — the process has a minimal `PATH`; git and tools used by hooks/credential helpers must still be found. *(Tests: Task 2.)*
5. **Detached HEAD and shallow clones (parents that are not in the log)** — graph still lays out and renders; dangling lanes simply end. *(Tests: Task 4, Task 5.)*

---

## File Structure

```
Cargo.toml                         workspace: crates/gg-core, crates/gg-devserver, src-tauri
crates/gg-core/src/
  lib.rs                           Core, EventSink, re-exports
  error.rs                         CoreError
  git/runner.rs                    GitRunner, CommandLog, PATH resolution
  git/log.rs                       Commit, LOG_FORMAT, LogParser, log_args
  git/refs.rs                      RefInfo, read_refs, read_remotes, read_head
  git/worktrees.rs                 WorktreeInfo, read_worktrees
  git/status.rs                    WorktreeStatus, read_status
  git/details.rs                   CommitDetails, read_details
  graph/layout.rs                  LaneAllocator, RowLayout, Edge
  repo/state.rs                    RepoState: commits, layout, refs, labels, rows, search
  repo/manager.rs                  open/close, load, refresh, generation, fetch, scheduler
  watcher.rs                       debounced git-dir watching
  store.rs                         recents, session, settings, repo prefs
  dispatch.rs                      cmd string → handler
  dto.rs                           every type that crosses the boundary
  testutil.rs                      TestRepo (cfg(any(test, feature="testutil")))
crates/gg-core/tests/              integration tests against real temp repos
crates/gg-devserver/src/main.rs    HTTP POST /invoke, SSE /events (dev/test only)
src-tauri/                         tauri.conf.json, capabilities/, src/lib.rs, src/main.rs
src/
  main.ts, App.svelte, app.css     shell, theme tokens
  lib/ipc.ts                       transport (Tauri or HTTP) + typed wrappers
  lib/types.ts                     TS mirror of dto.rs
  lib/state/app.svelte.ts          tabs, recents, settings, theme
  lib/state/repo.svelte.ts         per-repo snapshot, RowCache, selection, search
  lib/graph/rowCache.ts            paged row cache
  lib/graph/geometry.ts            lane/edge → canvas coordinates
  lib/graph/GraphView.svelte       virtual rows + canvas
  lib/graph/RefLabels.svelte
  lib/components/                  TabBar, Welcome, Toolbar, BranchFilter, SearchBox, CommitDetails, Splitter, Toast
e2e/                               Playwright specs + fixtures.ts (starts devserver on fixture repos)
scripts/gen-synthetic-repo.sh      large repo generator
scripts/perf.sh                    launches the built app, collects timings and memory
docs/perf/                         measured results
```

## Boundary Types (authoritative; `dto.rs` and `src/lib/types.ts` must match exactly)

```ts
type CoreErrorDto = { code: "not_a_repo"|"bare_repo"|"path_missing"|"git_not_found"|"git_failed"|"unknown_repo"|"stale_generation"|"bad_request"|"io"; message: string; detail: string|null }

type ViewOptions = { branches: string[]|null; showRemotes: boolean; showTags: boolean }   // branches null = all
type WorktreeInfo = { path: string; head: string|null; branch: string|null; isMain: boolean; locked: boolean; prunable: boolean }
type WorktreeStatus = { changed: number; untracked: number; conflicted: number }
type FetchState = { running: boolean; lastError: string|null; lastAtMs: number|null }

type RepoSnapshot = {
  repoId: number; name: string; rootPath: string; activeWorktree: string;
  generation: number; totalRows: number; loading: boolean; maxLane: number;
  head: { branch: string|null; hash: string|null };
  branches: string[]; remoteBranches: string[]; remotes: string[];
  worktrees: WorktreeInfo[]; status: WorktreeStatus;
  viewOptions: ViewOptions; fetch: FetchState;
}

type Edge = { from: number; to: number; color: number; half: "top"|"bottom"|"full" }
type RefLabel = {
  kind: "branch"|"remote"|"tag"; name: string;
  remotes: string[];            // branch only: remotes whose same-named branch is at this commit
  current: boolean;             // checked out in the active worktree
  worktreePath: string|null;    // checked out in this (any) worktree
  upstream: string|null; ahead: number; behind: number;
}
type CommitRow = {
  index: number; kind: "commit"|"uncommitted"; hash: string; parents: string[];
  author: string; email: string; time: number;        // author time, unix seconds
  subject: string; lane: number; color: number; edges: Edge[];
  labels: RefLabel[]; isHead: boolean; isMerge: boolean;
  detachedWorktrees: string[];  // worktree paths with detached HEAD at this commit
}
type RowsPage = { generation: number; start: number; rows: CommitRow[] }

type FileChange = { path: string; oldPath: string|null; status: "A"|"M"|"D"|"R"|"C"|"T"; additions: number|null; deletions: number|null }
type CommitDetails = {
  hash: string; parents: string[]; author: string; authorEmail: string; authorTime: number;
  committer: string; committerEmail: string; commitTime: number; message: string; files: FileChange[];
}

type Settings = { theme: "system"|"light"|"dark"; fetchIntervalSecs: number }   // default system, 300; 0 = off
type Recent = { path: string; name: string; lastOpenedMs: number }
type AppState = { recents: Recent[]; session: { tabs: string[]; active: string|null }; settings: Settings; launchPath: string|null }
```

**Commands** (`dispatch(cmd, args)`; args are a JSON object):

| cmd | args | returns |
|---|---|---|
| `get_app_state` | `{}` | `AppState` |
| `save_session` | `{tabs, active}` | `null` |
| `update_settings` | `{patch: Partial<Settings>}` | `Settings` |
| `remove_recent` | `{path}` | `Recent[]` |
| `open_repo` | `{path}` | `RepoSnapshot` (same `repoId` if that repository is already open) |
| `close_repo` | `{repoId}` | `null` |
| `get_snapshot` | `{repoId}` | `RepoSnapshot` |
| `get_rows` | `{repoId, generation, start, count}` | `RowsPage`; `stale_generation` error if generation is old |
| `find_row` | `{repoId, hash}` | `number|null` |
| `search_commits` | `{repoId, query}` | `number[]` (ascending row indices) |
| `commit_details` | `{repoId, hash}` | `CommitDetails` |
| `set_view_options` | `{repoId, options}` | `RepoSnapshot` |
| `fetch` | `{repoId}` | `null` (progress via events) |
| `refresh` | `{repoId}` | `RepoSnapshot` |
| `set_window_focused` | `{focused}` | `null` |
| `command_log` | `{sinceId}` | `CommandRecord[]` |
| `perf_mark` | `{name, ms}` | `null` |

**Events** (`EventSink::emit(name, payload)`): `repo-changed` → `RepoSnapshot` (sent on load progress, refresh, fetch state change, view-option change); `repo-error` → `{repoId, error: CoreErrorDto}`.

---

### Task 1: Workspace skeleton that builds, tests and opens a window

**Files:**
- Create: `Cargo.toml`, `crates/gg-core/{Cargo.toml,src/lib.rs}`, `crates/gg-devserver/{Cargo.toml,src/main.rs}`, `src-tauri/**`, `package.json`, `pnpm-workspace.yaml` (only if needed), `vite.config.ts`, `svelte.config.js`, `tsconfig.json`, `index.html`, `src/{main.ts,App.svelte,app.css}`, `playwright.config.ts`, `e2e/smoke.spec.ts`, `src/lib/smoke.test.ts`, `.gitignore`, `README.md`

**Interfaces:**
- Produces: `pnpm dev` (vite on port 1420), `pnpm test` (vitest), `pnpm e2e` (Playwright, chromium), `pnpm tauri dev`, `pnpm tauri build --debug`, `cargo test --workspace`. Tauri identifier `dev.gitgraph.desktop`, product name `GitGraph`, window 1280×800, min 800×500. Cargo release profile: `opt-level = 3`, `lto = "thin"`, `strip = true`, `panic = "abort"`.

- [ ] **Step 1:** Create branch `dev` from `main` and switch to it.
- [ ] **Step 2:** Scaffold the three crates and the Svelte 5 + TS + Vite frontend (use `pnpm create tauri-app`-equivalent files by hand or scaffold into a temp dir and move; the result must match the File Structure above). `gg-core` exposes `pub fn version() -> &'static str` with a unit test. `App.svelte` renders the text `GitGraph` in an element with `data-testid="app-root"`.
- [ ] **Step 3:** Create `src/lib/format.ts` with `shortHash(hash: string): string` returning the first 8 characters, and `src/lib/smoke.test.ts` (vitest) asserting `shortHash("471663edabcdef") === "471663ed"`.
- [ ] **Step 4:** Write `e2e/smoke.spec.ts`: Playwright starts `pnpm dev` via `webServer`, opens `/`, expects `[data-testid=app-root]` visible.
- [ ] **Step 5:** Run `cargo test --workspace`, `pnpm test`, `pnpm e2e`. Expected: all pass.
- [ ] **Step 6:** Run `pnpm tauri build --debug`. Expected: an app bundle is produced; launch the binary, confirm the process is still alive after 3 seconds, then quit it.
- [ ] **Step 7:** `README.md`: the dev commands above, nothing else. Commit: `chore: scaffold tauri workspace with core, devserver and svelte ui`.

### Task 2: Test repositories, git runner, command log

**Files:**
- Create: `crates/gg-core/src/{error.rs,testutil.rs,git/mod.rs,git/runner.rs}`
- Test: unit tests in `runner.rs`; `crates/gg-core/tests/runner.rs`

**Interfaces:**
- Produces:

```rust
// error.rs
#[derive(Debug, Clone, Serialize, thiserror::Error)] #[serde(rename_all="camelCase")]
pub struct CoreError { pub code: ErrorCode, pub message: String, pub detail: Option<String> }
pub enum ErrorCode { NotARepo, BareRepo, PathMissing, GitNotFound, GitFailed, UnknownRepo, StaleGeneration, BadRequest, Io } // serde snake_case
impl CoreError { pub fn new(code: ErrorCode, message: impl Into<String>) -> Self; pub fn with_detail(self, d: impl Into<String>) -> Self }
pub type Result<T> = std::result::Result<T, CoreError>;

// git/runner.rs
pub enum CmdKind { Read, Write }
pub struct GitOutput { pub stdout: Vec<u8>, pub stderr: String, pub code: i32 }
impl GitOutput { pub fn text(&self) -> String }           // lossy stdout
pub struct CommandRecord { pub id: u64, pub kind: CmdKind, pub cwd: String, pub args: Vec<String>,
    pub started_at_ms: i64, pub duration_ms: u32, pub exit_code: i32, pub stdout: String, pub stderr: String }
pub struct CommandLog;                                      // ring buffer, capacity 500
impl CommandLog { pub fn new() -> Arc<Self>; pub fn since(&self, id: u64) -> Vec<CommandRecord> }
#[derive(Clone)] pub struct GitRunner;
impl GitRunner {
    pub async fn detect(log: Arc<CommandLog>) -> Result<Self>;                 // GitNotFound if no git
    pub async fn run(&self, cwd: &Path, args: &[&str], kind: CmdKind) -> Result<GitOutput>;      // nonzero exit → GitFailed, message = first stderr line, detail = full stderr
    pub async fn run_unchecked(&self, cwd: &Path, args: &[&str], kind: CmdKind) -> Result<GitOutput>;
    pub fn spawn_stream(&self, cwd: &Path, args: &[&str]) -> Result<tokio::process::Child>;      // stdout piped; logged when the caller reports completion via log_stream_done
    pub fn log(&self) -> &Arc<CommandLog>;
}
pub fn resolve_login_path() -> Option<OsString>;  // runs `$SHELL -ilc 'printf %s "$PATH"'` with a 3 s timeout; None on failure or non-unix

// testutil.rs  (compiled for tests and behind feature "testutil")
pub struct TestRepo { pub dir: tempfile::TempDir }
impl TestRepo {
    pub fn new() -> Self;                                   // git init -b main, user.name "Test", user.email "test@example.com", commit.gpgsign false
    pub fn path(&self) -> &Path;
    pub fn git(&self, args: &[&str]) -> String;             // panics on failure, returns trimmed stdout
    pub fn commit(&self, msg: &str) -> String;              // writes a unique file, commits with fixed incrementing dates, returns full hash
    pub fn commit_as(&self, msg: &str, author: &str, email: &str) -> String;
    pub fn branch(&self, name: &str); pub fn checkout(&self, name: &str);
    pub fn merge(&self, name: &str) -> String;              // --no-ff, returns merge hash
    pub fn add_worktree(&self, branch: &str) -> PathBuf;
    pub fn clone_as_remote(&self) -> TestRepo;              // makes a bare "origin", adds it as remote, pushes all, fetches
}
```

- `CommandRecord.stdout` is kept only for `Write` commands and truncated to 16 KiB; `stderr` always, truncated to 16 KiB.
- `GitRunner` applies every env/arg rule from Global Constraints, and sets `PATH` to `resolve_login_path()` (cached once per process) when it returns `Some`.

- [ ] **Step 1: Failing tests** (`tests/runner.rs`):
  - `run_returns_stdout`: `run(repo, ["rev-parse","--is-inside-work-tree"], Read)` → `text().trim() == "true"`.
  - `nonzero_exit_is_git_failed`: `run(repo, ["checkout","nope"], Write)` → `Err` with `code == GitFailed`, non-empty `message`, `detail.is_some()`.
  - `run_unchecked_returns_code`: same command → `Ok` with `code != 0`.
  - `env_is_applied`: `run(repo, ["var","GIT_EDITOR"], Read)` → `"true"`; and `run(repo, ["config","--get","core.quotePath"], Read)` → `"false"`.
  - `never_waits_for_input`: `run(repo, ["commit"], Write)` on a repo with a staged change completes within 5 s (it must not open an editor or block).
  - `log_records_commands`: after two runs, `log.since(0)` has two records with correct `args` (excluding the injected `-c` pairs) and `kind`; Read record has empty `stdout`.
  - `log_is_capped`: 600 runs → `since(0).len() == 500` and the first id is 101.
  - `path_with_spaces_and_unicode`: a TestRepo created under a directory named `my répo ✨` works for `run`.
  - `login_path_is_used` (unix only): with env `SHELL` pointed at a temp script that prints `/custom/bin:/usr/bin:/bin`, `resolve_login_path()` returns that value; with `SHELL` pointed at a script that sleeps 10 s it returns `None` within 4 s. *(Review Focus 4.)*
- [ ] **Step 2:** Run `cargo test -p gg-core`. Expected: compile failures / FAIL.
- [ ] **Step 3:** Implement `error.rs`, `testutil.rs`, `git/runner.rs` to the signatures above (tokio `process::Command`, `kill_on_drop(true)`).
- [ ] **Step 4:** Run `cargo test -p gg-core`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(core): git runner with command log and test repo builder`.

### Task 3: Readers — log, refs, worktrees, status, commit details

**Files:**
- Create: `crates/gg-core/src/git/{log.rs,refs.rs,worktrees.rs,status.rs,details.rs}`, `crates/gg-core/src/dto.rs`
- Test: unit tests per file for pure parsers; `crates/gg-core/tests/readers.rs`

**Interfaces:**
- Consumes: `GitRunner`, `TestRepo`, `Result` (Task 2).
- Produces (`dto.rs` holds every type from **Boundary Types**, as Rust structs with serde camelCase; this task creates `ViewOptions`, `WorktreeInfo`, `WorktreeStatus`, `FileChange`, `CommitDetails` and the rest may be added by the tasks that first need them):

```rust
// log.rs
pub struct Commit { pub hash: String, pub parents: Vec<String>, pub author: String, pub email: String, pub time: i64, pub subject: String }
pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1e";
pub struct LogParser; impl LogParser { pub fn new() -> Self; pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Commit>); pub fn finish(self, out: &mut Vec<Commit>) }
pub fn log_args(opts: &ViewOptions, remote_branches: &[String]) -> Vec<String>;
// refs.rs
pub enum RefKind { Branch, Remote, Tag }
pub struct RefInfo { pub kind: RefKind, pub name: String, pub hash: String, pub upstream: Option<String>, pub ahead: u32, pub behind: u32 }
pub struct HeadInfo { pub branch: Option<String>, pub hash: Option<String> }
pub async fn read_refs(r: &GitRunner, cwd: &Path) -> Result<Vec<RefInfo>>;
pub async fn read_remotes(r: &GitRunner, cwd: &Path) -> Result<Vec<String>>;
pub async fn read_head(r: &GitRunner, cwd: &Path) -> Result<HeadInfo>;
// worktrees.rs / status.rs / details.rs
pub async fn read_worktrees(r: &GitRunner, cwd: &Path) -> Result<Vec<WorktreeInfo>>;   // `worktree list --porcelain -z`; first entry isMain
pub async fn read_status(r: &GitRunner, worktree: &Path) -> Result<WorktreeStatus>;    // `status --porcelain=v2 -z --untracked-files=all`
pub async fn read_details(r: &GitRunner, cwd: &Path, hash: &str) -> Result<CommitDetails>;
```

- `log_args`: always `["log", LOG_FORMAT, "--date-order"]`; then, when `branches` is `None`: `--branches`, plus `--remotes` if `showRemotes`, plus `--tags` if `showTags`, plus `HEAD`; when `branches` is `Some(list)`: each name as a revision (`refs/remotes/<n>` when the name is in `remote_branches`, otherwise `refs/heads/<n>`) and nothing else. Stash rows are Plan 2.
- `RefInfo.name` is the short name (`main`, `origin/main`, `v1.0`). `hash` is the commit the ref points at (annotated tags peeled). Remote `HEAD` symrefs (`origin/HEAD`) are skipped. `ahead`/`behind` from `%(upstream:track)`; upstream gone → `upstream` kept, counts 0.
- `read_details`: files compare against the first parent (root commit: against the empty tree); renames detected (`-M`); binary files → `additions`/`deletions` `None`.

- [ ] **Step 1: Failing tests**
  - `log.rs` unit: `parser_handles_split_records` — feed the bytes of two records split at every possible byte boundary; result always equals the two commits. `parser_handles_no_parents_and_many_parents` (0 and 3 parents). `parser_is_lossy` — subject containing bytes `0xFF 0xFE` yields a commit whose subject contains `\u{FFFD}`; no panic. `parser_handles_5000_char_subject`. *(Review Focus 3.)*
  - `log.rs` unit: `log_args_all` — `ViewOptions{branches:None,showRemotes:true,showTags:true}` → `["log", LOG_FORMAT, "--date-order", "--branches", "--remotes", "--tags", "HEAD"]`; `log_args_no_remotes_no_tags` → without `--remotes`/`--tags`; `log_args_selected` — `branches: Some(["main","origin/dev"])` with `remote_branches = ["origin/dev"]` → `["log", LOG_FORMAT, "--date-order", "refs/heads/main", "refs/remotes/origin/dev"]`.
  - `tests/readers.rs`: `log_reads_real_repo` — 3 commits + branch + `--no-ff` merge: spawn via `spawn_stream`, parse; newest first; merge commit has 2 parents; author `Test`. `refs_branches_tags_remotes` — repo with `clone_as_remote()`, branch `feature/ünï-✨`, annotated tag `v1`, local `main` 1 ahead of `origin/main` → names present exactly, tag hash equals the commit hash (peeled), `main.ahead == 1 && main.behind == 0 && main.upstream == Some("origin/main")`, no entry named `origin/HEAD`. `head_detached` — after `checkout --detach` → `branch == None`, `hash == Some(..)`. `head_empty_repo` — fresh repo → `branch == Some("main")`, `hash == None`; `read_refs` → empty vec, no error. `worktrees_lists_main_and_linked` — `add_worktree("wt-a")` → 2 entries, first `is_main`, second `branch == Some("wt-a")`, paths canonical. `status_counts` — 1 modified tracked + 2 untracked → `changed == 1, untracked == 2, conflicted == 0`; after a conflicting merge → `conflicted == 1`. `details_lists_files` — commit adding `a.txt` (2 lines), modifying `b.txt` (+1 −1), renaming `c.txt`→`d.txt`, adding a binary → statuses `A`,`M`,`R` (with `old_path == Some("c.txt")`) and binary with `additions == None`; message includes body text; `details_root_commit` lists its files as `A`; `details_merge_commit` compares against first parent.
- [ ] **Step 2:** Run `cargo test -p gg-core`. Expected: FAIL.
- [ ] **Step 3:** Implement the readers to the signatures and rules above.
- [ ] **Step 4:** Run `cargo test -p gg-core`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(core): readers for log, refs, worktrees, status and commit details`.

### Task 4: Graph layout

**Files:**
- Create: `crates/gg-core/src/graph/{mod.rs,layout.rs}`
- Test: unit tests in `layout.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)] #[serde(rename_all="lowercase")] pub enum EdgeHalf { Top, Bottom, Full }
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize)] pub struct Edge { pub from: u16, pub to: u16, pub color: u16, pub half: EdgeHalf }
#[derive(Clone, Debug)] pub struct RowLayout { pub lane: u16, pub color: u16, pub edges: Vec<Edge> }
pub struct LaneAllocator;
impl LaneAllocator { pub fn new() -> Self; pub fn push(&mut self, hash: &str, parents: &[String]) -> RowLayout; pub fn max_lane(&self) -> u16 }
```

**Algorithm (decided; implement exactly):** rows arrive newest first. State: `lanes: Vec<Option<(awaited_hash, color)>>` and `next_color: u16` starting at 0.
1. *Pass-through and arrivals.* Let `waiting` = indices of lanes awaiting `hash`. If non-empty: the node's lane is the lowest index in `waiting` and keeps that lane's color; emit `Top{from: i, to: node_lane, color: lane_i_color}` for every `i` in `waiting`; free every waiting lane except the node's. If empty: node lane = lowest free index (append if none), color = `next_color` (then increment); no `Top` edge.
2. Every other occupied lane `j` emits `Full{from: j, to: j, color: lane_j_color}`.
3. *Parents.* No parents: free the node's lane. First parent: the node's lane now awaits it, same color; emit `Bottom{from: node_lane, to: node_lane, color: node_color}`. Each further parent `p`: if some lane already awaits `p`, emit `Bottom{from: node_lane, to: that_lane, color: that_lane_color}` (lowest such lane); otherwise allocate the lowest free lane with a new color and emit `Bottom{from: node_lane, to: new_lane, color: new_color}`.
4. Lanes saturate at `u16::MAX`; trailing free lanes are truncated. `max_lane()` is the highest lane index ever used.

A lane allocated in step 3 on this row does not emit a `Full` edge on this row.

- [ ] **Step 1: Failing tests** (edges compared as sets):
  - `linear`: C3→C2→C1: every row lane 0, color 0; first row edges `{Bottom 0→0 c0}`; middle `{Top 0→0 c0, Bottom 0→0 c0}`; last `{Top 0→0 c0}`.
  - `branch_and_merge`: push `M[A,B]`, `B[C]`, `A[C]`, `C[]` → M: lane 0 c0 `{Bottom 0→0 c0, Bottom 0→1 c1}`; B: lane 1 c1 `{Full 0→0 c0, Top 1→1 c1, Bottom 1→1 c1}`; A: lane 0 c0 `{Top 0→0 c0, Bottom 0→0 c0, Full 1→1 c1}`; C: lane 0 c0 `{Top 0→0 c0, Top 1→0 c1}`. `max_lane() == 1`.
  - `two_tips_share_parent`: `X[P]`, `Y[P]`, `P[]` → X lane 0 c0; Y lane 1 c1 with `Full 0→0 c0`; P lane 0 with `{Top 0→0 c0, Top 1→0 c1}`.
  - `lane_reuse`: after the `branch_and_merge` sequence, pushing an unrelated tip `Z[]` gets lane 0 and color 2.
  - `missing_parent_dangles` *(Review Focus 5)*: `S[GONE]` then unrelated `T[]` → S lane 0 with `Bottom 0→0`; T lane 1 with `Full 0→0 c0`; no panic.
  - `octopus`: `O[A,B,C]` → three `Bottom` edges to lanes 0,1,2.
  - `scale`: 100,000 synthetic commits (linear with a side branch every 50) lay out in under 250 ms in release (`#[ignore]` unless `--release`; assert only in release via `cfg!(debug_assertions)`).
- [ ] **Step 2:** Run `cargo test -p gg-core graph`. Expected: FAIL.
- [ ] **Step 3:** Implement `LaneAllocator`.
- [ ] **Step 4:** Run `cargo test -p gg-core graph` and `cargo test -p gg-core --release graph -- --include-ignored`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(core): lane-based commit graph layout`.

### Task 5: Repository state — open, load, rows, labels, search, refresh

**Files:**
- Create: `crates/gg-core/src/repo/{mod.rs,state.rs,manager.rs}`; extend `dto.rs`, `lib.rs`
- Test: unit tests in `state.rs`; `crates/gg-core/tests/repo.rs`

**Interfaces:**
- Consumes: Tasks 2–4.
- Produces:

```rust
// lib.rs
pub trait EventSink: Send + Sync + 'static { fn emit(&self, name: &str, payload: serde_json::Value); }
// repo/manager.rs
pub struct RepoManager;
impl RepoManager {
    pub fn new(runner: GitRunner, sink: Arc<dyn EventSink>) -> Arc<Self>;
    pub async fn open(self: &Arc<Self>, path: &Path) -> Result<RepoSnapshot>;
    pub async fn close(&self, repo_id: u32) -> Result<()>;
    pub async fn snapshot(&self, repo_id: u32) -> Result<RepoSnapshot>;
    pub async fn rows(&self, repo_id: u32, generation: u64, start: u32, count: u32) -> Result<RowsPage>;
    pub async fn find_row(&self, repo_id: u32, hash: &str) -> Result<Option<u32>>;
    pub async fn search(&self, repo_id: u32, query: &str) -> Result<Vec<u32>>;
    pub async fn details(&self, repo_id: u32, hash: &str) -> Result<CommitDetails>;
    pub async fn set_view_options(self: &Arc<Self>, repo_id: u32, o: ViewOptions) -> Result<RepoSnapshot>;
    pub async fn refresh(self: &Arc<Self>, repo_id: u32) -> Result<RepoSnapshot>;
    pub async fn wait_loaded(&self, repo_id: u32);         // resolves when loading == false (tests, perf)
    pub fn open_paths(&self) -> Vec<(u32, PathBuf)>;
    pub fn git_dirs(&self, repo_id: u32) -> Option<(PathBuf /*common git dir*/, PathBuf /*root*/)>;
}
```

Behaviour (decided):
- `open(path)`: missing path → `PathMissing`; `git rev-parse --show-toplevel --git-common-dir --is-bare-repository` fails → `NotARepo` ("This folder is not a git repository."); bare → `BareRepo`. `activeWorktree` = the worktree containing `path`; `rootPath` = the main worktree; `name` = file name of `rootPath`. Opening any path of an already-open repository (same common git dir) returns the existing `repoId`. Initial `ViewOptions { branches: None, showRemotes: true, showTags: true }`.
- `open` returns as soon as refs/head/worktrees/status are read; the log loads in a background task that appends parsed commits and layouts in batches and emits `repo-changed` after the first 500 rows, then at most every 150 ms, then once with `loading: false`.
- When the active worktree is dirty (`changed + untracked + conflicted > 0`) and HEAD has a hash, row 0 is a synthetic row: `kind: "uncommitted"`, `hash: "UNCOMMITTED"`, `parents: [head]`, `subject: "Uncommitted changes (N)"` where N = changed + untracked, laid out through the same allocator before the commits.
- `rows()` builds `CommitRow`s on demand. Labels per commit: branches first (current first, then name order), then remotes, then tags. A local branch `b` absorbs every remote ref `<remote>/b` at the same commit into `remotes` (those remote refs are not emitted separately). `current` = checked out in the active worktree; `worktreePath` = path of any worktree that has the branch checked out. Hidden by view options: remote labels when `!showRemotes`, tag labels when `!showTags`. `isHead` = commit is the active worktree's HEAD. `detachedWorktrees` = paths of worktrees detached at that commit.
- `generation` starts at 1 and increases whenever row indices may have changed (reload, dirty-row appears/disappears). `rows()` with an old generation → `StaleGeneration`. Label-only changes (a ref moved between loaded commits with the same tip set) keep the commit list but still bump generation and emit `repo-changed`.
- `refresh()`: re-read head, refs, worktrees, status. Reload the log only if the set of distinct tip hashes selected by the view options changed, or HEAD's hash changed and is not a loaded commit; otherwise reuse commits. During a reload the old rows stay served until the new load has its first batch; the swap is atomic.
- `search(query)`: trimmed, case-insensitive; matches subject or author substring, or hash prefix (query of ≥ 4 hex chars); empty query → empty vec.
- Empty repository: `totalRows: 0`, `loading: false`, `head.hash: null`, no error.

- [ ] **Step 1: Failing tests** (`tests/repo.rs`, using a `RecordingSink` that stores emitted events):
  - `open_loads_all_rows`: repo with 5 commits and a merge → after `wait_loaded`, `totalRows` equals the commit count; `rows(.., 0, 100)` hashes equal `git log --date-order --branches --remotes --tags HEAD --format=%H`.
  - `open_subdirectory_and_worktree_same_repo`: opening `<repo>/sub` then a linked worktree path returns the same `repoId`; a manager that first opens the linked worktree path reports `activeWorktree` = that path and `rootPath` = main path.
  - `open_errors` *(Review Focus 2)*: nonexistent path → `PathMissing`; plain temp dir → `NotARepo`; `git init --bare` dir → `BareRepo`.
  - `empty_repo` *(Review Focus 1)*: fresh `TestRepo` → `totalRows == 0`, `loading == false`, `head.branch == Some("main")`, `head.hash == None`.
  - `labels_combine_local_and_remote`: `main` and `origin/main` at the same commit → one label `{kind: branch, name: "main", remotes: ["origin"], current: true}` and no separate `origin/main` label; after one more local commit the old commit has label `{kind: remote, name: "origin/main"}`.
  - `labels_respect_view_options`: with `showRemotes: false` there are no remote labels and every branch label's `remotes` is empty; with `showTags: false` there are no tag labels.
  - `worktree_marks`: linked worktree on branch `wt-a` → that branch's label has `worktreePath == Some(path)` and `current == false`; a worktree detached at a commit appears in that row's `detachedWorktrees`.
  - `detached_head` *(Review Focus 5)*: after `checkout --detach HEAD~1` → `head.branch == None`; the row for that commit has `isHead == true`.
  - `shallow_clone_loads` *(Review Focus 5)*: `git clone --depth 2 file://…` of a 5-commit repo → opens, `totalRows == 2`, no error.
  - `uncommitted_row`: modify a tracked file + add an untracked file → row 0 `kind == "uncommitted"`, `subject == "Uncommitted changes (2)"`, `parents == [head]`, row 1 is HEAD; `totalRows` = commits + 1.
  - `stale_generation`: capture generation, make a commit, `refresh()` → `rows()` with the old generation → `StaleGeneration`; snapshot generation is larger.
  - `refresh_without_history_change_keeps_commits`: create a new branch at an existing non-tip commit, `refresh()` → the command log shows no new `log` command since before the refresh, and the label appears on that row.
  - `refresh_with_new_commit_reloads`: new commit → after `refresh()` + `wait_loaded`, row 0 is the new commit.
  - `view_options_filter`: two diverged branches; `set_view_options(branches: Some(["feature"]))` → rows are exactly `git log refs/heads/feature`.
  - `search_matches`: subject substring (case-insensitive), author substring, 6-char hash prefix each return the right ascending indices; `""` → empty.
  - `find_row_works`: returns the index for a known hash, `None` for unknown.
  - `events_emitted`: a 1,200-commit repo emits at least two `repo-changed` events, the last with `loading == false` and `totalRows == 1200`.
- [ ] **Step 2:** Run `cargo test -p gg-core --test repo`. Expected: FAIL.
- [ ] **Step 3:** Implement `RepoState` (pure: commits, layouts, hash→index map, label building, search) and `RepoManager` (async: loading task, refresh, generation, events). Per-repo state behind a `tokio::sync::RwLock`.
- [ ] **Step 4:** Run `cargo test -p gg-core`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(core): repository state with streaming load, labels, search and refresh`.

### Task 6: Watching, fetching, background schedule

**Files:**
- Create: `crates/gg-core/src/watcher.rs`; extend `repo/manager.rs`
- Test: `crates/gg-core/tests/live.rs`

**Interfaces:**
- Consumes: `RepoManager` (Task 5).
- Produces:

```rust
impl RepoManager {
    pub async fn fetch(self: &Arc<Self>, repo_id: u32) -> Result<()>;          // `fetch --all --prune`; updates FetchState; emits repo-changed at start and end; then refresh()
    pub fn set_fetch_interval(self: &Arc<Self>, secs: u32);                     // 0 = off; applies to all open repos
    pub fn set_window_focused(self: &Arc<Self>, focused: bool);                 // gaining focus → refresh every open repo
}
// watcher.rs
pub struct RepoWatcher; // drop stops watching
pub fn watch_git_dir(common_git_dir: &Path, on_change: impl Fn() + Send + Sync + 'static) -> Result<RepoWatcher>;
```

Decided:
- Watched (via `notify`): the common git dir non-recursively (`HEAD`, `index`, `packed-refs`, `FETCH_HEAD`, `ORIG_HEAD`, `MERGE_HEAD`, …), and `refs/` and `worktrees/` recursively when they exist. Never `objects/` or `logs/`. Events for paths ending in `.lock` are ignored.
- Debounce: `on_change` fires 300 ms after the last event, and at least once per 1 s during continuous activity. `RepoManager::open` starts a watcher whose callback calls `refresh`; `close` drops it.
- A repository with no remotes: `fetch` is a no-op success. A failing fetch sets `FetchState.lastError` to git's first stderr line and does not emit `repo-error`.
- Only one fetch per repository at a time; a second call while running returns `Ok(())` immediately.
- Scheduled fetch runs only while the window is focused or was focused within the last 10 minutes (keeps idle CPU and network at zero when the app sits in the background for hours); the first scheduled fetch is one interval after open.
- Working-tree file watching is Plan 3; until then the dirty row updates on git-dir changes and on window focus.

- [ ] **Step 1: Failing tests** (`tests/live.rs`, generous timeouts, poll with `tokio::time::timeout`):
  - `external_commit_triggers_refresh`: open, `wait_loaded`, then `TestRepo::commit` from outside → within 2 s a `repo-changed` event arrives whose `totalRows` grew by 1.
  - `external_branch_triggers_refresh`: `git branch x` outside → within 2 s snapshot `branches` contains `x`.
  - `new_worktree_triggers_refresh`: `add_worktree` → within 2 s snapshot has 2 worktrees.
  - `burst_is_debounced`: 20 commits in a tight loop → fewer than 8 `log` commands recorded after the burst settles.
  - `fetch_updates_remote_refs`: repo with `clone_as_remote()`; push a new commit to the bare origin from a second clone; `fetch()` → `origin/main` label moves; `fetch.lastError == None`, `lastAtMs.is_some()`.
  - `fetch_no_remote_is_ok`; `fetch_failure_sets_last_error` (remote URL pointed at a nonexistent path) → `Ok(())`, `lastError.is_some()`.
  - `focus_triggers_refresh`: make a working-tree change (no git-dir change), call `set_window_focused(true)` → within 2 s row 0 is the uncommitted row.
  - `close_stops_watching`: after `close`, an external commit produces no further events for that `repoId` within 1.5 s.
- [ ] **Step 2:** Run `cargo test -p gg-core --test live`. Expected: FAIL.
- [ ] **Step 3:** Implement watcher, fetch, schedule, focus handling.
- [ ] **Step 4:** Run `cargo test -p gg-core`. Expected: PASS (run the `live` suite 3 times to check for flakiness).
- [ ] **Step 5:** Commit: `feat(core): git-dir watching, fetch and background fetch schedule`.

### Task 7: Store, Core, dispatch, both transports, typed IPC

**Files:**
- Create: `crates/gg-core/src/{store.rs,dispatch.rs}`; finish `lib.rs` (`Core`); `crates/gg-devserver/src/main.rs`; `src-tauri/src/lib.rs`; `src/lib/{ipc.ts,types.ts}`
- Test: unit tests in `store.rs`; `crates/gg-core/tests/dispatch.rs`; `src/lib/ipc.test.ts`

**Interfaces:**
- Consumes: Tasks 2–6.
- Produces:

```rust
pub struct Core;
impl Core {
    pub async fn new(config_dir: PathBuf, launch_path: Option<PathBuf>, sink: Arc<dyn EventSink>) -> Result<Arc<Self>>;
    pub async fn dispatch(self: &Arc<Self>, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value>;
}
```

```ts
// src/lib/ipc.ts
export class CoreError extends Error { code: CoreErrorDto["code"]; detail: string|null }
export function onEvent<T>(name: "repo-changed"|"repo-error", handler: (payload: T) => void): () => void
export const api: {
  getAppState(): Promise<AppState>; saveSession(tabs: string[], active: string|null): Promise<void>;
  updateSettings(patch: Partial<Settings>): Promise<Settings>; removeRecent(path: string): Promise<Recent[]>;
  openRepo(path: string): Promise<RepoSnapshot>; closeRepo(repoId: number): Promise<void>;
  getSnapshot(repoId: number): Promise<RepoSnapshot>;
  getRows(repoId: number, generation: number, start: number, count: number): Promise<RowsPage>;
  findRow(repoId: number, hash: string): Promise<number|null>; searchCommits(repoId: number, query: string): Promise<number[]>;
  commitDetails(repoId: number, hash: string): Promise<CommitDetails>;
  setViewOptions(repoId: number, options: ViewOptions): Promise<RepoSnapshot>;
  fetch(repoId: number): Promise<void>; refresh(repoId: number): Promise<RepoSnapshot>;
  setWindowFocused(focused: boolean): Promise<void>; perfMark(name: string, ms: number): Promise<void>;
  pickFolder(): Promise<string|null>;     // Tauri: dialog plugin; browser: window.prompt
}
```

Decided:
- Store file: `<config_dir>/state.json`, written atomically (temp file + rename), loaded once at start; a missing or corrupt file yields defaults (corrupt file is renamed to `state.json.bad`). Recents: most recent first, max 20, de-duplicated by canonical path; `open_repo` success adds/moves the repository's `rootPath` to the front. `save_session` stores tab root paths. `update_settings` validates `fetchIntervalSecs` to 0 or 30..=86400 and calls `set_fetch_interval`.
- `dispatch`: unknown command or malformed args → `BadRequest`. Table exactly as in **Commands**.
- Tauri side: one command `invoke_core(cmd: String, args: serde_json::Value) -> Result<Value, CoreError>`; `EventSink` → `app.emit(name, payload)`; config dir = Tauri's `app_config_dir`; `launch_path` = first CLI argument if it is an existing directory, else env `GG_OPEN`; window focus events → `set_window_focused`; plugin `tauri-plugin-dialog` for `pickFolder`; when env `GG_PERF_LOG` is set, `perf_mark` appends `name<TAB>ms` lines to that file.
- Dev server: `gg-devserver --port 1421 --config-dir <dir> [--open <path>]`; `POST /invoke {cmd,args}` → 200 `{ok: value}` or 200 `{err: CoreErrorDto}`; `GET /events` server-sent events `{name,payload}`; permissive CORS; binds 127.0.0.1 only.
- `ipc.ts` picks the Tauri transport when `window.__TAURI_INTERNALS__` exists, otherwise HTTP to `import.meta.env.VITE_GG_DEV_URL ?? "http://127.0.0.1:1421"`. Errors from either transport become `CoreError`.

- [ ] **Step 1: Failing tests**
  - `store.rs`: `defaults_when_missing`; `corrupt_file_is_set_aside` (file renamed to `state.json.bad`, defaults returned); `recents_dedupe_order_cap` (open a,b,a → [a,b]; 25 distinct → 20); `roundtrip_persists` (new Store on same dir sees data); `settings_validation` (`fetchIntervalSecs: 5` → `BadRequest`; `0` and `300` accepted).
  - `tests/dispatch.rs`: `unknown_command_is_bad_request`; `malformed_args_is_bad_request` (`open_repo` with `{}`); `open_then_rows_via_dispatch` (JSON in, JSON out, camelCase keys present: `repoId`, `totalRows`, `activeWorktree`); `open_adds_recent` (then `get_app_state` lists it first); `open_missing_recent_path_errors_and_recent_is_removable` *(Review Focus 2)*: recent whose folder was deleted → `open_repo` → `path_missing`; `remove_recent` returns a list without it; `launch_path_is_reported`.
  - `src/lib/ipc.test.ts` (vitest, `fetch` mocked): HTTP transport posts `{cmd:"get_rows", args:{repoId,generation,start,count}}`; `{err:{code:"not_a_repo",message:"m",detail:null}}` rejects with `CoreError` whose `code === "not_a_repo"`.
- [ ] **Step 2:** Run `cargo test -p gg-core` and `pnpm test`. Expected: FAIL.
- [ ] **Step 3:** Implement store, `Core`, dispatch, Tauri command wiring, dev server, `types.ts` (copy **Boundary Types** verbatim), `ipc.ts`.
- [ ] **Step 4:** Run `cargo test --workspace`, `pnpm test`. Manual check: start `gg-devserver`, `curl -s -XPOST localhost:1421/invoke -d '{"cmd":"get_app_state","args":{}}' -H 'content-type: application/json'` returns `{"ok":{...}}`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: core dispatch, persistent store, tauri and dev-server transports, typed ipc`.

### Task 8: App shell — theme, tabs, welcome, recents, opening repositories

**Files:**
- Create: `src/app.css`, `src/lib/state/app.svelte.ts`, `src/lib/components/{TabBar,Welcome,Toast}.svelte`, `e2e/fixtures.ts`, `e2e/shell.spec.ts`; modify `src/App.svelte`, `playwright.config.ts`
- Test: `src/lib/state/app.test.ts`, `e2e/shell.spec.ts`

**Interfaces:**
- Consumes: `api`, `onEvent`, types (Task 7).
- Produces:
  - `app.svelte.ts`: `export const app` with `$state` fields `tabs: {repoId: number; path: string; name: string}[]`, `activeRepoId: number|null`, `recents: Recent[]`, `settings: Settings`, `toasts`; functions `init()`, `openPath(path: string): Promise<void>`, `openViaPicker()`, `closeTab(repoId)`, `activate(repoId)`, `setTheme(t)`, `toast(kind: "info"|"error", message: string, detail?: string)`.
  - `e2e/fixtures.ts`: Playwright fixture `gg` that (per test) builds fixture repos with real `git` in a temp dir via exported helpers `makeRepo(name, script: (git) => void): string`, starts `gg-devserver` on a free port with a fresh config dir, starts/uses vite with `VITE_GG_DEV_URL`, and exposes `{ page, repoPath(name), git(name, ...args) }`. Named fixture repos: `small` (≈30 commits, 3 branches, 1 merge, a bare `origin` with `main` behind local, tag `v1`), `empty` (no commits).
  - CSS tokens in `app.css` (both themes): `--bg`, `--bg-elevated`, `--bg-hover`, `--bg-selected`, `--border`, `--text`, `--text-muted`, `--text-faint`, `--accent`, `--danger`, `--warning`, `--success`, `--lane-0` … `--lane-9`, `--font-ui`, `--font-mono`, `--row-h: 26px`, `--radius`.

Decided:
- Layout: tab bar on top (draggable window region on macOS, tabs + "+" button + theme toggle), then the active repository view (placeholder `<div data-testid="repo-view">` until Task 9) or `Welcome` when no tab is open.
- `Welcome`: "Open repository" button, recents list (name, shortened path, relative time, remove button), drop target for folders (Tauri drag-drop event; in browser the drop zone is inert).
- `init()`: load app state; apply theme (`data-theme` on `<html>`; `system` follows `prefers-color-scheme` live); reopen session tabs (failures become toasts and the tab is skipped); then open `launchPath` if present.
- `openPath` errors show a toast with `CoreError.message`; opening an already-open repository activates its tab.
- Session is saved after every tab change. Shortcuts: `Cmd/Ctrl+O` open, `Cmd/Ctrl+W` close tab, `Ctrl+Tab` / `Ctrl+Shift+Tab` cycle tabs, `Cmd/Ctrl+1..9` jump.
- Visual quality bar: this is the app's frame — use the `frontend-design` skill for direction; calm, dense, no default-browser look; both themes must look finished.

- [ ] **Step 1: Failing tests**
  - `app.test.ts` (vitest, `api` mocked): `openPath` adds a tab and activates it; opening the same path twice keeps one tab; `openPath` rejection calls `toast("error", message)` and adds no tab; `closeTab` on the active tab activates the neighbour; every change calls `saveSession` with the tab paths.
  - `e2e/shell.spec.ts`: welcome is shown with no tabs; opening `small` (through the picker prompt) shows a tab named `small` and `[data-testid=repo-view]`; reload → the tab is restored; closing the tab → welcome lists `small` under recents; clicking the recent reopens it; removing the recent removes it; opening a non-repo folder shows a toast containing "not a git repository" and no tab *(Review Focus 2)*; theme toggle switches `html[data-theme]` between `light` and `dark` and survives reload.
- [ ] **Step 2:** Run `pnpm test`, `pnpm e2e`. Expected: FAIL.
- [ ] **Step 3:** Implement tokens, state, components, fixtures.
- [ ] **Step 4:** Run `pnpm test`, `pnpm e2e`. Expected: PASS. Take screenshots of welcome and one open tab in both themes (Playwright `page.screenshot`) into `docs/perf/screens/`, look at them, and fix anything that looks unfinished.
- [ ] **Step 5:** Commit: `feat(ui): app shell with themes, tabs, welcome screen and recents`.

### Task 9: Graph view — virtual rows, canvas lanes, labels, live refresh

**Files:**
- Create: `src/lib/graph/{rowCache.ts,geometry.ts,GraphView.svelte,RefLabels.svelte}`, `src/lib/state/repo.svelte.ts`, `src/lib/components/Splitter.svelte`, `src/lib/format.ts` (extend), `e2e/graph.spec.ts`
- Test: `src/lib/graph/{rowCache.test.ts,geometry.test.ts}`, `src/lib/format.test.ts`, `e2e/graph.spec.ts`

**Interfaces:**
- Consumes: `api`, `onEvent`, `app` (Tasks 7–8).
- Produces:

```ts
// rowCache.ts
export class RowCache {
  constructor(fetchPage: (generation: number, start: number, count: number) => Promise<RowsPage>, pageSize = 200, maxPages = 60)
  reset(generation: number, totalRows: number): void       // drops everything when generation changes
  get(index: number): CommitRow | undefined
  ensure(first: number, last: number): Promise<void>       // loads missing pages covering [first,last] plus one page either side; de-dupes in-flight pages; evicts least-recently-used pages beyond maxPages
  onUpdate: () => void
}
// geometry.ts
export const LANE_W = 16; export const NODE_R = 4;
export function laneX(lane: number): number                 // LANE_W * lane + LANE_W / 2
export function graphWidth(maxLane: number): number         // LANE_W * (maxLane + 1)
export function edgePath(edge: Edge, rowTop: number, rowH: number): { x1:number; y1:number; cx1:number; cy1:number; cx2:number; cy2:number; x2:number; y2:number } | { x1:number; y1:number; x2:number; y2:number }
// repo.svelte.ts
export class RepoView {   // one per open tab
  snapshot: RepoSnapshot; cache: RowCache; selectedHash: string|null; searchHits: number[]; searchIndex: number
  select(hash: string|null): void
  scrollToRow(index: number): void
  applySnapshot(s: RepoSnapshot): Promise<void>   // on repo-changed: re-anchor scroll and selection by hash
}
export function repoView(repoId: number): RepoView
// format.ts
export function formatDate(unixSecs: number, now?: Date): string   // "12 Nov 2026 16:02"
export function shortenPath(path: string, home?: string): string    // "~/Dev/x"
```

Decided:
- `edgePath`: `full` → straight line x = laneX(from), rowTop → rowTop+rowH. `top` → from (laneX(from), rowTop) to (laneX(to), rowTop + rowH/2); `bottom` → from (laneX(from), rowTop + rowH/2) to (laneX(to), rowTop + rowH). When `from === to` it is a straight line; otherwise a cubic curve whose control points are vertically offset by `rowH/2 * 0.8` so lines leave and arrive vertically.
- One `<canvas>` the size of the visible graph column (scaled by `devicePixelRatio`), redrawn in `requestAnimationFrame` on scroll, resize, cache update and theme change; it draws edges then nodes for visible rows ± 2. Colors: `--lane-{color % 10}` read once per theme. HEAD node drawn as a ring; merge nodes smaller; the uncommitted row's node hollow grey with a dashed edge.
- Rows: a scroll container with a spacer of `totalRows * rowH`; a recycled pool of absolutely positioned row elements for visible rows ± 10 overscan. A row not yet in the cache renders a skeleton (never blank space, never layout shift). Scrolling calls `cache.ensure` for the visible range plus 400 rows in the scroll direction.
- Columns: Graph · Description (labels + subject) · Date · Author · Commit (8-char hash, monospace); header row; column widths resizable by dragging header edges, persisted in `localStorage` per column set; Graph column default width `min(graphWidth(maxLane), 240)` and clips horizontally.
- Labels (`RefLabels.svelte`): pill with branch icon; `name` then each remote in italics after a divider (`staging | origin`); current branch bold with a ring marker; a branch checked out in another worktree shows a small worktree glyph with the path as tooltip; tags have a tag icon; remote-only labels show `origin/name`; `↑n ↓m` when ahead/behind > 0. Label colour follows the row's lane colour. Labels never wrap; the subject truncates with an ellipsis.
- Merge commits: subject in `--text-muted`. HEAD row subject bold.
- Selection: click selects (highlight), `↑`/`↓` move selection and keep it in view, `Home`/`End` jump, `Esc` clears.
- Live refresh: on `repo-changed` for this repo, if `generation` changed: remember the hash of the first fully visible row and its pixel offset, `cache.reset`, then `findRow(hash)` and restore scroll so that row sits at the same offset (fall back to the same index if the hash vanished); selection is kept by hash. No visible jump when nothing above the viewport changed.
- `stale_generation` from `getRows` is swallowed: fetch a fresh snapshot and retry once.
- Empty repository: centred message "No commits yet" *(Review Focus 1)*. While `loading` with zero rows: skeleton rows.
- Report first paint: after the first frame that draws real rows, call `api.perfMark("first-rows", performance.now())`; when `loading` first becomes false, `api.perfMark("fully-loaded", performance.now())`.

- [ ] **Step 1: Failing tests**
  - `rowCache.test.ts`: `ensure(0, 50)` requests pages starting at 0 and 200 only (one page either side, clipped at 0); a second identical `ensure` while in flight makes no new request; `get` returns rows after resolve and `onUpdate` fired; `reset` with a new generation empties the cache and results arriving for the old generation are discarded; with `maxPages = 2`, loading a third page evicts the least recently used; `ensure` past `totalRows` clips.
  - `geometry.test.ts`: `laneX(0) === 8`, `laneX(2) === 40`, `graphWidth(1) === 32`; `edgePath({from:1,to:1,half:"full"}, 100, 26)` → `{x1:24,y1:100,x2:24,y2:126}`; `top` from 1 to 0 at rowTop 0, rowH 26 → starts `(24,0)`, ends `(8,13)`; `bottom` from 0 to 1 → starts `(8,13)`, ends `(24,26)`.
  - `format.test.ts`: `formatDate(1794477720)` in a fixed timezone (set `TZ=UTC` for vitest) → `"12 Nov 2026 10:02"`; `shortenPath("/Users/a/Dev/x", "/Users/a") === "~/Dev/x"`.
  - `e2e/graph.spec.ts` (fixture `small`, plus `big` = 3,000 linear commits generated in the fixture): rows show subject, author, date and an 8-char hash matching `git log`; the label for `main` is visible on the right row and remote/local combination renders as one pill containing `origin` when at the same commit; canvas is non-empty (sample pixels: at least one non-background pixel in the graph column); scrolling `big` to the bottom shows the oldest commit subject and never shows an empty row element after 500 ms; clicking a row sets `aria-selected=true`; `ArrowDown` moves selection; an external `git commit` in the fixture repo appears as the first row within 2 s without changing `scrollTop` when scrolled to top, and when scrolled to the middle of `big` the first visible subject is unchanged after the refresh; modifying a file externally and refocusing shows an "Uncommitted changes" row; `empty` shows "No commits yet".
- [ ] **Step 2:** Run `pnpm test`, `pnpm e2e`. Expected: FAIL.
- [ ] **Step 3:** Implement cache, geometry, `RepoView`, `GraphView`, `RefLabels`, column resizing; mount `GraphView` in the repo view.
- [ ] **Step 4:** Run `pnpm test`, `pnpm e2e`. Expected: PASS. Screenshot the graph for `small` in both themes into `docs/perf/screens/`, inspect, fix what looks off (alignment of dots with rows, label clipping, colour contrast).
- [ ] **Step 5:** Commit: `feat(ui): virtualized commit graph with canvas lanes, ref labels and live refresh`.

### Task 10: Speed proof (gate)

**Files:**
- Create: `scripts/gen-synthetic-repo.sh`, `scripts/perf.sh`, `docs/perf/2026-11-plan-1.md`, `crates/gg-core/benches/` or `crates/gg-core/tests/perf.rs`

**Interfaces:**
- Consumes: the built app (Tasks 1–9), `GG_OPEN`, `GG_PERF_LOG`, `perf_mark`.
- Produces: `scripts/gen-synthetic-repo.sh <dir> <commits> <branches>` (uses `git fast-import`; deterministic; merges every ~40 commits; branch names like `feature/n`); `scripts/perf.sh <repo>` prints a table of measured numbers.

- [ ] **Step 1:** Write `gen-synthetic-repo.sh`; generate `~/.cache/gitgraph-perf/r20k` (20,000 commits, 200 branches) and `r100k` (100,000 commits, 400 branches). Verify counts with `git rev-list --all --count`.
- [ ] **Step 2:** Write `tests/perf.rs` (`#[ignore]`, release only): opening `r20k` reaches the first `repo-changed` with `totalRows >= 500` in < 300 ms and `loading == false` in < 1,500 ms; `r100k` fully loaded in < 6 s; 1,000 random `rows(.., start, 200)` calls average < 2 ms; `search("feature")` on `r100k` < 150 ms. Paths come from env `GG_PERF_R20K` / `GG_PERF_R100K`; the test is skipped with a printed notice when unset.
- [ ] **Step 3:** Run it: `cargo test -p gg-core --release --test perf -- --ignored --nocapture`. Fix the core until it passes (profile before changing anything).
- [ ] **Step 4:** Write `scripts/perf.sh`: `pnpm tauri build` (release), launch the binary with `GG_OPEN=<repo>` and `GG_PERF_LOG`, record process start → `first-rows` and → `fully-loaded` (the UI marks are relative to navigation start; also record wall-clock from spawn to the first line appearing in the log), wait 20 s idle, then sample over 10 s: CPU % of the app process and its WebKit helper processes, and their summed resident memory (document exactly how helpers were identified). Report bundle size (`du -sh` of the `.app`).
- [ ] **Step 5:** Scroll test in the browser build against the dev server on `r100k` with Playwright: programmatically scroll from top to bottom in 200 steps over ~6 s while recording `requestAnimationFrame` deltas; report the 95th-percentile frame time and the count of frames in which any visible row was a skeleton for longer than 150 ms. Target: p95 ≤ 17 ms on the test machine, zero rows still skeleton 300 ms after scrolling stops.
- [ ] **Step 6:** Write `docs/perf/2026-11-plan-1.md`: a table of every spec §7 target against the measured value for `r20k` and `r100k`, the method for each, and for any miss the cause and what was tried. **If "launch to first screen" exceeds 1 s or scrolling cannot hold the target after optimization, stop and report to the coordinator instead of proceeding** (spec phase 0 gate).
- [ ] **Step 7:** Commit: `perf: synthetic repo generator, measurements and results for plan 1`.

### Task 11: Toolbar — branch filter, remotes toggle, search, fetch, refresh

**Files:**
- Create: `src/lib/components/{Toolbar,BranchFilter,SearchBox}.svelte`, `e2e/toolbar.spec.ts`; modify `repo.svelte.ts`, `GraphView.svelte`
- Test: `src/lib/state/repo.test.ts`, `e2e/toolbar.spec.ts`

**Interfaces:**
- Consumes: `RepoView`, `api` (Tasks 7, 9).
- Produces: `RepoView.setViewOptions(patch: Partial<ViewOptions>)`, `RepoView.runSearch(query: string)`, `RepoView.nextHit()`, `RepoView.prevHit()`, `RepoView.fetch()`, `RepoView.refresh()`.

Decided:
- Toolbar (under the tab bar): branch filter dropdown ("Show All" or "N branches"), "Remote branches" checkbox, "Tags" checkbox, spacer, search button, fetch button, refresh button.
- Branch filter: popover with a filter text field, "Show All", then local branches, then remote branches (hidden when remotes are off), multi-select with checkboxes; applies on change; keyboard navigable; closes on `Esc`/outside click. View options are remembered per repository path in `localStorage` and re-applied on open.
- Search: `Cmd/Ctrl+F` opens an inline search box; typing (debounced 150 ms) calls `searchCommits`; shows "3 of 41"; `Enter`/`Shift+Enter` next/previous (wraps); the current hit is scrolled into view and selected; all hits get a subtle row marker; matched text in visible subjects is highlighted; `Esc` closes and clears.
- Fetch: button shows a spinning state while `snapshot.fetch.running`; on `lastError` it shows a warning dot whose tooltip/toast carries the message; tooltip shows "Last fetched 3 min ago". `Cmd/Ctrl+Shift+F` fetch, `Cmd/Ctrl+R` refresh (must not reload the page: `preventDefault`).
- Changing view options keeps the selected commit selected and in view when it still exists.

- [ ] **Step 1: Failing tests**
  - `repo.test.ts` (api mocked): `runSearch("fix")` stores hits and selects the first; `nextHit` wraps from last to first; `setViewOptions({showRemotes:false})` calls `api.setViewOptions` with the merged options and persists them under the repo path key; empty query clears hits without calling the api.
  - `e2e/toolbar.spec.ts`: selecting only `feature` in the filter leaves exactly the commits of `git log feature`; "Show All" restores; unticking "Remote branches" removes `origin` from all labels; search for a known subject word shows the right "n of m", `Enter` moves the selection to the next hit and it is visible; `Esc` closes search; clicking fetch after pushing a commit to the fixture's bare origin from outside makes the `origin/main` label move; a fetch against a broken remote shows the warning state with git's message; pressing `Cmd/Ctrl+R` does not reload the page (a JS variable set before the keypress survives); filter choice survives a page reload.
- [ ] **Step 2:** Run `pnpm test`, `pnpm e2e`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `pnpm test`, `pnpm e2e`. Expected: PASS. Screenshot toolbar, open filter popover and active search in both themes; inspect and fix.
- [ ] **Step 5:** Commit: `feat(ui): toolbar with branch filter, search, fetch and refresh`.

### Task 12: Commit details panel

**Files:**
- Create: `src/lib/components/CommitDetails.svelte`, `src/lib/fileTree.ts`, `e2e/details.spec.ts`; modify repo view layout
- Test: `src/lib/fileTree.test.ts`, `e2e/details.spec.ts`

**Interfaces:**
- Consumes: `RepoView.selectedHash`, `api.commitDetails`, `Splitter`.
- Produces: `buildFileTree(files: FileChange[]): TreeNode[]` where `type TreeNode = { name: string; path: string; children: TreeNode[]; file: FileChange|null }`; folders with a single child folder are collapsed into one node (`app/peaches`); folders sorted before files, then by name.

Decided:
- Selecting a commit opens a panel docked at the bottom of the repo view (resizable via `Splitter`, height persisted, default 280 px); `Esc` or its close button closes it and clears selection. The uncommitted row shows no panel in this plan.
- Left: Commit (full hash, click to copy), Parents (each clickable → select and scroll to that commit), Author `name <email>`, Committer (only when different from author), Date (full local date-time), then the full message (whitespace preserved, selectable).
- Right: changed-file tree with a toggle between tree and flat list (persisted); each file shows a status colour (A green, M amber, D red, R blue) and `+a | −d` counts; renamed files show `old → new`; binary files show "binary"; header "N files changed".
- Details are cached per hash in the `RepoView` (max 50). Switching selection quickly must never show details for a previous selection (ignore late responses).
- Text everywhere in the app is selectable only where it is content (hash, message, paths); chrome is not.

- [ ] **Step 1: Failing tests**
  - `fileTree.test.ts`: `["app/peaches/a.py","app/peaches/b.py","app/routes/r.py","docs/A.md"]` → roots `app` (children `peaches`, `routes`) and `docs`; a lone chain `a/b/c/x.txt` collapses to node name `a/b/c`; folders precede files; a renamed file keeps `oldPath`.
  - `e2e/details.spec.ts`: clicking a known commit shows its full hash, author, and full message including body; the file list shows the right paths, statuses and counts for a fixture commit that adds, modifies and renames files; clicking the parent hash selects the parent row; the merge commit lists files relative to its first parent; rapidly clicking five rows ends showing the fifth's hash; `Esc` closes the panel; panel height survives reload.
- [ ] **Step 2:** Run `pnpm test`, `pnpm e2e`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `pnpm test`, `pnpm e2e`. Expected: PASS. Screenshot both themes; inspect and fix.
- [ ] **Step 5:** Commit: `feat(ui): commit details panel with changed-file tree`.

### Task 13: Real-app verification and polish pass

**Files:**
- Modify: whatever the pass finds; update `docs/perf/2026-11-plan-1.md`; `README.md`

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1:** Run the whole suite: `cargo test --workspace`, `cargo test -p gg-core --release --test perf -- --ignored`, `pnpm test`, `pnpm e2e`. Expected: PASS.
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` and `pnpm exec svelte-check` clean.
- [ ] **Step 3:** Build the release app; run `scripts/perf.sh` on `r20k` and on this repository; update the perf document with final numbers.
- [ ] **Step 4:** Launch the release app normally (not from a terminal: `open path/to/GitGraph.app`) with a real repository to confirm git is found under the Finder environment *(Review Focus 4)*; confirm drag-and-drop of a folder and the folder picker work; confirm launch with a path argument opens that repository.
- [ ] **Step 5:** Visual pass in the browser build over every screen in both themes at 1280×800 and 900×560: spacing rhythm, alignment of graph dots with text baselines, truncation, focus rings, hover states, empty and loading states, toasts. Fix what is found; keep screenshots in `docs/perf/screens/`.
- [ ] **Step 6:** Commit: `chore: plan 1 verification, perf results and polish`.
