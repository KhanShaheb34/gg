# Plan 3 — Worktrees, Pull Requests, Settings, `gg`, Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish version one: rename the app to gg with a `gg` terminal command, make worktrees first-class (panel, active worktree, create, commit, push, delete with warnings), add pull requests through `gh`, a settings screen, and make the repository and builds ready to be public on macOS, Linux and Windows.

**Architecture:** As Plan 2 — the UI sends typed requests, the core runs git/gh as argument arrays and returns typed results. A repository gains a switchable active worktree (`Repo.worktree` becomes mutable state; every op already runs there). Worktree, commit and pull-request actions are new `OpRequest` variants through the existing queue, danger tokens and command log. `gh` runs through the same runner machinery as git (no shell, no terminal, time-limited, logged, masked). A right-hand side panel hosts the worktree list and the commit panel.

**Tech Stack:** As Plans 1–2, plus `tauri-plugin-single-instance`, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-11-12-git-graph-desktop-app-design.md` — §5.4, §5.5, §5.8 (settings, shortcuts), "create worktree"/PR items of §5.2, §9 phases 3–5, and §11.

**Builds on:** Plans 1 and 2. Rulings that still bind: `.superpowers/sdd/2026-11-14-plan-2-actions/progress.md` R1–R44 (notably R17' stopped states, R19 opener, R30 notices, R36 masking, R38 push destination, R39 danger tokens and `expectHead`, R41 no terminal + Stop).

## Global Constraints

- Everything in Plan 1 and Plan 2 Global Constraints still binds.
- The product name is `gg` everywhere a user can see it. "GitGraph" / "Git Graph" appear nowhere in the app, README or site, except one README sentence crediting the VS Code extension as the inspiration.
- Dangerous paths added here cannot run unconfirmed, enforced in the core with a state token (R39): deleting a worktree that has uncommitted changes, untracked files, or commits on no remote; deleting its branch when unmerged/unpushed; `worktree remove --force`; discarding anything. A clean, fully pushed worktree is deleted without a red dialog.
- Every action that needs a working directory runs in the active worktree; the active worktree is always visible in the window; an op carries the worktree it was asked for (`expectWorktree`, checked in the core like `expectHead`) so switching worktrees can never redirect a queued op.
- `gh` is run only as an argument array through the runner: `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1`, `NO_COLOR=1`, no terminal, stdin null, 60 s limit, logged and masked like git. The app never reads, stores or asks for a GitHub token. Background PR loading happens only alongside a fetch (scheduled or user) and never more than once per 60 s per repository.
- The app never needs administrator rights. Installing the `gg` command writes one file/symlink into a user-writable directory already on the login `PATH`, only when the user asks, and says exactly what it wrote.
- Programs the user configures (editor, terminal) are run as argument arrays, never through a shell; the path is passed as one argument (R19 stays: no launch-by-type of repository files).
- No macOS-only API without a fallback; every `cfg(unix)` path has a working `cfg(windows)` counterpart or a plain "not available on this platform" message, never a silent no-op.
- No private data in the repository: no absolute home paths, no email addresses, no tokens, in code, docs, fixtures or screenshots.
- Speed targets (spec §7) must not regress; re-measured in the last task. Opening the side panel or switching worktree gives feedback within 100 ms.
- Tests accompany every task. `cargo test --workspace`, clippy (default and `--features perf`, `-D warnings`), `cargo fmt --check`, `pnpm check`, `pnpm test`, `pnpm e2e` pass with clean output before a task is reported done.
- Commits: small, conventional subjects. **Never add any AI attribution.** Work on branch `dev`. Do not push, do not merge. Never contact a real network host in tests (`gh` is replaced by a recording fake on `PATH`). Never write into a repository that is not a test fixture. No sudo.

## Review Focus

1. **Deleting a worktree with work in it** — uncommitted, untracked-only, ignored-only, commits on no remote, detached HEAD with unreachable commits, locked, the active worktree, the main worktree, a missing directory: each either refuses plainly or lists exactly what would be lost before a confirmed force; state changed between dialog and confirm re-asks. *(Tests: Task 4.)*
2. **Acting in the wrong worktree** — an op queued, then the active worktree switched; two worktrees with the same branch name pattern; a worktree removed in the terminal while it is active: the op is refused or the app falls back to the main worktree with a notice; nothing runs elsewhere. *(Tests: Task 3.)*
3. **Committing the wrong files** — ticked subset with staged-elsewhere changes, renames, deletions, untracked, paths with spaces/unicode/leading `-`, a file changed between listing and commit, an empty message, a failing hook: exactly the ticked paths are committed or nothing is; hook output shown. *(Tests: Task 5.)*
4. **`gh` absent, signed out, wrong host, slow or rate-limited; fork remotes; a branch whose PR is closed/merged** — said once, compare-page fallback, never a hang or a repeated notice; markers never go stale silently. *(Tests: Task 8.)*
5. **`gg` from odd places** — a non-repository, a subfolder, a linked worktree, a bare repo, a path with spaces, a relative path, the app not running / already running / mid-launch, a shell alias shadowing `gg`. *(Tests: Task 2.)*

## File Structure

Core (`crates/gg-core/src/`): `git/worktrees.rs` (richer reader), `git/status.rs` (file list), `repo/manager.rs` (active worktree, per-worktree state), `watcher.rs` (working-tree watch), `ops/worktree.rs`, `ops/commit.rs`, `ops/pr.rs`, `gh.rs` (runner + readers for `gh`), `hosts.rs` (remote URL → host/owner/repo, compare URL), `launch.rs` (terminal, reveal; `editor.rs` joins it), `cli.rs` (install/uninstall the `gg` command), `store.rs` (new settings, config migration).
App (`src-tauri/`): single instance, argument handling, `gg` launcher resource, rename.
UI (`src/lib/`): `components/SidePanel.svelte`, `WorktreePanel.svelte`, `CommitPanel.svelte`, `Settings.svelte`, `components/dialogs/{CreateWorktree,DeleteWorktree,CreatePullRequest}Dialog.svelte`, `menus/worktreeMenu.ts`, `state/worktrees.svelte.ts`, `state/prs.svelte.ts`, `state/settings.svelte.ts`.
Repo: `LICENSE`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `.github/workflows/{ci,release}.yml`, `scripts/release-macos.sh`.

## Boundary additions (authoritative for all tasks)

```ts
type WorktreeInfo += {
  active: boolean;
  status: WorktreeStatus | null;          // null until read; counts as today
  upstream: { name: string; ahead: number; behind: number } | null;  // null = no upstream
  unpushed: number | null;                // commits on no remote-tracking ref; null = unknown
  inProgress: InProgress["kind"] | null;
  pr: PullRequest | null;
}
type RepoSnapshot += { prs: PrState }
type PrState = { status: "unknown" | "loading" | "ready" | "unavailable"; reason: PrUnavailable | null; loadedAtMs: number | null }
type PrUnavailable = "no_gh" | "signed_out" | "not_github" | "failed"
type PullRequest = { number: number; title: string; url: string; state: "open" | "draft"; head: string; base: string }
type RefLabel += { pr: { number: number; state: "open" | "draft" } | null }      // local and remote branch labels
type FileEntry = { path: string; origPath: string | null; status: "M"|"A"|"D"|"R"|"C"|"T"|"U"|"?"; staged: boolean; unstaged: boolean }
type Settings += { terminalCommand: string | null; worktreePattern: string; dateFormat: "relative" | "iso" | "locale" }
//   worktreePattern default "../{repo}.worktrees/{branch}"; placeholders {repo} {branch}; relative to the main worktree
type RepoPrefs += { prBase: string | null; sidePanel: boolean }                   // sidePanel default true
type AppState += { cli: { installed: string | null; shadowedBy: string | null; candidates: string[] } }

// Every existing OpRequest that runs in a working directory gains: expectWorktree: string
type OpRequest |=
  | { op: "worktree_add"; path: string; branch: { existing: string } | { create: string; at: string } | { detach: string } }
  | { op: "worktree_remove"; path: string; deleteBranch: boolean }
  | { op: "worktree_prune" }
  | { op: "commit"; paths: string[]; message: string; push: boolean; expectHead: HeadInfo; expectWorktree: string }
  | { op: "pr_create"; branch: string; base: string; title: string; body: string; draft: boolean }
type DangerKind |= "remove_worktree_with_work" | "delete_unmerged_branch_with_worktree"
type OpHint |= { kind: "make_active"; worktree: string } | { kind: "open_url"; url: string; label: string }

// Commands (Core::dispatch):
set_active_worktree { repoId, path } -> RepoSnapshot
worktree_files { repoId, path } -> { files: FileEntry[]; truncated: boolean }     // cap 5000
worktree_default_path { repoId, branch } -> { path: string; exists: boolean }
pr_suggest { repoId, branch } -> { title: string; body: string; base: string; existing: PullRequest | null; compareUrl: string | null }
open_external { repoId, target: "editor" | "terminal" | "reveal" | "url"; path?: string; url?: string } -> { note: string | null }
install_cli { dir: string } -> AppState["cli"];  uninstall_cli {} -> AppState["cli"]
```

---

### Task 1: Rename to gg

**Files:** `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `crates/gg-core/src/store.rs`, `index.html`, `package.json`, `scripts/*.sh`, `README.md`, UI strings, e2e text assertions.

**Interfaces:** Produces: `productName` `gg`; bundle `gg.app`; identifier `com.shakirul.gg`; binary name `gg-desktop` (the short name `gg` is the launcher's, Task 2); `store::migrate_config(old_dir, new_dir) -> Result<bool>`.

- [ ] Test (store): `migrate_config` copies `state.json` from a `dev.gitgraph.desktop` config dir into an empty new dir exactly once, never overwrites an existing new `state.json`, leaves the old dir in place, and is skipped when `GG_CONFIG_DIR` is set.
- [ ] Rename product, window title, identifier, binary, scripts (`perf.sh`, `walk.sh` bundle paths and process names), README, About/menu strings. Call `migrate_config` once at startup before the store loads.
- [ ] `rg -i "git ?graph"` finds only: the spec, plan and perf docs (history), and one README credit line. Add that as a test (`tests/naming.rs` scanning `src/`, `src-tauri/`, `crates/`, `README.md`).
- [ ] `pnpm tauri build --bundles app` yields `gg.app`; launch it with a fresh `GG_CONFIG_DIR`, screenshot, confirm title/menu say gg. Commit.

### Task 2: The `gg` command and one app instance

**Files:** `src-tauri/Cargo.toml` (+`tauri-plugin-single-instance`), `src-tauri/src/lib.rs`, `src-tauri/resources/gg` (POSIX sh launcher), `src-tauri/resources/gg.cmd`, `crates/gg-core/src/cli.rs`, `dispatch.rs`, UI: app menu item + Welcome hint.

**Interfaces:** Consumes: `paths-opened` event and `take_opened_paths` (Plan 1). Produces: `install_cli`, `uninstall_cli`, `AppState.cli`; launcher contract below.

Launcher contract (`gg [path]`): resolve `path` (default `.`) to absolute; run `git -C <path> rev-parse --show-toplevel`; on failure print `gg: <path> is not inside a git repository` to stderr, exit 1; on success start the app with the absolute path as its argument, detached, and exit 0 at once. `gg --version`, `gg --help` print and exit 0. The launcher finds the app from its own real location (it is a symlink into `gg.app/Contents/Resources/`), so moving the app does not break it; if the app is gone it says so and exits 1. A second app instance forwards its arguments to the first (`paths-opened`), which opens or focuses that repository's tab and comes to the front.

- [ ] Tests (`cli.rs`): `candidates()` lists only directories that are on the login `PATH`, exist, are writable by the user and are not inside a repository (order: `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, then other writable `PATH` entries under the home dir); `install(dir)` creates a symlink named `gg`, refuses to replace a file that is not a previous install of ours (says what is there), is idempotent; `uninstall` removes only our symlink; `shadowed_by()` reports an alias/function/earlier `PATH` entry named `gg` by asking the login shell (`$SHELL -ic 'command -V gg'`, 3 s limit) and never fails the install because of it.
- [ ] Tests (launcher, run with `sh` against fixture repos, the app replaced by a recording stub via `GG_APP_BIN`): every case of Review Focus 5 — subfolder → toplevel passed; linked worktree → that worktree's path; bare repo and non-repo → message + exit 1; path with spaces; relative path; `-`-leading path handled with `--`.
- [ ] Single instance: second launch with a path emits `paths-opened` in the first and exits; without a path it just focuses. Test in the real app: start, run the launcher twice with two fixture repos, both tabs present, one process.
- [ ] UI: app menu "Install ‘gg’ Command…" opens a small dialog: the directory choice (candidates), what will be written, and, when `shadowedBy` is set, the plain warning "Your shell already defines `gg` as <what>; that wins over this command until you remove it" (the app never edits shell files). Welcome screen shows the one-line hint when not installed. vitest + e2e.
- [ ] README section "Open from the terminal". Commit.

### Task 3: Active worktree, richer worktree state, working-tree watching

**Files:** `git/worktrees.rs`, `repo/manager.rs`, `repo/state.rs`, `watcher.rs`, `dto.rs`, `dispatch.rs`, `ops/mod.rs` (+`expectWorktree`), `src/lib/types.ts`, `menus/dispatch.ts`, `state/ops.svelte.ts`.

**Interfaces:** Produces: `set_active_worktree`; `WorktreeInfo` additions (all but `pr`); `expectWorktree` on every working-directory op, refused with `"That was asked for in <a>, but <b> is the active worktree now."`; store remembers the active worktree per repository (falls back to main when gone).

- [ ] Tests (`tests/worktrees.rs`): snapshot lists status/upstream/unpushed/inProgress per worktree for: clean, dirty, untracked-only, no upstream, ahead 2 behind 1, detached, locked, prunable (directory deleted), mid-merge. `unpushed` = `rev-list --count HEAD --not --remotes`.
- [ ] Tests: `set_active_worktree` changes where the next op runs, bumps `seq`, moves the uncommitted row, status and `inProgress` to that worktree, is refused while an op runs or is queued, refuses a prunable path; an op carrying a stale `expectWorktree` is refused; the active worktree removed in a terminal → falls back to main with one `repo-error` notice.
- [ ] Per-worktree status is read lazily and cheaply: active worktree as today; others on open, on their git-dir change, on window focus, at most 4 at a time, each with the 60 s limit; never blocks the snapshot.
- [ ] Working-tree watch: the active worktree only, recursive, honouring ignore rules cheaply (skip `.git`, and directories git reports ignored via one `git ls-files -o -i --exclude-standard --directory -z` read at start and on `.gitignore` change); events debounce 500 ms / max 2 s into `request_status` only (never a history reload). If the platform refuses the watch (too many files), say so once and fall back to focus refresh. Test: editing a file updates `status.changed` within 2 s with no generation bump; a build writing 10,000 files into an ignored dir causes no status read. Idle CPU with a watched large tree is measured in Task 13.
- [ ] Opening a linked worktree's path (app or `gg`) opens the repository and makes that worktree active (replaces the R42 notice). Commit.

### Task 4: Worktree operations in the core

**Files:** `ops/worktree.rs`, `ops/danger.rs`, `ops/explain.rs`, `git/names.rs`, `store.rs` (`worktreePattern`), `tests/ops_worktree.rs`.

**Interfaces:** Produces: `worktree_add`, `worktree_remove`, `worktree_prune`, `worktree_default_path`; dangers `remove_worktree_with_work`, `delete_unmerged_branch_with_worktree`; hint `make_active`.

- [ ] Tests, add: existing branch; new branch at a commit; detached; default path from the pattern (`{branch}` slugged: `/`→`-`, other unsafe characters dropped, collisions get `-2`); refuses an existing non-empty directory, a path inside any worktree, a branch already checked out (explains where + `make_active` hint), an invalid name; parent directories are created; the new worktree appears in the snapshot and is NOT made active automatically.
- [ ] Tests, remove (Review Focus 1, one test per case): clean + pushed → removed without a danger; each kind of work → `needs_confirmation` whose message counts and whose detail lists the files/commits (cap 20 + "and N more"), token = worktree HEAD + status hash + unpushed list hash; confirm with a stale token re-asks; confirmed → `worktree remove --force`; ignored-only files are named as "ignored files will be deleted too" but need no red dialog unless there is other work; locked → refused with the lock reason; main worktree → refused; active worktree → allowed, main becomes active first; missing directory → suggests prune. `deleteBranch` runs after removal and carries the R9 unmerged danger plus "not pushed anywhere" when so.
- [ ] Tests, prune: removes only entries git calls prunable; lists what it pruned in the outcome message.
- [ ] Every existing "branch is checked out elsewhere" failure (checkout, delete, rebase…) gains the `make_active` hint naming the worktree (spec §5.4 last paragraph). Commit.

### Task 5: File list and commit in the core

**Files:** `git/status.rs`, `ops/commit.rs`, `dto.rs`, `dispatch.rs`, `tests/ops_commit.rs`.

**Interfaces:** Produces: `worktree_files`, `commit` op.

- [ ] Tests (reader): `FileEntry` for modified, added, deleted, renamed (with `origPath`), type change, untracked, unmerged, staged+unstaged on one file, paths with spaces/unicode/newline/leading `-`; submodules ignored as today; `truncated` at 5000.
- [ ] Tests (commit, Review Focus 3): commits exactly `paths` using a temporary index (`GIT_INDEX_FILE` copy → `add -A -- <paths>` via `--pathspec-from-file=- --pathspec-file-nul` → `commit`), so the user's own index is untouched when the commit fails and afterwards reflects the commit; unticked staged changes stay staged; deletions and renames work; empty `paths` or blank message → `nothing_to_do`/`bad_request` before git runs; refused while stopped (R17') except when `inProgress` is a merge with no conflicts left (then it concludes the merge with all files, message prefilled by git); hooks run; a failing `pre-commit`/`commit-msg` hook → `failed` with the hook's output as detail; identity missing → plain explanation; `push: true` pushes per R38 after a successful commit and reports a push failure separately ("Committed. The push failed: …" with the push hint); `expectHead`/`expectWorktree` enforced; a signing prompt fails fast (R41) with the README pointer.
- [ ] Commit.

### Task 6: Side panel and worktree list (UI)

**Files:** `SidePanel.svelte`, `WorktreePanel.svelte`, `menus/worktreeMenu.ts`, `state/worktrees.svelte.ts`, `App.svelte`, `detailsLayout.ts`, `Toolbar.svelte`, `shortcuts.ts`, `RefLabels.svelte`, e2e `worktrees.spec.ts`, fixtures `worktrees` repo.

**Interfaces:** Consumes: Task 3–4 boundary. Produces: right-hand side panel (resizable 240–480 px, default 300, remembered; `RepoPrefs.sidePanel`; `Cmd+B` toggles; at window widths under 1000 px it overlays the graph instead of squeezing it); `openDialog("createWorktree" | "deleteWorktree", …)`.

- [ ] Menu builder tests (`worktreeMenu.test.ts`): items per state — Make active (disabled on active/prunable, with reason) · Commit… · Push · Create pull request / Open pull request (Task 9) · Open in editor · Open in terminal · Reveal in file manager · Lock state shown · Delete… (disabled on main with reason) · Prune stale entries (panel menu).
- [ ] Panel rows: shortened path (home as `~`, common parent elided), branch or detached short hash, active marker, change count, ahead/behind or "no upstream", "unpushed" when on no remote, locked / missing badges, in-progress badge. Single click selects and scrolls the graph to that worktree's HEAD; double click or the row's button makes it active. Empty state for a single-worktree repo explains worktrees in one sentence with an "Add worktree" button.
- [ ] The active worktree is always shown in the repo header (name + branch), clickable to open the panel; the graph marks the active HEAD most prominently (spec §5.1).
- [ ] Dialogs: Create worktree (branch: existing | new at <ref>; location prefilled from `worktree_default_path`, editable, inline validation; "Make active after creating" off by default) — reached from the panel's add button, branch menu "Create worktree from …" and commit menu "Create worktree here…" (R2 lifted). Delete worktree (plain confirm when clean; the core's red danger when not; "Also delete branch <x>" off by default).
- [ ] e2e: list reflects a worktree added in the terminal within 1 s; make active moves the uncommitted row; create/delete round trip; delete with dirty files shows the list and needs the red confirm; keyboard reach (F6 cycle includes the panel, arrow keys, Enter, menu key). Screenshots light/dark, 1280×800 and 900×560. Commit.

### Task 7: Commit panel and external launchers (UI + core)

**Files:** `CommitPanel.svelte`, `state/worktrees.svelte.ts`, `launch.rs` (absorbs `editor.rs`), `dispatch.rs` (`open_external`), `store.rs` (`terminalCommand`), e2e `commit.spec.ts`.

**Interfaces:** Consumes: Task 5. Produces: `open_external`; commit panel inside the side panel (opened by "Commit…" on a worktree or by double-clicking the uncommitted row).

- [ ] Core tests (`launch.rs`): terminal plan per OS with no setting — macOS `open -a Terminal <dir>`; Linux first of `x-terminal-emulator`, `gnome-terminal --working-directory=`, `konsole --workdir`, `xterm` found on `PATH`, else a plain message; Windows `wt.exe -d <dir>` else `cmd.exe /K` started in `<dir>` with no arguments derived from the repository. With `terminalCommand`: split as the editor command is, `{path}` replaced or the path appended as one argument. Reveal: `open -R`, `explorer.exe /select,`, `xdg-open <parent>`. `url`: only `https://` URLs, via the system opener. Launch recording as for the editor.
- [ ] Commit panel: file list (status letter, path, rename shown as `old → new`), checkbox each, all ticked by default, tri-state "all" box, count line; message field (first line counter at 72, soft); **Commit** and **Commit & Push** (label names the destination per R38); disabled with reasons (nothing ticked, empty message, stopped state); list refreshes on status change without losing ticks or the message; the message draft survives closing the panel (per worktree, session only); hook failure shows the output with Show log; success clears the form and the notice offers Push when not pushed. Commit panel works for the active worktree; choosing "Commit…" on another worktree offers to make it active first (one click).
- [ ] e2e: commit a subset; commit & push to a local remote; failing hook; truncated list message at 5000. Screenshots. Commit.

### Task 8: Pull requests in the core

**Files:** `gh.rs`, `hosts.rs`, `ops/pr.rs`, `repo/fetch.rs` (load after fetch), `dto.rs`, `tests/prs.rs`, `testutil` fake `gh`.

**Interfaces:** Produces: `RepoSnapshot.prs`, `WorktreeInfo.pr`, `RefLabel.pr`, `pr_suggest`, `pr_create`, hint `open_url`.

- [ ] Tests (`hosts.rs`): parse `https://`, `ssh://`, `git@host:owner/repo(.git)`, with ports/userinfo (masked), GitHub Enterprise hosts; compare URL `https://<host>/<owner>/<repo>/compare/<base>...<head>?expand=1` with each ref percent-encoded; non-GitHub hosts → `null` except the same pattern for hosts named `github.*`.
- [ ] Tests (reader, fake `gh` on `PATH` recording argv and replaying JSON): `gh pr list --state open --limit 200 --json number,title,url,isDraft,headRefName,baseRefName,headRepositoryOwner,isCrossRepository` → PRs matched to local branches by head name and, for cross-repository PRs, only when the branch's push remote owner matches; `gh` missing → `unavailable/no_gh`; exit with "not logged in"/`gh auth login` text → `signed_out`; non-GitHub remote → `not_github` without running `gh`; timeout or other failure → `failed`, previous markers kept and marked stale; loaded only after a fetch and at most once per 60 s; the unavailable reason is surfaced once per repository per app run.
- [ ] Tests (`pr_suggest`): title = the single commit's subject, or the branch name humanised when several; body = the commit subjects as a list; base = `RepoPrefs.prBase`, else `gh repo view --json defaultBranchRef`, else `origin/HEAD`, else `main`; `existing` set when an open PR has that head.
- [ ] Tests (`pr_create`): pushes first when the branch has no upstream or is ahead (R38), then `gh pr create --head <b> --base <base> --title <t> --body-file - [--draft]` with the body on stdin; outcome message carries the URL and an `open_url` hint; remembers `prBase`; already exists → `open_url` to it; unavailable → `failed` with the plain reason and, when buildable, an `open_url` hint to the compare page.
- [ ] Commit.

### Task 9: Pull requests in the UI

**Files:** `state/prs.svelte.ts`, `CreatePullRequestDialog.svelte`, `menus/branchMenu.ts`, `menus/worktreeMenu.ts`, `RefLabels.svelte`, `WorktreePanel.svelte`, e2e `prs.spec.ts` (devserver started with the fake `gh`).

- [ ] Menu tests: branch and worktree menus show "Create pull request…" or "Open pull request #N" (R2 lifted); disabled with the reason when unavailable and no compare URL; "Open compare page" when only the fallback exists.
- [ ] Marker on branch labels and worktree rows (number, draft dimmed, tooltip title); stale markers dimmed with "as of <time>".
- [ ] Dialog: title, description, base (select of remote branches, default from `pr_suggest`), draft; primary "Create pull request" / "Push and create pull request"; pending state per R27; success notice with "Open" and "Copy link". e2e for create, existing, signed-out fallback. Screenshots. Commit.

### Task 10: Settings screen, shortcuts, Plan 2 leftovers

**Files:** `Settings.svelte`, `state/settings.svelte.ts`, `store.rs`, `shortcuts.ts`, `format.ts` (dates), `ops/stash.rs`, `ops/network.rs`, `runner.rs`, `Toolbar.svelte`.

- [ ] Settings (`Cmd+,`, app menu, toolbar): theme; fetch interval (off, 1, 5, 15, 60 min); editor command; terminal command; worktree location pattern (live preview for the current repository); date format; the `gg` command (install/uninstall, Task 2); "Reset to defaults". Each field validates inline, saves on change, shows the settings-file path, and survives a relaunch (store tests + e2e).
- [ ] Date format applies to the graph, details and log (vitest for the three formats).
- [ ] Shortcuts added and listed in a "Keyboard shortcuts" sheet (`?` or Help menu): side panel, settings, commit panel, switch worktree (`Cmd+Shift+1..9`), plus the existing table.
- [ ] Leftovers, each with a test: stash apply/pop carry `expectHead` + `expectWorktree` (O2); a stale `index.lock` older than the app's own last git child and with no live owner is named in the failure with a confirmed "Remove the lock file" action, never removed silently (M5); a `pull.ff=only` pull that cannot fast-forward offers the merge/rebase choice (`choose_pull_mode`); the busy indicator shows git's last progress line after 1 s (`op-progress`).
- [ ] Commit.

### Task 11: Other platforms and CI

**Files:** `.github/workflows/ci.yml`, platform `cfg` sites listed in the code map, `tauri.conf.json` (+`tauri.linux.conf.json`, `tauri.windows.conf.json`), `TabBar.svelte`, `platform.ts`, e2e fixtures.

- [ ] CI workflow: on push and pull request; matrix macos-latest, ubuntu-latest, windows-latest; steps: toolchain, pnpm, Linux webkit/gtk packages, `cargo fmt --check`, clippy, `cargo test --workspace`, `pnpm check`, `pnpm test`, `pnpm e2e` (Chromium), `pnpm tauri build` with the platform's default bundles as an artifact. Actions pinned by commit SHA; `permissions: contents: read`; no secrets.
- [ ] Windows paths: every `cfg(unix)` site gets its Windows counterpart — process tree end via Job Objects (or `taskkill /T` as a logged fallback), `CREATE_NO_WINDOW` on every child, no-terminal equivalent of `setsid`, login `PATH` = inherited, `gg.cmd` launcher + install into a user `PATH` dir, file identity without inodes, path comparisons case-insensitive where the filesystem is, `dunce` in test helpers (Plan 1 deferred). `cargo check --target x86_64-pc-windows-msvc` and `--target x86_64-unknown-linux-gnu` for `gg-core` must pass locally (install the targets; if a C dependency blocks cross-checking, say which and rely on CI).
- [ ] Linux/Windows window chrome: native title bar (no overlay/traffic-light inset), menu reachable, Ctrl shortcuts.
- [ ] Report honestly in `docs/platforms.md` what was verified where: built+tested locally (macOS), compiled-only locally, to be verified by CI on first push (the owner pushes). Commit.

### Task 12: Public repository and release packaging

**Files:** `LICENSE` (MIT, "Copyright (c) 2026 Shakirul"), `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `.github/workflows/release.yml`, `scripts/release-macos.sh`, `.gitignore`, issue/PR templates.

- [ ] Audit, with a script kept as `scripts/audit-public.sh` and run in CI: no absolute home paths, email addresses, tokens or hostnames of private remotes in tracked files (code, docs, fixtures, screenshots' text is out of reach — re-check screenshot content by eye and retake any that show a private path or repository name); list every finding and fix it. Report (do not rewrite) what the git history contains that the owner should know before going public: author email on commits, file names, any secret-looking strings (`git log -p` scan with the same patterns).
- [ ] README for strangers: what it is (one screenshot), install (download, the not-notarized note with both ways to open), `gg` command, features, requirements (git 2.29+, `gh` optional), build from source, how it works (uses your git; command log), privacy (no telemetry, no network except your remotes and `gh`), licence, credit line to the Git Graph extension as inspiration. Development docs (`docs/superpowers/`, `docs/perf/`) stay, with a one-paragraph `docs/README.md` saying what they are.
- [ ] `scripts/release-macos.sh`: universal build (`--target universal-apple-darwin`), ad-hoc signed (`codesign -s -`), `.dmg` with an Applications link via `hdiutil` (no AppleScript window styling — the Plan 1 DMG flake), `gg-macos-universal.dmg` + `.sha256` + `latest.json` into `dist-release/` (git-ignored). Run it; mount the dmg, copy the app out, launch it with a fresh `GG_CONFIG_DIR`, confirm it starts on this Mac; report size.
- [ ] `release.yml`: on a `v*` tag, builds macOS universal dmg, Linux AppImage + deb, Windows NSIS exe, attaches them to a draft GitHub release with checksums. Not run here (needs a push; the owner's action).
- [ ] Version set to `0.3.0` in one place. Commit.

### Task 13: Verify and polish

- [ ] Real app, both themes, 1280×800 and 900×560, on fixture repositories: every Task 1–10 feature used end to end — `gg` from a terminal into a running and a not-running app; worktree create → edit a file → panel count updates by itself → commit subset → push → create PR (fake `gh`) → delete worktree (clean, then one with work); settings changes take effect; the staging routine still works. Fix what is rough; polish outranks everything.
- [ ] Perf re-measure (r20k; large working tree with the watch on): launch, idle CPU, idle memory, scroll, side-panel open, worktree switch → `docs/perf/2026-11-plan-3.md`, against Plans 1–2, with causes for any miss.
- [ ] All screenshots retaken under the new name into `docs/screens/` (the site uses them). README final pass. Full suites green. Report what was verified in the real app, what only in the browser build, and what not at all.
