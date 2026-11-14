# GitGraph

A fast, light desktop viewer for git history: tabs of repositories, each a virtualized
commit graph with branch and tag labels, a branch filter, search, fetch and refresh, and a
details panel with the commit's message and changed files. Tauri 2 shell, Rust core
(`crates/gg-core`), Svelte 5 UI (`src/`). It never changes your work: no commits, checkouts
or merges. The only git command it runs that writes is `git fetch` (when you ask, and on a
schedule you can turn off), which updates remote-tracking refs, objects and `FETCH_HEAD`, as
in a terminal.

## Using it

- Open a repository folder with the + button, ⌘O, by dropping the folder on the window, or
  by launching the app with the folder as its argument. Open tabs come back at the next start.
- ⌘1…⌘8 go to a tab, ⌘9 to the last one, Ctrl+Tab / Ctrl+Shift+Tab to the next or previous
  one; ⌘W closes the tab (with no tab open, the window). On a focused tab, ←/→ move and
  Delete closes it.
- In the graph: ↑/↓, Page Up/Down, Home/End select commits; Esc clears the selection.
  Clicking a short hash copies the full one.
- ⌘F searches subjects, authors and hashes (Enter / Shift+Enter: next / previous hit),
  ⌘⇧F fetches from all remotes, ⌘R re-reads the repository. On Windows and Linux, Ctrl
  stands for ⌘.
- The command log (⌘⇧L, or the terminal button in the toolbar) shows the git commands the app
  ran for the repository: each operation under its title, with each command's line, time,
  duration, exit code and output; "Background reads" adds the reads the app makes by itself.
  "Show log" on a failed operation's notice opens it there. Credentials in remote URLs are
  shown as `***`.
- git is looked up on your login shell's `PATH` (read when the app starts), falling back
  to the app's own `PATH`, so a git your shell finds is the one used even when the app is
  started from the Finder.

### Keyboard shortcuts

On Windows and Linux, Ctrl stands for ⌘.

| Keys | Does |
| --- | --- |
| ⌘O | Open a repository |
| ⌘W | Close the tab (with none open, the window) |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous tab |
| ⌘1…⌘8, ⌘9 | A tab, the last tab |
| ⌘F | Search commits |
| ⌘⇧F | Fetch from all remotes |
| ⌘R | Refresh |
| F6 | The newest notice (its first button) |
| ⌘⇧L | Show or hide the command log |
| Esc | Close what has the focus: a dialog, a menu, the command log, the details panel |

## Dev commands

```sh
pnpm install                 # install JS dependencies
pnpm dev                     # vite dev server on http://127.0.0.1:1420
pnpm test                    # unit tests (vitest)
pnpm e2e                     # browser tests (Playwright, chromium)
pnpm check                   # type-check (svelte-check)
pnpm tauri dev               # run the desktop app
pnpm tauri build --debug     # build a debug app bundle
cargo test --workspace       # Rust tests
```

The browser build talks to `gg-devserver` (dev/test only) on http://127.0.0.1:1421:

```sh
cargo run -p gg-devserver -- --config-dir /tmp/gg-config [--open <repo>] [--allow-origin <origin>]
curl -s -XPOST 127.0.0.1:1421/invoke -H 'content-type: application/json' \
  -d '{"cmd":"get_app_state","args":{}}'
```

It answers only loopback `Host` names, and browsers only from http://127.0.0.1:1420,
http://localhost:1420 and each `--allow-origin`; `/invoke` takes only `application/json`.

## Speed

Targets and measured numbers are in [docs/perf/2026-11-plan-1.md](docs/perf/2026-11-plan-1.md);
`scripts/perf.sh` measures the release app (macOS) on a repository, and
`scripts/gen-synthetic-repo.sh` makes the large test repositories it uses.
