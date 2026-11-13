# Plan 2 — Actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the graph actionable: right-click menus and dialogs for branches, commits, tags and stashes; one-at-a-time operations with a visible busy state; the conflict banner; the command log; and the smooth checkout → merge → push routine.

**Architecture:** The UI sends one typed `OpRequest` to the core (`run_op`). The core checks it for dangers, queues it per repository, runs the git commands in the active worktree through `GitRunner`, refreshes the repository once, and returns an `OpOutcome` (done / stopped on conflicts / failed with a plain explanation / needs confirmation). Menus are pure functions from (row, label, snapshot) to item lists; dialogs collect options and call one store function, `runOp`, which owns confirmation, busy state and result notices.

**Tech Stack:** As Plan 1 — Rust (`gg-core`, tokio), Tauri 2, Svelte 5 runes, TypeScript strict, vitest, Playwright against `gg-devserver`.

**Spec:** `docs/superpowers/specs/2026-11-12-git-graph-desktop-app-design.md` — this plan implements §5.2 (except "create worktree" and pull-request items, which are Plan 3), §5.3, §5.6, §5.7, the stash parts of §5.1, and the "Operation" and "In-progress state" concepts of §4.

**Builds on:** Plan 1 (`docs/superpowers/plans/2026-11-12-plan-1-foundation-and-graph.md`). Boundary reference for existing types: `.superpowers/sdd/2026-11-12-plan-1-foundation-and-graph/boundary-types.md`.

## Global Constraints

- Everything in Plan 1's Global Constraints still binds (git only through `GitRunner` as argument arrays; no shell; machine-readable output; UI never builds git commands or parses git output; camelCase JSON, lowercase enum strings; Svelte 5 runes, TS strict, no component library; no macOS-only API without a fallback).
- Every git child keeps Plan 1's environment and leading args, plus: `GIT_SEQUENCE_EDITOR=true`, `GIT_MERGE_AUTOEDIT=no`. Operations pass `--no-edit` wherever git accepts it. Git must never wait for terminal input.
- User-typed names (branch, tag, stash message) never reach an option position: names are validated in the core (`git check-ref-format`), passed as full refs or after `--`, and a name starting with `-` is rejected with a plain message.
- Operations on one repository run one at a time, in request order. A second request while one runs is queued, never run in parallel and never dropped silently.
- Every operation runs in the repository's active worktree (`RepoSnapshot.activeWorktree`).
- Dangerous paths cannot run unconfirmed, enforced in the core, not the UI: force push, hard reset, deleting an unmerged branch, deleting a remote branch, dropping a stash. Force push always uses `--force-with-lease`.
- The app never edits conflicted files and never resolves conflicts.
- The app writes into the user's repository only as the direct effect of an operation the user asked for. Background activity stays as in Plan 1.
- Operations the user starts run the repository's hooks, like the terminal. Network operations (push, pull, fetch into local, remote deletes) stop after 120 s without output or 2 h in total (`TimeLimit::idle`), local ones have no time limit.
- A failed operation shows git's own message in a readable notice, with a plain one-line explanation above it where the core recognises the failure, and a link to its command-log entry.
- Feedback after a click (menu opens, dialog opens, busy state shows) within 100 ms. Plan 1's speed targets must not regress: re-measure launch, scroll and idle CPU at the end.
- Tests accompany every task. `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` (default and `--features perf`), `cargo fmt --check`, `pnpm check`, `pnpm test`, `pnpm e2e` pass, with clean output, before a task is reported done.
- Commits: small, conventional-commit subjects. **Never add any AI attribution** — no `Co-Authored-By`, no session links, no "generated with" lines. Work on branch `dev`. Do not push, do not merge.

## Review Focus

1. **Names that look like options or are not valid refs** — a new branch called `-f`, `a..b`, `has space`, or an existing name: rejected before any git command runs, with a message that says why; emoji and slashes work. *(Tests: Task 2.)*
2. **Working tree in the way** — checkout, merge, pull, cherry-pick or stash pop that would overwrite uncommitted changes: nothing is lost, the notice says which files are in the way. *(Tests: Task 2, Task 5.)*
3. **Detached HEAD and unborn branch** — items that need a current branch (merge into current, rebase current, push current) are disabled with the reason shown; nothing crashes on an empty repository. *(Tests: Task 6 menu builders, Task 2.)*
4. **Network without credentials or upstream** — push with no upstream, to a remote that needs a password with no helper, or to an unreachable host: fails with an explanation, never hangs; the next operation still runs. *(Tests: Task 3.)*
5. **State changed underneath** — `index.lock` held by another process, a branch deleted or a stash dropped in the terminal between right-click and confirm, a conflict state started outside the app: plain message or correct banner, never a wrong target. *(Tests: Task 1, Task 4, Task 5.)*

## File Structure

Core (`crates/gg-core/src/`):
- `ops/mod.rs` — `OpRequest`, `OpOutcome`, `Danger`, `OpHint`; `RepoManager::run_op`; the per-repository queue.
- `ops/danger.rs` — which dangers a request carries, given repository state.
- `ops/explain.rs` — maps git failures to plain explanations and hints.
- `ops/local.rs` — branch, tag, commit operations (Task 2). `ops/network.rs` — push, pull, remote operations (Task 3). `ops/progress.rs` — continue / abort (Task 4). `ops/stash.rs` (Task 5).
- `git/names.rs` — name validation. `git/progress.rs` — in-progress state reader. `git/stash.rs` — stash reader.
- `git/runner.rs` — environment additions, write log, op tagging. `store.rs` — per-repository preferences, `editorCommand`.

UI (`src/lib/`):
- `state/ops.svelte.ts` — `runOp`, confirmation flow, result notices.
- `components/ContextMenu.svelte`, `components/Dialog.svelte`, `components/dialogs/*.svelte`, `components/ConflictBanner.svelte`, `components/CommandLog.svelte`.
- `menus/commitMenu.ts`, `menus/branchMenu.ts`, `menus/tagMenu.ts`, `menus/stashMenu.ts`, `menus/types.ts` — pure menu builders.

## Boundary additions (authoritative for all tasks)

```ts
// Existing types gain:
type CoreErrorDto["code"] |= "bad_name" | "busy" | "nothing_to_do"
type ViewOptions += { showStashes: boolean }                       // default true
type RefLabel["kind"] |= "stash"                                   // name "stash@{0}"; subject is the row's
type CommitRow["kind"] |= "stash"
type RepoSnapshot += {
  op: { id: number; title: string; startedAtMs: number; queued: number } | null;
  inProgress: InProgress | null;
}
type InProgress = {
  kind: "merge" | "rebase" | "cherry-pick" | "revert";
  conflicted: string[];               // paths relative to the worktree, sorted
  step: number | null; total: number | null;   // rebase only
  subject: string | null;             // what is being applied, e.g. the incoming branch or commit subject
}
type Settings += { editorCommand: string | null }                  // default null = system default opener
type RepoPrefs = { mergeNoFf: boolean }                            // default true
type CommandRecord += { repoId: number | null; opId: number | null }

type MergeTarget = { kind: "branch" | "remote" | "commit"; name: string }   // name: branch name, "origin/x", or full hash
type OpRequest =
  | { op: "checkout_branch"; branch: string; fastForward: boolean }
  | { op: "checkout_remote"; remoteBranch: string; localName: string }
  | { op: "checkout_commit"; hash: string }
  | { op: "create_branch"; name: string; at: string; checkout: boolean }
  | { op: "rename_branch"; branch: string; newName: string }
  | { op: "delete_branch"; branch: string }
  | { op: "merge"; target: MergeTarget; noFf: boolean; squash: boolean; noCommit: boolean }
  | { op: "rebase"; onto: string }                                  // local or remote branch name
  | { op: "cherry_pick"; hash: string; mainline: number | null; noCommit: boolean }
  | { op: "revert"; hash: string; mainline: number | null }
  | { op: "reset"; hash: string; mode: "soft" | "mixed" | "hard" }
  | { op: "create_tag"; name: string; hash: string; message: string | null }
  | { op: "delete_tag"; name: string; remote: string | null }       // remote set = also delete there
  | { op: "push_tag"; name: string; remote: string }
  | { op: "push"; branch: string; remote: string; setUpstream: boolean; force: boolean }
  | { op: "pull"; branch: string; mode: "default" | "merge" | "rebase" }
  | { op: "fetch_into_local"; remoteBranch: string; localName: string }
  | { op: "delete_remote_branch"; remote: string; branch: string }
  | { op: "stash_save"; message: string | null; includeUntracked: boolean }
  | { op: "stash_apply" | "stash_pop" | "stash_drop"; stash: string }       // stash commit hash
  | { op: "stash_branch"; stash: string; name: string }
  | { op: "continue" } | { op: "abort" }

type DangerKind = "force_push" | "hard_reset" | "delete_unmerged_branch" | "delete_remote_branch" | "drop_stash"
type Danger = { kind: DangerKind; message: string; detail: string | null }
type OpHint =
  | { kind: "push"; branch: string; remote: string; setUpstream: boolean }
  | { kind: "fetch" } | { kind: "pull"; branch: string }
  | { kind: "force_push"; branch: string; remote: string }
  | { kind: "choose_pull_mode"; branch: string }
type OpOutcome = {
  opId: number;
  status: "done" | "stopped" | "failed" | "needs_confirmation";
  title: string;                // "Merge feature/x into staging"
  message: string;              // one plain line for the notice
  detail: string | null;        // git's own output on failure
  logFrom: number; logTo: number;      // command-log ids of this operation (0,0 if none ran)
  dangers: Danger[];            // non-empty only for needs_confirmation
  hints: OpHint[];
  snapshot: RepoSnapshot;       // state after the operation and its refresh
}
```

Commands added: `run_op {repoId, request, confirmed: DangerKind[]}` → `OpOutcome`; `command_log {sinceId, repoId?, writesOnly?}` (extended) → `CommandRecord[]`; `get_repo_prefs {repoId}` → `RepoPrefs`; `set_repo_prefs {repoId, patch}` → `RepoPrefs`; `open_in_editor {repoId, path: string | null}` → `null`.
Events added: `op-progress` → `{repoId, opId, line}` (latest progress line of a network operation, at most 10 per second).

`status: "stopped"` means git stopped on conflicts: it is not a failure; `snapshot.inProgress` is set. `needs_confirmation` means nothing ran.

---

### Task 1: Operation framework

**Files:**
- Create: `crates/gg-core/src/ops/mod.rs`, `ops/danger.rs`, `ops/explain.rs`, `crates/gg-core/src/git/names.rs`, `crates/gg-core/tests/ops.rs`
- Modify: `git/runner.rs`, `repo/manager.rs`, `dto.rs`, `dispatch.rs`, `store.rs`, `lib.rs`, `src/lib/types.ts`, `src/lib/ipc.ts`

**Interfaces:**
- Consumes: `GitRunner::run_unchecked`, `run_with_timeout`, `CommandLog`, `RepoManager::refresh`, `refresh_lock`.
- Produces:
  - `pub async fn RepoManager::run_op(self: &Arc<Self>, repo_id: u32, request: OpRequest, confirmed: &[DangerKind]) -> Result<OpOutcome>`
  - `pub(crate) struct OpCtx<'a> { pub repo: &'a Arc<Repo>, pub runner: GitRunner, pub cwd: PathBuf, pub op_id: u64 }` with `async fn git(&self, args: &[&str]) -> Result<GitOutput>` (Write, unchecked exit code, tagged with repo and op id) and `async fn git_net(&self, args: &[&str]) -> Result<GitOutput>` (same with the network time limit and `op-progress` events).
  - `pub(crate) enum Step { Done(String), Stopped(String), Failed { message: String, detail: String, hints: Vec<OpHint> } }` — what an operation's body returns; `run_op` turns it into `OpOutcome`.
  - `pub(crate) async fn dangers(ctx: &OpCtx<'_>, request: &OpRequest) -> Result<Vec<Danger>>`
  - `pub(crate) fn explain(args: &[&str], out: &GitOutput) -> Step` — the failed step for a non-zero exit.
  - `pub fn check_branch_name(name: &str) -> Result<()>` and `check_tag_name` in `git/names.rs` (error code `bad_name`).
  - `Store::repo_prefs(root: &Path) -> RepoPrefs`, `Store::set_repo_prefs(root, patch)`; `Settings.editor_command`.
  - In this task `run_op` implements only `create_branch` (without checkout) end to end, as the proof of the framework; every other variant returns `bad_request` "not implemented" until its task.

- [ ] **Step 1: Failing tests in `tests/ops.rs`** (real temporary repositories via `testutil`):
  - `create_branch_runs_and_returns_the_new_snapshot` — outcome `done`, `snapshot.branches` contains the name, `logFrom..=logTo` are `Write` records whose `opId` matches.
  - `operations_on_one_repository_run_one_at_a_time` — with a test-only delay on the op body, two `run_op` calls overlap in wall time zero; the second starts after the first ends; `snapshot.op` is set (with `queued: 1`) in a `repo-changed` event while the first runs and is `null` in both outcomes.
  - `an_operation_refreshes_once` — exactly one `repo-changed` with a new generation or labels after the operation, watcher events during it included.
  - `index_lock_held_fails_with_an_explanation` — create `.git/index.lock`, run an op that needs the index (use a test hook op or `create_branch` + checkout once Task 2 lands; here assert `explain` on captured stderr): message says another git process is running in this repository.
  - `bad_names_are_rejected_before_git_runs` — `-f`, `a..b`, `has space`, `""`, `HEAD`, an existing branch: error `bad_name`, no `Write` record logged; `feature/émoji-🚀` passes.
  - `writes_survive_many_reads_in_the_log` — 600 read commands after one write: `command_log {writesOnly: true}` still returns the write.
  - `repo_prefs_round_trip_and_default` — default `mergeNoFf: true`; a patch persists across `Store` reloads and is keyed by repository root.
- [ ] **Step 2: Run them, see them fail.** `cargo test -p gg-core --test ops`
- [ ] **Step 3: Implement.** Add `GIT_SEQUENCE_EDITOR=true`, `GIT_MERGE_AUTOEDIT=no` to `GIT_ENV`. `CommandLog` keeps writes in their own 500-entry ring next to the reads ring, ids still global and increasing; records carry `repo_id` and `op_id`. Queue: a fair `tokio::sync::Mutex` per repository held for the whole operation (dangers check → body → refresh); `snapshot.op` is published on start and cleared on end; watcher-driven refreshes requested while an operation holds the queue are folded into the operation's final refresh. Danger check: if `dangers()` returns any kind not listed in `confirmed`, return `needs_confirmation` without running anything.
- [ ] **Step 4: Tests pass**; update `types.ts` and `ipc.ts` (`runOp`, `commandLog` options, `getRepoPrefs`, `setRepoPrefs`, `openInEditor`) with the vitest boundary test in `ipc.test.ts` extended for the new shapes.
- [ ] **Step 5: Commit.**

### Task 2: Local operations

**Files:**
- Create: `crates/gg-core/src/ops/local.rs`
- Modify: `ops/mod.rs`, `ops/danger.rs`, `ops/explain.rs`, `tests/ops.rs`

**Interfaces:**
- Consumes: Task 1's `OpCtx`, `Step`, `explain`, name checks.
- Produces: `run_op` handles `checkout_branch` (without `fastForward`, which is Task 3), `checkout_remote`, `checkout_commit`, `create_branch` (with `checkout`), `rename_branch`, `delete_branch`, `merge`, `rebase`, `cherry_pick`, `revert`, `reset`, `create_tag`, `delete_tag` (local part). A body that ends with git stopped on conflicts returns `Step::Stopped`; detection in this task is "exit code non-zero and `MERGE_HEAD` / `CHERRY_PICK_HEAD` / `REVERT_HEAD` / `rebase-merge` / `rebase-apply` exists under the worktree's git dir" (helper `pub(crate) async fn stopped_kind(ctx) -> Option<InProgressKind>`; Task 4 builds the full reader on it).

Commands, exactly (names as full refs where git accepts them):
- checkout branch: `checkout <branch> --`; remote: `checkout -b <local> --track <remote/branch>`; commit: `checkout --detach <hash>`.
- create branch: `branch -- <name> <hash>` or `checkout -b <name> <hash>`; rename: `branch -m -- <old> <new>`.
- delete branch: `branch -d -- <name>`; when the `delete_unmerged_branch` danger is confirmed, `branch -D -- <name>`.
- merge: `merge --no-edit [--no-ff | --ff] [--squash] [--no-commit] <target>`; `--squash` excludes `--no-ff`.
- rebase: `rebase <onto>`; cherry-pick: `cherry-pick [-m N] [--no-commit] <hash>`; revert: `revert --no-edit [-m N] <hash>`.
- reset: `reset --soft|--mixed|--hard <hash>`.
- tag: `tag -- <name> <hash>` or `tag -a -m <message> -- <name> <hash>`; delete: `tag -d -- <name>`.

Dangers: `delete_branch` → `delete_unmerged_branch` when the branch is not an ancestor of HEAD nor of its upstream (`merge-base --is-ancestor`), message names how many commits only it has; `reset` with `hard` → `hard_reset`, message names the count of changed and untracked files that will be lost (untracked files are kept by git; say so).

Explanations added to `explain.rs` (each with a test on real git output): local changes would be overwritten (lists the files git names); branch is checked out in another worktree (names the path); branch already exists; not fully merged; nothing to commit / already up to date (`nothing_to_do`, reported as `done` with that message, not as a failure); cherry-pick or revert of a merge without `mainline`; no current branch for merge/rebase on a detached HEAD.

- [ ] **Step 1: Failing tests**, one per operation for the success path asserting the resulting refs/HEAD with git, plus:
  - `merge_no_ff_creates_a_merge_commit_and_ff_does_not`, `merge_squash_leaves_changes_staged`, `merge_no_commit_leaves_merge_in_progress_without_conflicts` (status `done`, `inProgress` handled in Task 4 — here assert `MERGE_HEAD` exists).
  - `merge_with_conflicts_stops` / `rebase…` / `cherry_pick…` / `revert…` — status `stopped`, no editor hang (test completes under 10 s).
  - `checkout_with_changes_in_the_way_fails_and_keeps_them` — file content unchanged afterwards; message lists the file.
  - `delete_unmerged_branch_needs_confirmation` — unconfirmed: `needs_confirmation`, branch still exists, no `Write` record; confirmed: gone.
  - `hard_reset_needs_confirmation` — same shape; soft and mixed run without.
  - `delete_merged_branch_runs_without_confirmation`.
  - `checkout_branch_held_by_another_worktree_explains`.
  - `cherry_pick_merge_commit_without_mainline_explains`.
  - `operations_on_detached_head` — merge into detached HEAD works; `rebase` explains.
  - `empty_repository` — every request returns a plain failure, none panics.
- [ ] **Step 2: Run, fail.** **Step 3: Implement.** **Step 4: Pass.** **Step 5: Commit** (one commit per group: checkout/branch, merge/rebase, cherry-pick/revert/reset, tags).

### Task 3: Network operations and the staging routine in the core

**Files:**
- Create: `crates/gg-core/src/ops/network.rs`
- Modify: `ops/mod.rs`, `ops/danger.rs`, `ops/explain.rs`, `ops/local.rs` (checkout `fastForward`), `tests/ops.rs`, `testutil.rs` (bare-remote helper if missing)

**Interfaces:**
- Consumes: `OpCtx::git_net`.
- Produces: `run_op` handles `push`, `pull`, `fetch_into_local`, `delete_remote_branch`, `push_tag`, `delete_tag` with `remote`, and `checkout_branch` with `fastForward: true`.

Commands:
- push: `push --progress [--set-upstream] [--force-with-lease] <remote> refs/heads/<branch>:refs/heads/<branch>`.
- pull, current branch: `pull --progress --no-edit` plus `--no-rebase` for `merge`, `--rebase` for `rebase`, nothing for `default` (the user's config decides). Pull of a branch that is not checked out: `fetch --progress <remote> refs/heads/<b>:refs/heads/<b>` (fast-forward only; its upstream's remote).
- fetch into local: `fetch --progress <remote> refs/heads/<rb>:refs/heads/<local>`.
- delete remote branch: `push --progress <remote> --delete refs/heads/<branch>`; push tag: `push --progress <remote> refs/tags/<name>`; delete remote tag: `push --progress <remote> --delete refs/tags/<name>`.
- checkout with fast-forward: after the checkout succeeds, `merge --ff-only @{upstream}`; if that fails the outcome is `done` for the checkout with a message that the fast-forward was not possible.

Dangers: `push` with `force` → `force_push` (message names the remote branch and how many remote commits will be discarded, from the last fetched state); `delete_remote_branch` → always.

Hints and explanations (tests on real git output against local bare remotes):
- push rejected, non-fast-forward → message "The remote has commits you don't have", hints `fetch`, `pull`, `force_push`.
- push with no upstream configured and `setUpstream: false` → the core sets upstream itself when the branch has none (documented choice); a missing remote → explanation.
- pull with diverged branches and no configured mode → hint `choose_pull_mode`.
- pull / fetch-into-local that is not a fast-forward for a non-checked-out branch → explanation.
- authentication failed / could not read username / permission denied (publickey) / host unreachable → one explanation each; outcome arrives promptly.
- After a successful `merge` whose current branch has an upstream or a single remote, the outcome carries hint `push` (set this in Task 2's merge body; test here with a remote present).

- [ ] **Step 1: Failing tests:** `push_sets_upstream_when_missing`, `push_rejected_explains_and_hints`, `force_push_needs_confirmation_and_uses_lease` (assert the logged args contain `--force-with-lease` and never `--force`), `force_with_lease_fails_when_remote_moved_since_fetch`, `pull_current_fast_forwards`, `pull_other_branch_updates_it_without_checkout`, `pull_diverged_asks_for_a_mode`, `fetch_into_local_creates_or_fast_forwards`, `delete_remote_branch_needs_confirmation`, `push_and_delete_tag_on_remote`, `checkout_with_fast_forward`, `push_to_unreachable_remote_fails_fast_and_queue_continues` (remote URL to a closed local port; a following op succeeds), `network_op_reports_progress_events`, `merge_outcome_hints_push`.
- [ ] **Step 2–4** as before. **Step 5: Commit.**

### Task 4: In-progress states — reader, continue, abort, open in editor

**Files:**
- Create: `crates/gg-core/src/git/progress.rs`, `crates/gg-core/src/ops/progress.rs`
- Modify: `repo/manager.rs` (snapshot `inProgress`, read alongside status), `watcher.rs` if the state files are not already covered, `dispatch.rs`, `dto.rs`, `tests/readers.rs`, `tests/ops.rs`

**Interfaces:**
- Produces: `pub async fn read_in_progress(runner: &GitRunner, worktree: &Path) -> Result<Option<InProgress>>`; `RepoSnapshot.in_progress`; `run_op` handles `continue` and `abort`; `Core` command `open_in_editor`.
- Reader: kind from the git dir's state (`rev-parse --git-path` for `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `rebase-merge`, `rebase-apply`; rebase wins when several exist); conflicted paths from `status --porcelain=v2 -z` unmerged entries; rebase step/total from `rebase-merge/msgnum` and `end` (or `rebase-apply/next`, `last`); `subject` from `MERGE_MSG` first line or the stopped commit's subject.
- Continue: the UI confirms first (Task 9), then the core runs `add -- <the paths that were conflicted>` and then `merge --continue` / `rebase --continue` / `cherry-pick --continue` / `revert --continue`, each with editors suppressed. A rebase that stops again returns `stopped`.
- Abort: `merge --abort` / `rebase --abort` / `cherry-pick --abort` / `revert --abort`.
- `open_in_editor`: runs `Settings.editor_command` (split into program and arguments by whitespace with simple quotes; the path appended as a separate argument; spawned directly, no shell) or, when null, the platform opener (`open` on macOS, `xdg-open` on Linux, `cmd /C start "" ` on Windows) on the file, or on the worktree folder when `path` is null. Paths must resolve inside the worktree.

- [ ] **Step 1: Failing tests:** reader — one per kind, `state_started_outside_the_app_is_detected` (plain `git merge` in the fixture then `refresh`), `rebase_reports_step_and_total`, `no_state_is_none`, `conflicted_paths_with_spaces_and_unicode`; ops — `continue_stages_resolved_files_and_finishes_the_merge`, `continue_rebase_that_stops_again_returns_stopped`, `abort_restores_the_branch` for each kind, `continue_with_nothing_in_progress_is_nothing_to_do`, `other_operations_explain_while_in_progress` (checkout during a merge → plain message), `open_in_editor_rejects_paths_outside_the_worktree`, `editor_command_is_split_without_a_shell` (a command containing `;` or `$(…)` is passed literally).
- [ ] **Step 2–4.** Snapshot must update within the normal refresh after an outside change (watcher). **Step 5: Commit.**

### Task 5: Stashes

**Files:**
- Create: `crates/gg-core/src/git/stash.rs`, `crates/gg-core/src/ops/stash.rs`
- Modify: `git/log.rs` or `repo/loader.rs` + `repo/state.rs` + `graph/layout.rs` (stash rows), `dto.rs`, `dispatch.rs`, `tests/readers.rs`, `tests/repo.rs`, `tests/ops.rs`, `src/lib/types.ts`, `src/lib/state/viewOptionsStore.ts`, `src/lib/components/Toolbar.svelte` (Stashes toggle beside Tags), `src/lib/graph/RefLabels.svelte` (stash label style), `e2e/graph.spec.ts`

**Interfaces:**
- Produces: `pub async fn read_stashes(runner, cwd) -> Result<Vec<Stash>>` (`Stash { hash, selector: "stash@{n}", base: hash, index_parent: Option<hash>, untracked_parent: Option<hash>, subject, time }` from `stash list` with an explicit `--format` and `-z`); stash rows in the graph: each stash commit is a row of `kind: "stash"` with one parent (its base), carrying a `RefLabel { kind: "stash" }`; its index and untracked helper commits never appear as rows; hidden when `showStashes` is false; `run_op` handles `stash_save`, `stash_apply`, `stash_pop`, `stash_drop`, `stash_branch`.
- Commands: `stash push [-u] [-m <message>]`; `stash apply|pop|drop <stash@{n}>`; `stash branch <name> <stash@{n}>`. The request names the stash by commit hash; the core resolves the selector at run time inside the queue and fails plainly if that stash no longer exists.
- Danger: `stash_drop` → `drop_stash`.
- Explanations: nothing to stash (`nothing_to_do`); apply/pop with changes in the way; pop that stops on conflicts (the stash is kept — say so; status `failed` with that explanation, since no in-progress state exists).

- [ ] **Step 1: Failing tests:** `stash_list_parses_subjects_with_odd_bytes`, `stash_rows_appear_once_with_one_parent` (a stash with untracked files adds exactly one row), `stashes_hidden_by_view_option`, `save_with_message_and_untracked`, `apply_keeps_and_pop_removes`, `drop_needs_confirmation`, `drop_targets_the_right_stash_after_the_list_shifted` (drop stash B by hash after a newer stash was saved in between), `stash_gone_fails_plainly`, `branch_from_stash`, `pop_with_conflict_keeps_the_stash_and_explains`; e2e: stash row and label visible, toggle hides it, scroll/selection kept.
- [ ] **Step 2–4.** Lane layout tests in `layout.rs` extended for stash rows. **Step 5: Commit.**

### Task 6: UI foundation — menus, dialogs, operation state

**Files:**
- Create: `src/lib/components/ContextMenu.svelte`, `src/lib/components/Dialog.svelte`, `src/lib/components/dialogs/ConfirmDialog.svelte`, `src/lib/state/ops.svelte.ts`, `src/lib/state/ops.test.ts`, `src/lib/menus/types.ts`, `src/lib/menus/commitMenu.ts` + test, `src/lib/menus/branchMenu.ts` + test, `src/lib/menus/tagMenu.ts` + test, `src/lib/menus/stashMenu.ts` + test, `e2e/menus.spec.ts`
- Modify: `src/App.svelte` (the existing `contextmenu` handler), `src/lib/graph/GraphView.svelte` and `RefLabels.svelte` (right-click and the menu key on a row or a label; `rowAt(event)` exists), `src/lib/components/Toolbar.svelte` (busy indicator), `src/lib/components/Toast.svelte` and `state/app.svelte.ts` (notices with several action buttons and a "Show log" link)

**Interfaces:**
- Produces:
  - `type MenuItem = { id: string; label: string; disabledReason?: string; danger?: boolean; submenu?: MenuItem[] } | { separator: true }`
  - `commitMenu(row: CommitRow, snap: RepoSnapshot): MenuItem[]`, `branchMenu(label: RefLabel, row: CommitRow, snap): MenuItem[]` (local and remote), `tagMenu(label, snap)`, `stashMenu(row, snap)`, `uncommittedMenu(snap)`. Item ids are stable strings (`"checkout"`, `"merge"`, `"reset.hard"` …) used by tests and by the dispatcher in Tasks 7–9.
  - `openMenu(at: {x, y}, items: MenuItem[], onPick: (id: string) => void)`; `openDialog(component, props)` / `closeDialog()`.
  - `runOp(repoId: number, request: OpRequest): Promise<OpOutcome>` in `ops.svelte.ts`: sends `run_op`; on `needs_confirmation` opens `ConfirmDialog` listing the dangers and, on confirm, resends with those kinds; applies `outcome.snapshot` to the repo store; shows the result notice (`done`: info with hint buttons; `failed`: error with message, git's text collapsed below, "Show log" link carrying `logFrom`; `stopped`: no toast, the banner of Task 9 shows it); exposes `ops.busy(repoId)` from `snapshot.op`.
- Menu content follows spec §5.2 exactly, minus "create worktree" and pull-request items (Plan 3). Items that cannot apply are shown disabled with the reason as a tooltip and `aria-disabled`: no current branch (detached or unborn) for "merge into current", "rebase current onto", "push"; the current branch for "delete", "merge into current", "checkout"; a branch with no remote for "pull"; an operation in progress for everything except copy items. While an operation runs, menus still open and items stay enabled (requests queue).
- Copy items (name, hash, message) write to the clipboard and show a short notice.
- In this task picking a non-copy item calls a dispatcher stub that Tasks 7–9 fill in; the stub must exist as `src/lib/menus/dispatch.ts` exporting `pick(id, target, snap)`.

Behaviour to build and test:
- Menu: opens at the pointer within one frame, stays inside the window, closes on Escape / outside click / scroll / window blur; arrow keys, Home/End, type-ahead, Enter; submenus open on hover and ArrowRight; focus returns to the row; `role="menu"` semantics; right-click selects the row first; the menu key and Shift+F10 open it for the selected row.
- Dialog: modal, focus trapped and restored, Escape and Cancel close, Enter submits unless focus is on Cancel or in a multi-line field, the primary button is disabled while fields are invalid, dangerous primary buttons are styled as such and are never the default focus.
- Busy: the toolbar shows the running operation's title with a spinner within 100 ms, and a count when more are queued.

- [ ] **Step 1: Failing unit tests** for each menu builder (table tests over: current branch, other local branch, branch behind/ahead, remote branch, detached HEAD, empty repo, operation in progress, stash row, uncommitted row, merge commit) and for `runOp` (confirmation resend, cancel sends nothing, notices per status, hint buttons call the right follow-up request).
- [ ] **Step 2: Failing e2e** in `menus.spec.ts`: right-click a commit / a branch label / a tag label shows the expected item names; keyboard path; disabled reason visible; copy hash puts it on the clipboard; menu closes on scroll.
- [ ] **Step 3: Implement. Step 4: Pass, and look at screenshots of each menu and the confirm dialog in both themes. Step 5: Commit.**

### Task 7: Commit and tag actions

**Files:**
- Create: `src/lib/components/dialogs/CreateBranchDialog.svelte`, `CreateTagDialog.svelte`, `ResetDialog.svelte`, `CherryPickDialog.svelte`, `RevertDialog.svelte`, `DeleteTagDialog.svelte`, `PushTagDialog.svelte`, `src/lib/nameCheck.ts` + test, `e2e/commit-actions.spec.ts`
- Modify: `src/lib/menus/dispatch.ts`

**Interfaces:**
- Consumes: `runOp`, `openDialog`, menu ids from Task 6.
- Produces: every commit and tag menu item works end to end. `nameCheck.ts`: `refNameProblem(name: string, existing: string[]): string | null` — instant feedback mirroring the core's rules (the core remains the authority).

Dialogs (wording is the builder's; fields are fixed):
- Checkout commit: no dialog; the notice says HEAD is detached.
- Create branch: name, "check out after creating" (default on).
- Create tag: name, optional message (makes it annotated).
- Cherry-pick: "record no commit, only apply" (no-commit); for a merge commit, a required parent choice showing each parent's short hash and subject.
- Revert: parent choice for a merge commit; otherwise a one-line confirm.
- Reset current branch to here: soft / mixed / hard with one line each saying what happens to changes; hard goes through the danger confirmation.
- Delete tag: "also delete on remote" with a remote choice when remotes exist. Push tag: remote choice (preselect the only one, or `origin`).

- [ ] **Step 1: Failing e2e**, one per item, each asserting the result in the graph (new label on the right row, HEAD moved, new commit on top) and the notice; plus `hard_reset_asks_twice_and_cancel_changes_nothing`, `invalid_name_disables_the_button_and_says_why`, `cherry_pick_conflict_shows_no_error_toast` (banner comes in Task 9; here assert no failure notice and the snapshot's `inProgress`).
- [ ] **Step 2–5.**

### Task 8: Branch actions and the staging routine

**Files:**
- Create: `src/lib/components/dialogs/MergeDialog.svelte`, `RebaseDialog.svelte`, `RenameBranchDialog.svelte`, `DeleteBranchDialog.svelte`, `PushDialog.svelte`, `PullDialog.svelte`, `CheckoutRemoteDialog.svelte`, `FetchIntoLocalDialog.svelte`, `e2e/branch-actions.spec.ts`, `e2e/staging-routine.spec.ts`
- Modify: `src/lib/menus/dispatch.ts`, `e2e/fixtures.ts` (repositories with a local bare remote and a second clone to move the remote)

**Interfaces:**
- Consumes: `runOp`, hints, `getRepoPrefs` / `setRepoPrefs`.
- Produces: every local- and remote-branch menu item works; the staging routine of spec §5.3.

Dialogs:
- Merge into current: shows "Merge X into Y"; "create a merge commit even if fast-forward is possible" (default from `RepoPrefs.mergeNoFf`, saved back on submit), "squash", "no commit". Squash disables the first.
- Rebase current onto X: one-line confirm naming both branches and that it rewrites the current branch's commits.
- Rename; Delete (plain confirm; the unmerged danger adds the second, explicit confirmation); Delete remote branch (danger confirmation only).
- Push: remote choice, "set upstream" (on when the branch has none), "force (with lease)" off by default.
- Pull: for the current branch a plain action; on hint `choose_pull_mode` a dialog offering Merge or Rebase.
- Checkout remote branch: local name prefilled with the branch name; if that local branch exists, offer to check it out instead.
- Fetch into local: local name prefilled.

Staging routine:
- Checking out a local branch that is behind its upstream and not ahead asks "Fast-forward N commits first?" (Yes / No / Cancel) and sends `fastForward` accordingly; a branch that is level or diverged checks out directly.
- The notice after a successful merge has a **Push** button (from hint `push`) that pushes the current branch.
- A rejected push shows the explanation with **Fetch**, **Pull** and a separate **Force push…** button that leads to the danger confirmation.
- Ahead/behind counts on labels update after each of these without a manual refresh.
- Double-clicking a local branch label checks it out (same path as the menu item).

- [ ] **Step 1: Failing e2e:** one per item; `staging-routine.spec.ts`: `checkout_staging_offers_fast_forward_then_merge_then_push` (the whole §5.3 path in one test, asserting the remote's ref with git at the end), `push_rejected_then_pull_then_push`, `force_push_needs_the_second_confirmation`, `merge_dialog_remembers_no_ff_per_repository`, `delete_unmerged_branch_double_confirm`, `actions_queue_while_one_runs` (two quick actions; both complete in order; busy indicator shows the count).
- [ ] **Step 2–5.**

### Task 9: Conflict banner and stash actions in the UI

**Files:**
- Create: `src/lib/components/ConflictBanner.svelte`, `src/lib/components/dialogs/StashSaveDialog.svelte`, `StashBranchDialog.svelte`, `ContinueDialog.svelte`, `e2e/conflicts.spec.ts`, `e2e/stash.spec.ts`
- Modify: `src/lib/menus/dispatch.ts`, `src/App.svelte` or the repo view (banner placement above the graph), `src/lib/graph/GraphView.svelte` (uncommitted-row menu)

**Interfaces:**
- Consumes: `snapshot.inProgress`, `runOp` (`continue`, `abort`, stash requests), `openInEditor`.
- Produces: the banner of spec §5.6 and all stash items of §5.2.

Banner: persistent while `inProgress` is set; names the operation ("Merging feature/x into staging", "Rebasing, step 2 of 5", …); lists conflicted files (scrollable past 6, each opens in the editor on click); buttons **Abort** (one-line confirm), **Open in editor** (the worktree folder), **Continue** (disabled with a reason while files are still conflicted according to the latest status; on click a confirm that lists the files that will be staged). It updates when files are resolved outside the app (watcher refresh), and appears for states started outside the app. It does not move the graph's scroll position when it appears or goes (the graph area shrinks from the top; the anchor row stays).

Stash: "Stash changes…" on the uncommitted row's menu (message, include untracked); stash row menu: apply, pop, drop (danger confirm), create branch from stash.

- [ ] **Step 1: Failing e2e:** `merge_conflict_shows_banner_with_files`, `continue_disabled_until_resolved_then_finishes` (resolve by writing the file from the test), `abort_returns_to_before`, `rebase_banner_shows_steps_and_continues_through_two_stops`, `state_started_outside_appears`, `banner_does_not_move_the_graph`, `open_in_editor_calls_the_core` (devserver records the call; no real editor in tests); stash: `save_apply_pop_drop_branch` paths, `drop_needs_confirmation`, `stash_untracked_option`.
- [ ] **Step 2–5.** Screenshots of the banner (merge, rebase) in both themes.

### Task 10: Command log panel

**Files:**
- Create: `src/lib/components/CommandLog.svelte`, `src/lib/state/commandLog.svelte.ts` + test, `e2e/command-log.spec.ts`
- Modify: `src/lib/components/Toolbar.svelte` (toggle button), `src/lib/shortcuts.ts` (toggle shortcut, Escape closes), `src/App.svelte` (panel placement, resizable with the existing `Splitter`), `state/app.svelte.ts` ("Show log" links)

**Interfaces:**
- Consumes: `command_log {sinceId, repoId, writesOnly}`, `OpOutcome.logFrom/logTo`.
- Produces: a bottom panel listing commands newest last: time, `git …` command line as text (arguments with spaces quoted for display only), duration, exit code, expandable output (stdout for writes, stderr always). Default view: operations only (`writesOnly`), grouped under their operation title; a toggle shows background reads too. "Show log" in a notice opens the panel scrolled to that operation with it highlighted. A "Copy" button copies a command line. The panel polls only while open (new entries since the last id, on `repo-changed` and at most once a second) and costs nothing while closed. Long output is virtualized or capped so a 16 KiB entry never stalls the UI. All text is rendered as text, with Plan 1's control-character and bidi handling.

- [ ] **Step 1: Failing tests:** store — incremental fetch by id, grouping by `opId`, cap of kept entries; e2e — `operation_appears_with_its_commands_and_output`, `failed_operation_notice_links_to_its_entry`, `reads_hidden_by_default_and_shown_by_toggle`, `panel_closed_makes_no_requests` (devserver request count over 3 s), `escape_closes_and_focus_returns`.
- [ ] **Step 2–5.** Screenshots in both themes.

### Task 11: Verify and polish

**Files:** as needed; `docs/perf/2026-11-plan-2.md`, `docs/perf/screens/`, `README.md`.

- [ ] **Step 1: Use the real app** (`pnpm tauri build --bundles app`, release build) on a scratch clone with a local bare remote: do the whole staging routine, each menu item once, a conflicted merge and rebase through the banner, stash round trip, the command log. Record what was done and seen. A launch counts only if the window painted; say plainly what could not be checked.
- [ ] **Step 2: Screenshots** of every menu, every dialog, the banner, the log panel, busy state and each notice kind, light and dark, at 1280×800 and 900×560; read each PNG and fix what looks wrong (alignment, truncation, contrast, focus rings, danger styling).
- [ ] **Step 3: Measure** with `scripts/perf.sh` (GG_CONFIG_DIR; never the real config): Plan 1's launch, scroll and memory numbers again (no regression beyond noise), plus: right-click to menu painted, menu pick to dialog painted, pick to busy indicator (each under 100 ms), operation end to graph updated. Investigate the open Plan 1 item — idle CPU on the large-working-tree repository (median 0.69 %, one launch at 23.8 %): find the cause with a profile or by elimination (focus events, redraw loop, status reads), fix it if found, otherwise record what was ruled out.
- [ ] **Step 4: Hardening pass** over the Review Focus list: try each item by hand in the app and add a test for any that was not already covered.
- [ ] **Step 5: Docs** — README section on actions, safety confirmations and the command log; keyboard shortcuts table updated.
- [ ] **Step 6: Full verification and commit.**
