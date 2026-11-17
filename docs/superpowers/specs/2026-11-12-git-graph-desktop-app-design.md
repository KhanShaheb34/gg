# Lightweight Git Graph Desktop App — Design

Date: 2026-11-12
Status: awaiting approval
Working name: `git-graph` (placeholder; the final name is the owner's to pick and does not affect the build)

Items marked **(call)** were decided by the design author, not by the owner. Everything else comes from the owner's own description.

## 1. Why this exists

The owner codes with AI agents (orchestrated through Orca) and uses git and GitHub at work. For anything beyond trivial git, they open Cursor or VS Code purely to use the Git Graph extension: a visual commit graph where click and right-click do checkout, merge, rebase, cherry-pick and so on. Opening a full editor for that is slow and resource-hungry, and leaving it open is worse.

They also run several agents at once on the same codebase, each in its own git worktree, and have found no UI that makes worktrees comfortable: they cannot easily see which worktrees exist, which branch each is on, or commit, push and delete one by hand. Today they ask the agent to do that final step; they would rather do it themselves, carefully, and see what is happening.

This app replaces "open Cursor for Git Graph" and makes worktrees easy to see and manage.

### The two jobs it must do well

1. **Ship to staging (almost daily).** Check out `staging`, merge a feature branch into it, push. Few clicks, clear feedback, safe.
2. **Look around.** Read the commit log for a branch; see what has landed on the feature and fix branches the owner and a teammate are working on.

### The worktree job

Agents work in separate worktrees. When one finishes, the owner wants to: see which files changed, commit, push, create a pull request if none exists, then delete the worktree — with a warning if anything is uncommitted or unpushed.

## 2. Goals, non-goals, anti-goals

**Goals**

- Everything the owner does with Git Graph today, in a standalone app.
- Worktrees as a first-class feature.
- Light enough to leave open all day; opens fast; never laggy.
- Visually polished, with light and dark themes.
- Open any repository folder; recent repositories list.
- Multiplatform codebase.

**Non-goals for version one**

- Diff viewer (the owner opens a code editor for diffs).
- Conflict resolution editor.
- Interactive rebase, blame, file history, submodules.
- Issue linking and the other repository-settings extras in Git Graph.
- Code signing, notarization, auto-update, store distribution.
- Storing GitHub tokens or implementing GitHub auth.

**Anti-goals (it has failed if…)**

- It feels heavy: slow to open, noticeable CPU or memory while idle, janky scrolling.
- An action loses work without a warning (deleting a worktree with uncommitted or unpushed work, hard reset, force push).
- The UI looks rough or unfinished. Polish outranks being fully native — the owner chose this trade explicitly.
- The owner cannot tell what the app just did or why something failed.

## 3. How it gets made

**Decided by the owner**

- Tauri: Rust core, UI in the system web view. Chosen over iced/GPUI because polish matters most and agents write all the code.
- Agents write all code. The owner judges by using the app, so the UI must be verifiable by agents (run it, click through, screenshot) before it is handed over.
- Speed is a requirement to be achieved through deliberate optimization.

**Decided by the design author (call)**

- Tauri 2. Frontend: Svelte 5 + TypeScript + Vite, plain CSS with custom properties for theming. No heavy component library. The builder may substitute another small framework if they have a strong reason, but not one that noticeably raises memory or startup time.
- All git operations run the user's installed `git` binary as a child process (argument arrays, never through a shell). This keeps SSH keys, credential helpers, hooks and config identical to the terminal. No libgit2/gitoxide in version one unless measurement shows the CLI cannot meet the speed targets for reads.
- GitHub pull requests go through the installed `gh` CLI.
- macOS is the build-and-test platform for version one. Code stays cross-platform (no macOS-only APIs without a fallback); Windows and Linux builds are verified in the last phase.
- Package manager: pnpm. Installed toolchain at design time: git 2.54, gh 2.101, rustc 1.98, node 24, pnpm 10.

## 4. Core concepts

**Repository.** A git repository opened by folder. Opening any path inside a repo, including a linked worktree, resolves to the same repository. Several repositories can be open as tabs. Recents persist across launches; the last session's tabs are restored.

**Active worktree.** A repository has one main worktree and zero or more linked worktrees. Exactly one is *active* in the app at a time. Every action that needs a working directory (checkout, merge, rebase, cherry-pick, reset, stash, commit, pull) runs in the active worktree. The active worktree is always visible in the window chrome, and switching it is one click in the worktree panel. The graph itself is shared, since all worktrees share one history.

**Operation.** One user-triggered git action. Operations on a repository run one at a time through a queue; the UI shows a busy state for the running one. Each operation records the exact git commands run, their exit codes and output.

**In-progress state.** A merge, rebase, cherry-pick or revert that stopped on conflicts. It belongs to a specific worktree.

## 5. Features (version one)

### 5.1 Graph view — the main screen

- Rows: graph lanes with coloured lines and commit dots, description with ref labels (local branches, remote branches, tags, stashes, HEAD), date, author, short hash. Resizable columns.
- Local and remote labels for the same branch at the same commit are combined, as Git Graph does (`staging | origin`).
- Each worktree's checked-out commit is marked, with the active worktree's HEAD most prominent. Uncommitted changes in the active worktree appear as a row at the top.
- Branch filter (all, or a chosen set), show/hide remote branches, show/hide tags and stashes.
- Search commits by message, author, hash; jump between matches.
- Clicking a commit expands a details area: full hash, parents (clickable), author, committer, date, full message, and a tree of changed files with added/deleted line counts. No diff.
- Merge commits are visually de-emphasised.
- Fetch button; automatic background fetch (`fetch --all --prune`), default every 5 minutes, configurable, can be turned off **(call)**.
- Automatic refresh when the repository changes on disk (an agent commits, a branch moves, a worktree is added), via file watching on the git directory with debouncing. Refresh must preserve scroll position and selection.

### 5.2 Right-click actions

Branch (local):
checkout · rename · delete · merge into current branch · rebase current branch onto it · push · pull · create worktree from it · create pull request / open existing pull request · copy name

Branch (remote):
checkout as local tracking branch · delete remote branch · fetch into local · merge into current · copy name

Commit:
checkout (detached) · create branch here · create tag here · cherry-pick · revert · reset current branch to here (soft / mixed / hard) · create worktree here · copy hash · copy message

Tag: delete · push · copy name

Stash: apply · pop · drop · create branch from stash. Saving a stash is available from the uncommitted-changes row and the worktree panel (with optional message and include-untracked).

Dialogs mirror Git Graph's options where they exist. Merge dialog: "create a merge commit even if fast-forward is possible" (default on, remembered per repository **(call)**), squash, no-commit.

### 5.3 The staging routine

The path checkout → merge → push must be smooth:

- Checking out a branch that is behind its upstream offers to fast-forward it first.
- After a successful merge, the result notice carries a **Push** button.
- A branch that is ahead of or behind its upstream shows the counts on its label.
- Push that would be rejected explains why and offers fetch/pull; force push is a separate, confirmed choice and uses `--force-with-lease`.

### 5.4 Worktrees

A panel, visible by default, listing every worktree of the open repository (read from git, so worktrees created by Orca, agents or the terminal all appear):

- Path (shortened), branch or detached commit, active marker.
- Uncommitted-change count; ahead/behind versus upstream; "no upstream" when unpushed entirely.
- Locked and missing-directory (prunable) states.
- Pull request state for its branch, when known.

Actions per worktree:

- **Make active.**
- **Commit.** Opens a commit panel: list of changed files (status letter, path) with checkboxes, all ticked by default; message field; **Commit** and **Commit & Push**. Hooks run as normal; if a hook fails its output is shown.
- **Push.** Sets upstream on first push.
- **Create pull request** (see 5.5).
- **Open in editor**, **open in terminal**, **reveal in file manager.** Editor and terminal commands are settings.
- **Delete.** Before deleting, the app checks for uncommitted changes, untracked files, and commits not on any remote. If any exist it lists them in a warning and requires explicit confirmation before forcing. Offers to delete the branch too (off by default; same unmerged/unpushed warning).
- **Prune** stale entries.

Creating a worktree: from a branch or commit context menu, or the panel's add button. Asks for branch (existing or new) and location. Default location **(call)**: `../<repo-name>.worktrees/<branch-slug>`, with the pattern editable in settings.

When an action targets a branch checked out in another worktree (git refuses this), the app says which worktree holds it and offers to make that worktree active.

### 5.5 Pull requests

- Uses `gh`. On fetch/refresh the app loads open pull requests for the repository and shows a small marker on branches that have one.
- Create: a short form — title (prefilled from the branch's commits), description, base branch (default: the repository's default branch; remembered per repository), draft toggle. Pushes the branch first if needed. After creation, shows the link.
- If a pull request already exists for the branch, the action becomes "open pull request" in the browser.
- If `gh` is missing or not signed in, or the remote is not GitHub: the app says so once, and falls back to opening the host's compare page in the browser when it can build the URL. No other hosts are specifically supported in version one.

### 5.6 Conflicts and in-progress states

When a merge, rebase, cherry-pick or revert stops:

- A persistent banner on that worktree names the operation and lists conflicted files.
- Three choices: **Abort**, **Open in editor**, **Continue** (enabled once no conflicts remain; the app stages resolved files as part of continue after confirming).
- The app never edits conflicted files.
- States started outside the app are detected and shown the same way.

### 5.7 Safety and transparency

- Confirm before: force push, hard reset, deleting an unmerged branch, deleting a remote branch, dropping a stash, deleting a worktree with unsaved work.
- Git must never block waiting for terminal input: prompts are disabled and editors are suppressed (`--no-edit` or equivalent). If credentials are needed and no helper can supply them, the operation fails with a plain explanation.
- A command log panel **(call)** shows every git command the app ran, with output and result. Failures surface git's own message in a readable notice with a link to the log entry. This serves the owner's wish to see how and why things work.

### 5.8 Repositories, settings, look

- Open folder (dialog, drag-and-drop, or path as launch argument). Recents list with remove. Tabs. A welcome screen when nothing is open.
- Settings: theme (system / light / dark), fetch interval, editor command, terminal command, worktree location pattern, date format. Stored as a file in the platform's app-config directory.
- Light and dark themes, following the system by default. The visual direction is left to the builder with these constraints: information-dense but calm; monospace for hashes; readable branch colours in both themes; native-feeling window chrome on macOS; keyboard shortcuts for open, search, fetch, refresh, switch tab, and closing panels.

## 6. Architecture

```
┌────────────────────────── Tauri window (web view) ──────────────────────────┐
│  UI: tabs · toolbar · graph (canvas lanes + virtualized rows) ·             │
│      worktree panel · commit details · dialogs · command log                │
└───────────────▲──────────────────────────────────────────┬──────────────────┘
        events (repo changed, op progress)          commands (typed)
┌───────────────┴──────────────────────────────────────────▼──────────────────┐
│  Rust core                                                                   │
│   git runner ── spawns git/gh, captures output, no shell, no prompts         │
│   readers ───── log, refs, worktrees, status, stashes, PRs → typed data      │
│   graph layout  lane assignment, edges, colours                              │
│   op queue ──── one mutating operation per repository at a time              │
│   watcher ───── debounced file watching of each git dir                      │
│   scheduler ─── background fetch                                             │
│   store ─────── recents, session, settings, per-repo preferences             │
└──────────────────────────────────────────────────────────────────────────────┘
```

Boundaries:

- The UI never constructs git commands. It calls typed core commands ("merge branch X into active worktree with these options") and renders typed results.
- Parsing of git output lives only in the readers. Machine-readable formats only (`--porcelain`, `-z`, explicit `--format`).
- Graph layout is computed in Rust and delivered to the UI in a compact form, in chunks, so the first screen appears before the whole history is processed.
- The UI renders only visible rows; graph lines are drawn on a canvas aligned with the rows.
- Repository state is refreshed incrementally: a ref change does not reload commits that are already loaded unless history actually changed.

## 7. Speed targets

Measured on the owner's Mac against a repository of roughly 20,000 commits and 200 branches, plus a synthetic 100,000-commit repository. These are acceptance criteria, measured and reported at the end of each phase.

| What | Target |
|---|---|
| Launch to first screen of graph (last-opened repo) | under 1 second |
| Full 20k-commit history loaded and scrollable | under 2 seconds |
| Scrolling the graph | 60 fps, no blank rows |
| Feedback after a click (menu, selection, busy state) | under 100 ms |
| Refresh after an on-disk change | visible within 1 second, no scroll jump |
| Idle CPU, window open | effectively zero |
| Memory, one 20k-commit repo open | under 150 MB total across processes |
| App bundle size | under 20 MB |

If a target cannot be met, the builder reports the measured number and the cause instead of quietly accepting it.

## 8. Testing

- Rust: unit tests for every reader/parser and for graph layout; integration tests that build real temporary repositories (branches, merges, worktrees, conflicts, stashes) and run each operation against them, including the failure paths in 5.6 and 5.7.
- Worktree deletion warnings and the other confirmations in 5.7 each have a test proving the dangerous path cannot run unconfirmed.
- UI: the builder runs the real app and verifies each feature by using it, with screenshots, in both themes, before reporting a phase done.
- A script generates the large synthetic repository used for speed measurement.

## 9. Build phases

Each phase ends with something the owner can open and judge.

0. **Skeleton and speed proof.** App shell, open a repository, load and draw the graph for a large repo. Confirms the speed targets are reachable before features are built on top. If they are not, stop and report.
1. **Read-only graph.** Full graph view: refs, filters, search, commit details, recents, tabs, themes, auto-refresh, fetch.
2. **Actions.** Branch, commit, tag and stash context menus; dialogs; operation queue; command log; in-progress/conflict banner; the staging routine polish.
3. **Worktrees.** Panel, active worktree, create, commit, push, delete with warnings, open in editor/terminal.
4. **Pull requests and settings.** `gh` integration, settings screen, keyboard shortcuts, overall visual polish pass.
5. **Other platforms.** Windows and Linux builds verified; platform differences fixed.

## 10. Left to the builder

- Internal module layout, data formats between core and UI, graph layout algorithm.
- Visual design details within the constraints in 5.8.
- Exact wording of dialogs and warnings.
- Chunk sizes, debounce intervals, caching strategy — whatever meets section 7.
- Which keyboard shortcuts beyond the listed ones.

## 11. Additions (2026-11-15, from the owner)

- **Name.** The app is called **gg** (lowercase). The development name "GitGraph" is retired everywhere; it is also the name of the VS Code extension this app replaces, which a public release must not borrow.
- **`gg` in a terminal.** Running `gg` inside a git repository opens that repository in the app; `gg <path>` opens that path. If the app is already running, the repository opens as a tab in the existing window (one app instance). Outside a repository it says so and exits non-zero. The app offers to install the command; it never needs administrator rights.
- **Public and open source.** The repository is prepared to be made public (licence, README for strangers, contribution and security notes, no private data, CI). Making it public, pushing and tagging stay the owner's actions.
- **Landing page** at gg.shakirul.com (Cloudflare): features, download button, link to GitHub, "Built by Shakirul" linking to shakirul.com. Built on its own branch (`site`), outside this plan's tasks.
- **Downloads.** A universal macOS build as a `.dmg`, with checksum. Not notarized in version one (section 2 non-goal stands); the page and README say how to open it.
