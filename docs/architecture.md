# Architecture

A map for contributors. gg is a Rust core that runs git, a thin desktop shell around it, and
a web UI that only renders what the core gives it.

```
┌──────────── UI (Svelte 5, src/) ─────────────┐
│ tabs · toolbar · graph · details · dialogs   │
│ menus · conflict banner · command log        │
│ side panel: commit, pull request, worktrees  │
│ settings · keyboard shortcuts                │
└──────┬───────────────────────────▲───────────┘
       │ typed commands (JSON)     │ events: repo-changed, op-progress, …
┌──────▼───────────────────────────┴───────────┐
│ transport: Tauri shell (src-tauri/)          │
│        or gg-devserver (HTTP, dev and tests) │
└──────┬───────────────────────────▲───────────┘
       │ Core::dispatch(cmd, args) │ EventSink
┌──────▼───────────────────────────┴───────────┐
│ gg-core (crates/gg-core)                     │
│ readers · graph layout · repo state · ops    │
│ queue · dangers · watcher · fetch · store    │
│ worktrees · commit · pull requests · launch  │
│ updates (when, what is offered, the status)  │
└──────┬───────────────────────────────────────┘
       │ spawns git and gh, and on a click claude or codex (no shell, no prompts)
       ▼
   git, gh, claude / codex
```

## The parts

- **`crates/gg-core`**: everything that talks to git and shapes data for the UI. Its one
  entry point is `Core::dispatch` (`src/dispatch.rs`): a command name and JSON arguments in,
  JSON out. Each command's arguments are a struct that rejects unknown fields. Types that
  cross to the UI are in `src/dto.rs` and mirrored by hand in `src/lib/types.ts`.
- **`src-tauri`**: the desktop app. One Tauri command, `invoke_core(cmd, args)`, forwards to
  `Core::dispatch`; core events go out as Tauri events. Window, menus and platform glue live
  here, not logic. A second launch (`gg <path>` while the app runs) hands its folders to the
  running app through the single-instance plugin and exits. `src-tauri/resources/` holds the
  `gg` launcher scripts (`gg` for macOS and Linux, `gg.cmd` for Windows) that the app installs
  on request. The macOS app also carries the launcher, filled in, at
  `gg.app/Contents/Resources/gg`, for the Homebrew cask to link into its `bin`.
- **gg updating itself** is split the same way. `crates/gg-core/src/update.rs` decides when
  to check (once the UI has shown its first rows, then about once a day by the wall clock),
  offers only a strictly newer version, works out whether this copy can replace itself
  (translocated, on a disk image, a read-only folder, a Linux package, a development build),
  and keeps the status the UI shows (`update-changed` events; commands `update_status`,
  `start_update_checks`, `check_for_updates`, `restart_to_update`, the last refused while git
  works in an open repository). It does the work through an `UpdateBackend`:
  `src-tauri/src/update.rs` drives Tauri's updater plugin (check, download and verify,
  install, restart), whose one endpoint and public key are in `tauri.conf.json` and nowhere
  else; the web view is granted none of the plugin's commands. `gg-devserver
  --fake-updates <dir>` stands in for it in the UI's tests. A performance build can be given
  a local test source (`src-tauri/src/update_hook.rs`, compiled only with `--features perf`),
  which `scripts/update-proof.sh` uses to update a real app end to end.
- **`crates/gg-release`**: release tooling only: signs each update (`sign`), writes the
  updater's `latest.json`, and checks each update's signature against the app's key before
  a release is drafted.
- **`crates/gg-devserver`**: serves the same core over HTTP on 127.0.0.1 (`POST /invoke`,
  `GET /events` as server-sent events) so the UI runs in a plain browser, where Playwright
  can drive it. Development and tests only; the app never includes it.
- **`src/`**: the Svelte 5 UI. `src/lib/ipc.ts` is its only way to the core (Tauri IPC in the
  app, HTTP to the dev server in a browser).

## Inside the core

| Where | What |
|---|---|
| `git/runner.rs` | `GitRunner`, the only place the app spawns git. Every command gets the same non-interactive environment (`GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, editors replaced by `true`), runs without a shell or a terminal, and is recorded in the `CommandLog`. |
| `git/` (other files) | Readers: log, refs, worktrees (`worktrees.rs`), a worktree's changes (`status.rs`, `--porcelain=v2`), stashes, commit details, what git is stopped in. They parse machine-readable output only (`--porcelain`, `-z`, explicit `--format`). Name checks for branches and tags. |
| `graph/layout.rs` | Lane assignment, colours and line segments, computed as commits arrive. |
| `repo/` | `RepoManager` opens repositories, loads each log in the background in batches (`loader.rs`), serves rows and searches from memory, refreshes, switches the active worktree, and fetches on request and on a schedule (`fetch.rs`). `prs.rs` keeps each repository's open pull requests, loaded through `gh` alongside a fetch; `pr_details.rs` reads one pull request's merge status, checks and comments when the Pull request tab shows it (`pr_details`: at most once every 10 s per pull request, a request during a load waiting for its answer). `base.rs` is a branch's base, one rule for the pull request form's suggestion and Committed on branch: the base remembered for the branch, the branch it was created from (its reflog, else HEAD's at that moment), the nearest remote branch by history (within a time limit), the default (the remote's HEAD, else gh's default branch as gh last said it, else `main` or `master` on the remote; for the default branch itself, only the base remembered for it comes before); worked out when asked and kept per branch for what it depends on. `branch_files.rs` lists what a worktree's branch changes in its own commits for the Commit tab (`branch_files`: the base chosen from the snapshot, `merge-base` then `diff-tree -M --raw --numstat`, read as a commit's files are; one answer kept per worktree for its HEAD, base and base commit, a request during a read waiting for its answer). `state.rs` is a repository's in-memory state: pure, no git, no I/O. |
| `ops/` | Operations the user asks for (`run_op`): local ones (checkout, branch, merge, rebase, cherry-pick, revert, reset, tags), network ones (push, pull, fetch into a branch, remote deletes), stashes, continue/abort/skip of what git stopped in, worktrees (`worktree.rs`: add, remove, prune), commits of the ticked files (`commit.rs`, through a temporary index so the user's staging is left alone), pull requests (`pr.rs`: opening one, and the Pull request tab's writes, `pr_reply`, `pr_resolve` and `pr_comment`, through `gh` on the repository `pr_details` reads, the user's text on `gh`'s stdin, never in an argument; each done, the pull request's details are forgotten so the tab's next load asks `gh`) and a stale `index.lock` (`lock.rs`). `danger.rs` decides what needs confirming; `explain.rs` turns known git failures into plain messages; `order.rs` keeps requests in the order they were sent. |
| `gh.rs`, `hosts.rs` | Runs the GitHub CLI for pull requests (the app never reads or stores a token), and works out from a remote's URL which host, owner and repository it names and whether that host is GitHub. |
| `ai/` | Generate with AI (`ai_tools`, `ai_generate`, `ai_cancel`): a commit message or a pull request's title and description, written by the AI command-line tool the user installed (Claude Code, Codex), run as `gh` is. `tools.rs` is one adapter per tool (finding it, its version, its command line, where its answer is, what its failures mean; adding a tool is one more adapter); `prompt.rs` the prompts (Orca's, see THIRD_PARTY_NOTICES.md), their variables, the diff's budget and reading the answer; `context.rs` what is read with git for a prompt (only reads); `run.rs` finding the tools, running one in a temporary folder with the prompt on stdin, a time limit, Cancel, and telling its failures apart. Each generation is an operation in the command log, without its prompt. The settings hold the tool, each one's model and the prompts. The UI's side: `AiGenerate.svelte`, `AiProblem.svelte`, `state/ai.svelte.ts`. The e2e and `gg-devserver` tests run `gg-fake-ai` (a program of the devserver crate) in place of the tools. |
| `pr_details.rs` | Reads `gh pr view --json` and the review threads' `gh api graphql` answer leniently into a pull request's details: a state or shape it does not know reads as unknown; each thread keeps its node id and what the user may do to it (`viewerCanReply`, `viewerCanResolve`, `viewerCanUnresolve`; false when not said), and the details whether a comment can be added (not archived; locked, only with write access); comment bodies are kept as written (the UI draws them from Markdown tokens with its own elements, `src/lib/markdown.ts`, never as HTML; an image is never loaded, only its alt text drawn, or nothing when it repeats the text beside it, or the link it is all of). |
| `watcher.rs` | Watches each repository's git directory, and the active worktree's files, debounced, so outside changes show without a manual refresh. |
| `store.rs` | Recents, open tabs, settings and per-repository preferences in `state.json` in the app's config folder. On first start after the rename to gg it copies the state over from the old config folder once. |
| `mask.rs` | Hides credentials in URLs (`https://user:token@host/…`) in everything the core hands the UI: results, errors, events and the command log. |
| `launch.rs` | Starts the user's editor, a terminal, the file manager or the browser as argument arrays, never through a shell, and never hands a repository's file to the system's default application. The browser gets only `https://` addresses: a pull request's page on the repository's GitHub host, or (a link in a comment, a check's page) any host, at most 2048 characters. |
| `cli.rs` | Installs and removes the `gg` terminal command in a folder already on the user's `PATH`, and finds what else answers to `gg` there. |
| `shell_line.rs` | The one marked line gg adds, only when asked, to the end of a zsh or bash start-up file to turn off a `gg` alias or function, and takes out again: plain files only, appended without rewriting the file, through links, every byte and permission kept; taken out by shortening the file, never by writing it again. |
| `platform.rs` | What differs between macOS, Linux and Windows: finding programs on a `PATH`, ending a child's whole process tree, process liveness and file identity. See [platforms.md](platforms.md). |

## How a request flows

Take "Merge `feature` into `main`" from a branch label's menu:

1. The menu (`src/lib/menus/`) builds a typed `OpRequest` and the UI calls `run_op` through
   `ipc.ts`. The UI never builds a git command line; it names an operation and its
   arguments.
2. The transport hands it to `Core::dispatch`, which parses the arguments and calls
   `RepoManager::run_op`.
3. The request is checked without git (names, what is supported), then waits for its turn:
   one operation at a time per repository, in the order asked.
4. In its turn, the core works out the dangers on the repository as it is now. Each danger
   carries a token of the state it was computed from. If any danger is not confirmed with
   the same token, nothing runs and the outcome is `needs_confirmation`; the UI shows the
   red confirmation and sends the request again with the tokens.
5. The operation runs git children through `GitRunner` in the active worktree. Network
   commands have time limits and report progress as `op-progress` events; a running
   operation can be stopped.
6. It ends with one refresh: the readers re-read refs, status and what git is stopped in;
   the new state becomes a new *generation*, and a snapshot of it is emitted as
   `repo-changed`. The UI swaps in the snapshot and asks for the rows it shows with
   `get_rows` for that generation, keeping the scroll position.

Reads follow the same path without the queue: `get_snapshot`, `get_rows`,
`commit_details`, `search_commits`, `command_log`.

## The UI

- `src/lib/state/`: per-app and per-repository state (`*.svelte.ts`), operations in flight,
  details, search, the command log, worktrees, pull requests, settings and the `gg` command.
  The Pull request tab's drafts (replies, comments) are the side panel's state
  (`prDrafts.svelte.ts`, held by `PanelState`, in memory only); `prCompose.svelte.ts` sends
  them as operations and loads the details again once done.
- `src/lib/graph/`: the virtualized graph. Rows are DOM, lanes are drawn on one canvas
  aligned with them (`lanesCanvas.ts`); `rowCache.ts` keeps a few pages of rows around the
  viewport.
- `src/lib/menus/`: what each right-click menu offers and why an item is disabled, as plain
  functions with unit tests; `dispatch.ts` turns a picked item into a request or a dialog.
- `src/lib/components/`: dialogs, banner, notices, command log, toolbar, tabs, the status bar
  (`StatusBar.svelte`: the active worktree's folder, and the last fetch from the snapshot's
  `fetch.fetchedAtMs`, which the core reads from `FETCH_HEAD`), the side panel
  (`SidePanel.svelte` with `WorktreePanel.svelte` and `CommitPanel.svelte`, which holds
  `CommittedFiles.svelte`, the branch's committed files; the Pull request tab's
  `PullRequestView.svelte` with its comments, `PrCommentCard.svelte`, and the boxes to reply
  and comment, `PrComposer.svelte`), settings and the
  keyboard shortcuts sheet.
- `src/lib/shortcuts.ts`: the keyboard shortcuts, one table for macOS (⌘) and for Windows and
  Linux (Ctrl).

## Tests

| Suite | Where | Runs with |
|---|---|---|
| Rust unit tests | next to the code in each crate | `cargo test --workspace` |
| Core integration tests | `crates/gg-core/tests/` | `cargo test --workspace` |
| Dev server tests | `crates/gg-devserver/tests/` | `cargo test --workspace` |
| UI unit tests (Vitest) | `src/**/*.test.ts` | `pnpm test` |
| End-to-end (Playwright, Chromium) | `e2e/*.spec.ts` | `pnpm e2e` |

- Integration tests build real throwaway repositories (`gg_core::testutil`, behind the
  `testutil` feature) with an empty global git config, so the host's hooks, signing and
  templates cannot affect them. Every dangerous path has a test showing it cannot run
  unconfirmed.
- End-to-end tests run the UI in Chromium against the Vite dev server and a fresh
  `gg-devserver` per test, on repositories made by `e2e/fixtures.ts`.
- `e2e/perf/` runs only with `GG_PERF=1` and measures rather than checks; see
  [performance.md](performance.md). `e2e/walk.spec.ts` runs the end-to-end walk only with
  `GG_WALK=1`. `e2e/screens.spec.ts` takes screenshots of every view, menu and dialog in both
  themes, only with `GG_SCREENS=1`, into `.screens/` (git-ignored); the README's pictures in
  `docs/screens/` are picked from them.
- `scripts/update-proof.sh` (macOS, by hand) updates a real app end to end: the perf
  build at an old version, a local update source on 127.0.0.1 serving a newer one signed
  with a throwaway key, Restart to update clicked in the app's own notice, then the cases
  where nothing may be installed (tampered, another key, signed for another version, the
  same or an older version).
- CI (`.github/workflows/ci.yml`) runs formatting, clippy, every suite above and a bundle
  build on macOS, Linux and Windows (the end-to-end tests are skipped on Windows; see
  [platforms.md](platforms.md)).

## Releases

`.github/workflows/release.yml` runs on a version tag that matches `Cargo.toml`'s version and
a commit CI passed on. It builds the macOS universal disk image (`scripts/release-macos.sh`,
which puts the `gg` launcher in the app with `scripts/fill-launcher.sh`, signed ad hoc, not
notarized), the Linux AppImage and `.deb` and the Windows installer, each
with its SHA-256, and puts them on a draft GitHub release that is published by hand.

The release also carries what the app updates itself from. Each build job makes its
platform's update, unsigned: the macOS `gg-macos-universal.app.tar.gz` (made by
`scripts/update-archive.sh` from the final signed app, launcher included), the AppImage, the
Windows installer. The signer job builds `gg-release` (`cargo build --locked -p gg-release`)
without any secret. The sign job checks nothing out and builds nothing: it fetches the signer
and the updates, and its one step that sees the secrets `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` runs only `gg-release sign` by its absolute path, with a
clean `PATH`; it writes signatures in the format `tauri signer sign --app-version` writes, and
fails without the secrets. A separate job, with no secrets, verifies every signature against
the public key in `src-tauri/tauri.conf.json` (`gg-release verify`, the verifier the app uses),
writes `latest.json` (the plugin's static manifest: `darwin-aarch64` and `darwin-x86_64` both
name the universal archive, then `linux-x86_64` and `windows-x86_64`, each URL one of the
tag's own assets) and checks it (`gg-release check-manifest`) against the list of assets the
draft release uploads. Apps check `releases/latest/download/latest.json`, so a release
reaches them once its draft is published. The first version that updates itself (0.4.0)
is installed by hand.
