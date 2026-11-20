# gg

A small, fast desktop app for the git commit graph, your worktrees, and the everyday actions
on them.

[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/graph-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/graph-light.png">
  <img alt="gg showing a repository's commit graph: coloured lanes on the left, branch, remote and tag labels beside each commit's subject, then date, author and short hash columns, and the worktree panel on the right" src="docs/screens/graph-light.png" width="1280">
</picture>

## Why

If you open VS Code or Cursor just to look at the commit graph, merge a branch and push it,
gg does that job on its own, without the editor. It is a small desktop app around the git you
already have: it opens in under a second on a 20,000-commit repository, uses no CPU while it sits
there, and is light enough to leave open beside your terminal all day. It also keeps every
worktree of a repository in view, with what has changed in each, which helps when several
branches (or several tools) are at work at once.

## Install

### macOS

1. Download gg from **https://gg.shakirul.com** (one disk image for Apple silicon and Intel).
2. Open the download and drag **gg** to **Applications**.

Or with [Homebrew](https://brew.sh):

```sh
brew install --cask khanshaheb34/tap/gg
```

Installed this way gg opens straight away: the cask checks the download against its
checksum and then removes the quarantine flag that causes the message described below.
(Newer versions of Homebrew ask you to trust a third-party tap first. If the install is
refused for that reason, run `brew trust khanshaheb34/tap` and install again.)

Requires macOS 12 or later.

gg is not notarized by Apple: that needs a paid Apple developer account, which this project
does not have. So the first time you open a copy you downloaded yourself, macOS will say it cannot check the app. Open it
either way:

- Open **System Settings → Privacy & Security**, scroll down to the message about gg and
  click **Open Anyway**. (On macOS 14 and earlier you can instead right-click, or
  Control-click, **gg** in Applications, choose **Open**, then **Open** again.)
- Or, in a terminal, remove the quarantine flag the download was given:

  ```sh
  xattr -cr /Applications/gg.app
  ```

After the first time it opens normally.

### Windows and Linux

Each release also has a Windows installer and a Linux AppImage and `.deb`, on the
[releases page](https://github.com/KhanShaheb34/gg/releases). Be aware that these builds are
built and tested on CI only: gg is used day to day on macOS, and nobody has yet used it for
real on Windows or Linux. Reports of what works and what does not are very welcome.
[docs/platforms.md](docs/platforms.md) says what was verified where, and the known gaps.

## Open from the terminal

```sh
gg            # opens the repository you are in
gg path/to/repo
```

Install the command from the app: **gg → Install ‘gg’ Command…** on macOS, **File → Install
‘gg’ Command…** on Windows and Linux, or **Settings → Command line**. It writes one small
script into a folder already on your `PATH`, needs no administrator rights and edits none of
your shell files. If gg is already running, the repository opens there.

If your shell already has an alias called `gg` (Oh My Zsh's git plugin defines one, for
example), the alias wins until you remove it, with `unalias gg` at the end of your
`~/.zshrc` for instance; the install dialog warns you when that is the case.

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

**Worktrees**

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/worktrees-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/worktrees-light.png">
  <img alt="The worktree panel beside the graph: eight worktrees, each with its branch, its uncommitted changes and how it stands with its upstream, and markers for a merge in progress, a missing folder and a locked worktree" src="docs/screens/worktrees-light.png" width="640">
</picture>

- The side panel (⌘B) lists every worktree of the repository, including ones made in a
  terminal or by other tools, each with its branch, its uncommitted changes (kept up to date
  as files change) and how it stands with its upstream, and says what is unusual: locked,
  folder missing, a merge stopped there.
- Make any worktree the **active** one, where every action runs, with a double-click or
  ⌘⌥1…9.
- **Add worktree…** makes a folder for a new or existing branch, beside the repository by
  default (`<repo>.worktrees/<branch>`; the pattern is a setting).
- **Delete…** removes one: a clean worktree after a plain question, one with uncommitted or
  untracked work, or commits on no branch, only after a red confirmation that lists them.
  **Prune** forgets worktrees whose folder is gone.
- Open a worktree in your editor, a terminal or the file manager from its menu.

**The commit panel**

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/commit-panel-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/commit-panel-light.png">
  <img alt="The commit panel: six changed files, each ticked, with its status letter, a message box with a subject and a body, and the Commit and Commit and Push to origin/main buttons" src="docs/screens/commit-panel-light.png" width="640">
</picture>

Press ⌘K (or click a worktree's change count) to commit the files you tick, with
**Commit & Push** to send the branch upstream in the same step. Files you staged but did not
tick stay staged and out of the commit. When a merge is being concluded, its message is filled
in for you.

**Pull requests**

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/pull-requests-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/pull-requests-light.png">
  <img alt="Pull request markers such as #1284 and #1295 beside branch labels in the graph and beside a worktree in the side panel" src="docs/screens/pull-requests-light.png" width="640">
</picture>

Through the GitHub CLI [`gh`](https://cli.github.com), signed in: **Create pull request…** on
a branch or worktree suggests a title and description from its commits and pushes the branch
first when it needs to. Open pull requests show as markers beside their branches and
worktrees. Without `gh`, gg pushes the branch and opens GitHub's compare page instead. gg
never reads or stores a GitHub token.

**Right-click actions**

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/commit-menu-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/commit-menu-light.png">
  <img alt="The right-click menu of a commit: check out, create branch or tag, cherry-pick, revert, and a reset submenu with soft, mixed and a red hard reset" src="docs/screens/commit-menu-light.png" width="640">
</picture>

- Commits: check out, create a branch or tag here, cherry-pick, revert, reset the current
  branch here (soft, mixed, hard), copy the hash or message.
- Branches: check out, rename, delete, merge into the current branch, rebase the current
  branch onto it, push, pull, add a worktree, create a pull request; remote branches: check
  out as a local branch, fetch into a local branch, merge, delete from the remote.
- Tags: push, delete. Stashes: save, apply, pop, drop, create a branch from one.
- Push sets the upstream on a first push. A push the remote rejects offers fetch, pull or a
  separate **force push**, which always uses `--force-with-lease` pinned to the remote commit
  you were shown.

**Checkout, merge, push**

The routine of updating a shared branch takes three clicks from the graph: check out
`staging` (gg offers to fast-forward it if it is behind), merge your branch into it, then
press **Push** on the notice the merge leaves.

**Conflicts**

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/conflict-banner-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/conflict-banner-light.png">
  <img alt="The conflict banner above the graph: merging feature/x into main, one file resolved and one with conflict markers, and Abort, Open in editor, Continue anyway and Continue buttons" src="docs/screens/conflict-banner-light.png" width="640">
</picture>

When git stops in a merge, rebase, cherry-pick or revert, started in gg or in a terminal, a
banner names it and lists each conflicted file, with **Open in editor**, **Abort…** and
**Continue…** (once every file is resolved), and **Skip this commit** when the commit being
applied has nothing left to apply.

**Seeing what happened**

- The command log (⌘⇧L) shows every git command gg ran for the repository, grouped by
  operation, with its output, exit code and duration. A failed operation's notice links to
  its entry.
- The graph and the worktree panel refresh by themselves when the repository changes on disk
  (a commit in a terminal, an agent working in a worktree), keeping your scroll position, and
  gg fetches from your remotes in the background, every 5 minutes by default.

**Settings and themes**

**Settings** (⌘,) holds the theme (follow the system, light or dark), the date format, the
background fetch interval (or turning it off), the editor and terminal gg opens, where new
worktrees go, and the `gg` command. The button at the top right of the window cycles the
theme.

**Keyboard**

On Windows and Linux, Ctrl stands for ⌘. Press **?** in the app for the full list.

| Keys | Does |
| --- | --- |
| ⌘O | Open a repository |
| ⌘F | Search commits (Enter / Shift+Enter: next / previous) |
| ⌘⇧F | Fetch from all remotes |
| ⌘R | Refresh |
| ⌘K | Commit (the commit panel, its message focused); ⌘Enter commits |
| ⌘B | Show or hide the side panel (worktrees) |
| ⌘⌥1…⌘⌥9 (Ctrl+Shift+1…9) | Make a worktree active |
| ⌘⇧L | Show or hide the command log |
| ⌘1…⌘9, Ctrl+Tab | Switch tabs |
| ⌘W | Close the tab |
| ⌘, | Settings |
| ↑ ↓, Page Up/Down, Home/End | Move through commits |
| Shift+F10 or the menu key | The selected row's menu |

[docs/usage.md](docs/usage.md) describes every action, confirmation and shortcut in detail.

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
- **gg deletes a folder only through `git worktree remove`**, for a folder git lists as a
  worktree of that repository, and refuses one that holds another repository or submodules.

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

- macOS 12 or later. (Windows and Linux builds exist; see [Install](#windows-and-linux).)
- git 2.29 or later. With an older git, gg says so once and skips the background fetch.
- Optional: the GitHub CLI `gh`, signed in, for pull requests.

## Build from source

Prerequisites:

- [rustup](https://rustup.rs). The Rust release gg builds with is pinned in
  `rust-toolchain.toml` (1.98.1); rustup installs it by itself the first time you run
  `cargo` in the repository.
- [Node.js](https://nodejs.org) 24 and [pnpm](https://pnpm.io) 10 (the exact pnpm version
  is in `package.json`; `corepack enable` provides it).
- macOS: Xcode's command line tools (`xcode-select --install`).
- Linux: Tauri's system libraries. On Debian or Ubuntu:

  ```sh
  sudo apt-get install build-essential pkg-config file libssl-dev libxdo-dev libdbus-1-dev \
    libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf
  ```

- Windows: the Microsoft C++ build tools and WebView2 (see Tauri's
  [prerequisites](https://v2.tauri.app/start/prerequisites/)).

Then:

```sh
pnpm install                     # JavaScript dependencies
pnpm tauri dev                   # run the app with hot reload
pnpm tauri build --bundles app   # macOS: build gg.app into target/release/bundle/macos/
pnpm tauri build                 # this platform's bundles, into target/release/bundle/
```

Tests and checks (the same ones CI runs on macOS, Linux and Windows):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace           # Rust unit and integration tests
pnpm check                       # type-check (svelte-check, tsc)
pnpm test                        # UI unit tests (Vitest)
pnpm e2e                         # end-to-end tests (Playwright, Chromium; macOS and Linux)
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

Measured for 0.3.0 on an Apple M1 Pro laptop with a 20,000-commit, 200-branch repository:

- about 0.8 s from launch to the first rows of the graph on a busy machine, about 0.66 s on
  a quiet one;
- 0 % CPU while idle;
- about 155 MB of memory idle, across the app and its WebKit processes, and about 285 MB
  after scrolling the whole history. The 150 MB target is missed; the reasons are in the
  performance notes;
- an app of about 8 MB.

Method, all numbers, the misses, and how to reproduce them:
[docs/performance.md](docs/performance.md).

## Status

gg is at version 0.3.0. It is used every day on macOS. The Windows and Linux builds are built
and tested on CI on every change, but have not been used for real yet.

## Contributing

Bug reports, ideas and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
Security issues: see [SECURITY.md](SECURITY.md).

## Licence

[MIT](LICENSE).

## Credits

gg is inspired by the Git Graph extension for VS Code.

Built by [Shakirul](https://shakirul.com).
