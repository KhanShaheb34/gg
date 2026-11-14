# GitGraph

A fast, light desktop git client built around the commit graph: tabs of repositories, each a
virtualized graph with branch and tag labels, a branch filter, search, fetch and refresh, a
details panel with the commit's message and changed files, and the everyday actions on
commits, branches, tags and stashes from right-click menus. Tauri 2 shell, Rust core
(`crates/gg-core`), Svelte 5 UI (`src/`).

It writes into a repository only when you ask for an action (and for `git fetch`, which also
runs on a schedule you can turn off, updating remote-tracking refs, objects and `FETCH_HEAD`
as in a terminal). Every action runs git itself, with your hooks, one at a time per
repository, and shows git's own message when it fails.

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
  "Show log" on a failed operation's notice opens it there. Credentials in URLs
  (`https://user:token@host/…`) are shown as `***` here and in every message, notice and
  error the app shows from git.
- A conflict banner above the graph shows when git stops in a merge, rebase, cherry-pick or
  revert (started in the app or in a terminal): what is coming in, each conflicted file
  ("Conflict markers", "Git kept one side", "Resolved"; a file opens in your editor from
  there), Abort…, Open in editor and Continue…. Continue is enabled once every file is
  resolved; "Continue anyway…" goes ahead after listing what is not. The app never edits a
  conflicted file. A continue that comes out empty (the commit's changes are already there)
  offers "Skip this commit" on its notice (rebase, cherry-pick, revert).
- git is looked up on your login shell's `PATH` (read when the app starts), falling back
  to the app's own `PATH`, so a git your shell finds is the one used even when the app is
  started from the Finder.

### Actions

Right-click a commit, a branch or tag label, or a stash (or press the menu key or Shift+F10
on the selected row) for its menu. Items that cannot run say why when you point at them
(for example "HEAD is detached: there is no current branch").

| On | Items |
| --- | --- |
| A commit | Check out this commit · Create branch here… · Create tag here… · Cherry-pick… · Revert… · Reset *current* to here (Soft, Mixed, Hard) · Copy hash · Copy message |
| A local branch | Check out · Rename… · Delete… · Merge *it* into *current*… · Rebase *current* onto *it*… · Push… · Pull · Copy name |
| A remote branch | Check out as a local branch… · Fetch into a local branch… · Merge *it* into *current*… · Delete from *remote*… · Copy name |
| A tag | Push… · Delete… · Copy name |
| A stash | Apply · Pop · Create branch from stash… · Drop… |
| The uncommitted row | Stash changes… |

- **Check out** a branch that is behind its upstream asks whether to fast-forward it first.
  Double-clicking a local branch's label checks it out too.
- **Merge** offers "Create a merge commit even if fast-forward is possible" (remembered per
  repository), "Squash the commits" and "No commit". After a merge, the notice offers
  **Push** the merged branch; a push the remote rejects offers Fetch, Pull and a separate
  Force push…. So the staging routine is: check out `staging`, merge your branch into it,
  push — three clicks from the graph.
- **Pull** on a branch that has diverged from its upstream asks whether to merge or rebase.
- Actions on one repository run one at a time, in the order asked; a second waits (the
  toolbar shows what runs and how many wait). A busy indicator shows when one takes more
  than 150 ms.
- Names (branches, tags) are checked as you type and again by the core: a name git would
  refuse, one that is taken, or one starting with `-` is refused before git runs.
- Network actions (push, pull, fetch into a branch, deleting on a remote) stop after 120 s
  without output or 2 hours in all; git never waits for a password prompt in a terminal.

### Safety confirmations

These run only after a red confirmation that says what would be lost; the core refuses them
otherwise, whatever the UI sends:

| Action | Why it is confirmed |
| --- | --- |
| Force push | It replaces the remote branch's commits; the confirmation lists the commits that would be lost there. It always uses `--force-with-lease` pinned to the remote commit you were shown: if the remote moved meanwhile, it asks again. |
| Reset hard | Uncommitted changes to tracked files are discarded and cannot be got back. |
| Delete a branch that is not merged | Its commits would be reachable from no branch (they stay in the reflog for a while). The confirmation says when they are safe in the current branch. |
| Delete a branch or tag on a remote | Other people's clones lose it at their next fetch. |
| Drop a stash | Its changes are gone from the stash list. |

While git is stopped in a merge, rebase, cherry-pick or revert, actions that would start
another one, check out something else or apply a stash are refused with a plain reason;
reset stays allowed (it is a way out).

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
| Menu key, Shift+F10 | The selected row's menu (arrow keys, Enter, Esc in it) |
| ↑/↓, Page Up/Down, Home/End | Select commits in the graph |
| Esc | Close what has the focus: a dialog, a menu, the command log, the details panel; in the graph, clear the selection |

In the command log: ↑/↓, Home/End move between rows, Enter or Space opens one, C copies its
command line.

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

Targets and measured numbers are in [docs/perf/2026-11-plan-1.md](docs/perf/2026-11-plan-1.md)
and, for the actions, [docs/perf/2026-11-plan-2.md](docs/perf/2026-11-plan-2.md);
`scripts/perf.sh` measures the release app (macOS) on a repository, and
`scripts/gen-synthetic-repo.sh` makes the large test repositories it uses.
