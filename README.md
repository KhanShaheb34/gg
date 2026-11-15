# gg

A small, fast desktop app for the git commit graph, and the everyday actions on it.

[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/graph-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/graph-light.png">
  <img alt="gg showing a repository's commit graph: coloured lanes on the left, branch, remote and tag labels beside each commit's subject, then date, author and short hash columns" src="docs/screens/graph-light.png" width="1280">
</picture>

## Why

If you open VS Code or Cursor just to look at the commit graph, merge a branch and push it,
gg does that job on its own, without the editor. It is a small desktop app around the git you
already have: it opens in under a second on a 20,000-commit repository, uses no CPU while it sits
there, and is light enough to leave open beside your terminal all day.

## Install

1. Download gg from **https://gg.shakirul.com**.
2. Open the download and drag **gg** to **Applications**.

Requires macOS 12 or later.

gg is not notarized by Apple: that needs a paid Apple developer account, which this project
does not have. So the first time you open it, macOS will say it cannot check the app. Open it
either way:

- Right-click (or Control-click) **gg** in Applications and choose **Open**, then **Open**
  again. If macOS does not offer **Open**, open **System Settings → Privacy & Security**,
  scroll down to the message about gg and click **Open Anyway**.
- Or, in a terminal, remove the quarantine flag the download was given:

  ```sh
  xattr -cr /Applications/gg.app
  ```

After the first time it opens normally.

## Open from the terminal

```sh
gg            # opens the repository you are in
gg path/to/repo
```

Install the command from the app's menu with **Install 'gg' Command…**.

If your shell already has an alias called `gg` (oh-my-zsh's git plugin defines one, for
example), the alias wins until you remove it with `unalias gg`, for instance at the end of
your `~/.zshrc`.

## Features

**The graph**

- Each repository in its own tab. Lanes and coloured lines for every branch, with local
  branch, remote branch, tag and stash labels on their commits; ahead and behind counts on
  a branch's label.
- Search subjects, authors and hashes; filter which branches are shown; show or hide remote
  branches, tags and stashes.
- Select a commit for its details: full hash, parents, author, date, message, and the files
  it changed with added and deleted line counts, as a tree or a list.
- The uncommitted changes of the active worktree as a row at the top.

**Right-click actions**

<img alt="The right-click menu of a commit: check out, create branch or tag, cherry-pick, revert, and a reset submenu with soft, mixed and a red hard reset" src="docs/screens/commit-menu-dark.png" width="640">

- Commits: check out, create a branch or tag here, cherry-pick, revert, reset the current
  branch here (soft, mixed, hard), copy the hash or message.
- Branches: check out, rename, delete, merge into the current branch, rebase the current
  branch onto it, push, pull; remote branches: check out as a local branch, fetch into a
  local branch, merge, delete from the remote.
- Tags: push, delete. Stashes: save, apply, pop, drop, create a branch from one.
- Push sets the upstream on a first push. A push the remote rejects offers fetch, pull or a
  separate **force push**, which always uses `--force-with-lease` pinned to the remote commit
  you were shown.

**Checkout, merge, push**

The routine of updating a shared branch takes three clicks from the graph: check out
`staging` (gg offers to fast-forward it if it is behind), merge your branch into it, then
press **Push** on the notice the merge leaves.

**Worktrees**

A worktree panel lists every worktree of the repository, including ones made in a terminal
or by other tools. Make one the active worktree, create a new one, commit the files you tick,
push, and delete a worktree, with a warning first when that would lose work. Stale entries
can be pruned.

**Pull requests**

Pull requests for your branches, through the GitHub CLI [`gh`](https://cli.github.com).

**Conflicts**

<img alt="The conflict banner above the graph: merging feature/x into main, two files with conflict markers, and Abort, Open in editor, Continue anyway and Continue buttons" src="docs/screens/conflict-banner-dark.png" width="640">

When git stops in a merge, rebase, cherry-pick or revert, started in gg or in a terminal, a
banner names it and lists each conflicted file, with **Open in editor**, **Abort…** and
**Continue…** (once every file is resolved), and **Skip this commit** when the commit being
applied has nothing left to apply.

**Seeing what happened**

- The command log (⌘⇧L) shows every git command gg ran for the repository, grouped by
  operation, with its output, exit code and duration. A failed operation's notice links to
  its entry.
- Light and dark themes, following the system by default.
- The graph refreshes by itself when the repository changes on disk (a commit in a terminal,
  an agent working in a worktree), keeping your scroll position, and gg fetches from your
  remotes in the background, every 5 minutes by default.
- **Settings** holds the theme, the background fetch interval (or turning it off) and the
  editor that conflicted files open in.

**Keyboard**

| Keys | Does |
| --- | --- |
| ⌘O | Open a repository |
| ⌘F | Search commits (Enter / Shift+Enter: next / previous) |
| ⌘⇧F | Fetch from all remotes |
| ⌘R | Refresh |
| ⌘⇧L | Show or hide the command log |
| ⌘1…⌘9, Ctrl+Tab | Switch tabs |
| ⌘W | Close the tab |
| ↑ ↓, Page Up/Down, Home/End | Move through commits |
| Shift+F10 or the menu key | The selected row's menu |

## Safety

- **Every destructive action says what will be lost, and is confirmed against the state you
  were shown.** Force push lists the commits the remote would lose; a hard reset lists the
  changes it would discard; deleting an unmerged branch, a branch or tag on a remote, a stash
  or a worktree with work in it, and aborting a merge or rebase, each say what goes. The
  check is made by the core, not the UI. If the branch, commit or files changed between the
  confirmation and the action, gg asks again with things as they are now.
- **gg never edits a conflicted file.** You resolve conflicts in your editor; Continue stages
  the files you resolved, after asking.
- **gg writes to a repository only when you ask.** The exception is the background fetch,
  which updates remote-tracking branches as `git fetch` does and can be turned off. Actions
  on one repository run one at a time, in the order asked, and an action on the current
  branch is refused if another branch was checked out meanwhile.

## How it works, and privacy

- gg runs the git you have installed, found on your login shell's `PATH`. Your SSH keys,
  credential helpers, config and hooks behave as they do in a terminal, and every action
  shows git's own message when it fails.
- git runs without a terminal, so nothing can wait on a prompt there. Anything that would
  ask on a terminal fails at once instead: commit signing with a loopback pinentry, an SSH
  key whose passphrase is not in an agent, hooks that read `/dev/tty`. Use an agent
  (ssh-agent, gpg-agent, 1Password) or a graphical pinentry such as pinentry-mac.
- No telemetry and no account. The only network use is git talking to your own remotes,
  and `gh` when you use pull requests.
- Credentials in URLs (`https://user:token@host/…`) are shown as `***` in everything gg
  displays: notices, errors and the command log.

## Requirements

- macOS 12 or later.
- git 2.29 or later. With an older git, gg says so once and skips the background fetch.
- Optional: the GitHub CLI `gh`, signed in, for pull requests.

## Build from source

Prerequisites: [Rust](https://rustup.rs) (stable), [Node.js](https://nodejs.org) and
[pnpm](https://pnpm.io), and Xcode's command line tools on macOS.

```sh
pnpm install                     # JavaScript dependencies
pnpm tauri dev                   # run the app with hot reload
pnpm tauri build --bundles app   # build the .app into target/release/bundle/macos/
```

Tests and checks:

```sh
cargo test --workspace           # Rust unit and integration tests
pnpm test                        # UI unit tests (Vitest)
pnpm e2e                         # end-to-end tests (Playwright, Chromium)
pnpm check                       # type-check (svelte-check, tsc)
```

The end-to-end tests run the UI in a browser against `gg-devserver`, a small HTTP server for
the core that exists only for development and tests:

```sh
pnpm dev                         # the UI on http://127.0.0.1:1420
cargo run -p gg-devserver -- --config-dir /tmp/gg-config --open path/to/repo
```

It listens on 127.0.0.1:1421 and answers only loopback hosts and the dev UI's origin.

See [docs/architecture.md](docs/architecture.md) for how the code is laid out.

## Performance

Measured on an Apple M1 Pro laptop with a 20,000-commit, 200-branch repository:

- about 0.75 s from launch to the first rows of the graph;
- 0 % CPU while idle;
- about 160 MB of memory idle, across the app and its WebKit processes; about 250 MB after
  scrolling the whole history (the 150 MB target is missed; the reasons are in the
  performance notes);
- an app of about 7 MB.

Method, all numbers and how to reproduce them: [docs/performance.md](docs/performance.md).

## Status and platforms

gg runs on macOS today. Windows and Linux builds are planned.

## Contributing

Bug reports, ideas and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
Security issues: see [SECURITY.md](SECURITY.md).

## Licence

[MIT](LICENSE).

## Credits

gg is inspired by the Git Graph extension for VS Code.

Built by [Shakirul](https://shakirul.com).
