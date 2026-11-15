# Architecture

A map for contributors. gg is a Rust core that runs git, a thin desktop shell around it, and
a web UI that only renders what the core gives it.

```
┌──────────── UI (Svelte 5, src/) ─────────────┐
│ tabs · toolbar · graph · details · dialogs   │
│ menus · conflict banner · command log        │
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
└──────┬───────────────────────────────────────┘
       │ spawns git (no shell, no prompts)
       ▼
     git
```

## The parts

- **`crates/gg-core`**: everything that talks to git and shapes data for the UI. Its one
  entry point is `Core::dispatch` (`src/dispatch.rs`): a command name and JSON arguments in,
  JSON out. Each command's arguments are a struct that rejects unknown fields. Types that
  cross to the UI are in `src/dto.rs` and mirrored by hand in `src/lib/types.ts`.
- **`src-tauri`**: the desktop app. One Tauri command, `invoke_core(cmd, args)`, forwards to
  `Core::dispatch`; core events go out as Tauri events. Window, menus and platform glue live
  here, not logic.
- **`crates/gg-devserver`**: serves the same core over HTTP on 127.0.0.1 (`POST /invoke`,
  `GET /events` as server-sent events) so the UI runs in a plain browser, where Playwright
  can drive it. Development and tests only; the app never includes it.
- **`src/`**: the Svelte 5 UI. `src/lib/ipc.ts` is its only way to the core (Tauri IPC in the
  app, HTTP to the dev server in a browser).

## Inside the core

| Where | What |
|---|---|
| `git/runner.rs` | `GitRunner`, the only place the app spawns git. Every command gets the same non-interactive environment (`GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, editors replaced by `true`), runs without a shell or a terminal, and is recorded in the `CommandLog`. |
| `git/` (other files) | Readers: log, refs, worktrees, status, stashes, commit details, what git is stopped in. They parse machine-readable output only (`--porcelain`, `-z`, explicit `--format`). Name checks for branches and tags. |
| `graph/layout.rs` | Lane assignment, colours and line segments, computed as commits arrive. |
| `repo/` | `RepoManager` opens repositories, loads each log in the background in batches (`loader.rs`), serves rows and searches from memory, refreshes, and fetches on request and on a schedule (`fetch.rs`). `state.rs` is a repository's in-memory state: pure, no git, no I/O. |
| `ops/` | Operations the user asks for (`run_op`): local ones (checkout, branch, merge, rebase, cherry-pick, revert, reset, tags), network ones (push, pull, fetch into a branch, remote deletes), stashes, continue/abort/skip of what git stopped in. `danger.rs` decides what needs confirming; `explain.rs` turns known git failures into plain messages; `order.rs` keeps requests in the order they were sent. |
| `watcher.rs` | Watches each repository's git directory (not the working tree), debounced, so outside changes show without a manual refresh. |
| `store.rs` | Recents, open tabs, settings and per-repository preferences in `state.json` in the app's config folder. |
| `mask.rs` | Hides credentials in URLs (`https://user:token@host/…`) in everything the core hands the UI: results, errors, events and the command log. |
| `editor.rs` | Opens files or folders in the user's editor or the platform's opener, never through a shell. |

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
  details, search, the command log.
- `src/lib/graph/`: the virtualized graph. Rows are DOM, lanes are drawn on one canvas
  aligned with them (`lanesCanvas.ts`); `rowCache.ts` keeps a few pages of rows around the
  viewport.
- `src/lib/menus/`: what each right-click menu offers and why an item is disabled, as plain
  functions with unit tests; `dispatch.ts` turns a picked item into a request or a dialog.
- `src/lib/components/`: dialogs, banner, notices, command log, toolbar, tabs.

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
  [performance.md](performance.md). `e2e/screens.spec.ts` takes screenshots only with
  `GG_SCREENS=1`.
