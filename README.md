# gg

A small, fast desktop app for the git commit graph, your worktrees and pull requests, and the
everyday actions on them.

[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/graph-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/graph-light.png">
  <img alt="gg showing a repository's commit graph: coloured lanes on the left, branch, remote and tag labels beside each commit's subject, then date, author and short hash columns, the side panel on the right, on its Worktrees tab, the ↑1 push count in the toolbar, and the status bar along the bottom with the repository's folder" src="docs/screens/graph-light.png" width="1280">
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
checksum and then removes the quarantine flag that causes the message described below. It
also sets up the `gg` command (see [Open from the terminal](#open-from-the-terminal)).
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

### Updates

gg updates itself. Shortly after it starts, and about once a day while it runs, it looks for a
newer release; when there is one it downloads it in the background, checks its signature,
and says **gg X.Y.Z is ready** with a **Restart to update** button. It never restarts by
itself, and waits while git is working. Settings → About shows where this stands, has
**Check now**, and turns the automatic checks off.

Where gg cannot replace itself it says so, and links the download page instead: when it runs
from the disk image or from where it was downloaded (move it to Applications first), from a
folder you may not write to, or from a Linux `.deb` package (update that with your package
manager; the AppImage updates itself). An app installed with Homebrew updates itself like any
other; `brew upgrade --cask gg` works too, and gg names it when it cannot update itself.

What changed in each version is on the [changelog](https://gg.shakirul.com/changelog).

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

If gg is already running, the repository opens there.

If you installed gg with Homebrew, the command is already there: Homebrew links `gg` in its
`bin` folder to the launcher inside the app, and removes the link when you uninstall gg.
Otherwise install the command from the app: **gg → Install ‘gg’ Command…** on macOS,
**File → Install ‘gg’ Command…** on Windows and Linux, or **Settings → Command line**. It
writes one small script into a folder already on your `PATH` and needs no administrator
rights. It edits none of your shell files unless you ask it to, as below.

If your shell already has an alias called `gg` (Oh My Zsh's git plugin defines one, for
example), the alias wins until you remove it, with `unalias gg` at the end of your
`~/.zshrc` for instance; the install dialog and **Settings → Command line** warn you when
that is the case. With zsh or bash they can also do it for you, only if you ask (a box in
the dialog, off until you tick it, or a button in Settings): gg adds one line, marked as
added by gg, to the end of your `~/.zshrc` (in `$ZDOTDIR` if you set one) or bash's
start-up file, `unalias gg 2>/dev/null || :` (`unset -f gg 2>/dev/null || :` for a
function). It takes that line out again, and nothing else, when you remove the command in gg
or choose **Take the line out** in Settings. It takes effect in new terminal windows, unless
a start-up file read after it (a `.zlogin`, say) makes the alias again: gg asks your shell
afterwards and says which it is.

## Features

**The graph**

- Each repository in its own tab. Lanes and coloured lines for every branch, with local
  branch, remote branch, tag and stash labels on their commits; ahead and behind counts on
  a branch's label.
- Search subjects, authors and hashes; filter which branches are shown; show or hide remote
  branches, tags and stashes (View, in the toolbar).
- The toolbar holds actions; a quiet status bar along the window's foot holds the active
  worktree's folder (a click copies it or shows it in Finder) and when the repository was
  last fetched, by gg or from a terminal (View › Status bar hides it).
- Select a commit for its details: full hash, parents, author, date, message, and the files
  it changed with added and deleted line counts, as a tree or a list.
- The uncommitted changes of the active worktree as a row at the top.

**Worktrees**

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/worktrees-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/worktrees-light.png">
  <img alt="The side panel's Worktrees tab beside the graph: eight worktrees, each with its branch, its uncommitted changes and how it stands with its upstream, and markers for a merge in progress, a missing folder and a locked worktree" src="docs/screens/worktrees-light.png" width="640">
</picture>

- The side panel (⌘B, or its button at the right of the toolbar) has three icon tabs, Commit,
  Pull request and Worktrees, and opens on the one you used last. Worktrees lists every worktree of the
  repository, including ones made in a terminal or by other tools, each with its branch, its
  uncommitted changes (kept up to date as files change) and how it stands with its upstream,
  and says what is unusual: locked, folder missing, a merge stopped there.
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
  <img alt="The side panel's Commit tab: six changed files, each ticked, with its status letter, a message box with a subject and a body, and the Commit and Commit and Push to origin/main buttons" src="docs/screens/commit-panel-light.png" width="640">
</picture>

Press ⌘K (or open the side panel's Commit tab, or click a worktree's change count) to commit the files
you tick, with **Commit & Push** to send the branch upstream in the same step. Files you
staged but did not tick stay staged and out of the commit. With nothing ticked, **Commit with
no changes** makes an empty commit (to run CI again, say). When a merge is being concluded,
its message is filled in for you. Under the message, **Committed on branch** lists what the
branch's own commits change since it left its base (the pull request's base, else the
branch it was created from, as the pull request form offers it): each file with its folder,
lines added and removed, and its status.

**Generate with AI.** The sparkle beside the message (or ⌘G in it) writes a commit message
from the files you tick, and beside a pull request's title it writes the title and
description from the branch's commits and changes. It uses the AI command-line tool you
already have, signed in to your own account: [Claude Code](https://code.claude.com/docs/en/setup)
or [Codex](https://developers.openai.com/codex/cli), so it costs nothing beyond your own plan
with it. The text lands in the field for you to edit; Undo (or ⌘Z in the field) puts back
what was there, and nothing is committed or created by it. The tool, its model and the
prompts are in Settings → AI. What is sent, and to whom, is under
[privacy](#how-it-works-and-privacy).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/committed-on-branch-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/committed-on-branch-light.png">
  <img alt="The Commit tab on feature/report: two changed files ticked and a message, and under the Commit buttons, Committed on branch: 5 files against origin/main, each with its folder, lines added and removed and its status letter, one of them renamed from docs/old-guide.md" src="docs/screens/committed-on-branch-light.png" width="640">
</picture>

**Pull requests**

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/pr-details-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/pr-details-light.png">
  <img alt="The side panel's Pull request tab for fix/typo: pull request #1302, open, main ← fix/typo, with Open in browser and Copy link; Ready to merge and Approved; four passing checks; and four comments with an All, Humans and Bots filter: a person's comment, a bot's, a code comment on README.md:3 with its reply, and an approving review" src="docs/screens/pr-details-light.png" width="640">
</picture>

Through the GitHub CLI [`gh`](https://cli.github.com), signed in. The side panel's **Pull
request** tab (⌘⇧P) is about the current branch: it shows the branch's open pull request, or
the form to make one, with a title made from the branch's name (`sam/fix-login-error-wording`
gives "Fix login error wording"), a description listing its commits, the branch it goes into
(by default the one it was created from: a branch cut from `staging` goes into `staging`),
and Create as draft; it pushes the branch first when it needs to, and says so.
**Create pull request…** on a branch or worktree shows the tab for that branch. Under an open
pull request the tab shows where it stands (Ready to merge, Checks failing,
Conflicts with main, Approved…), its checks as they run (asked again every 15 s while one
runs and the window is in front), and its comments, review summaries and code comments, with
people's and bots' apart. Comments are drawn without their HTML and never load images or
anything else from the web; a link opens in your browser. You can reply in a code comment's
thread, resolve or unresolve it, and add a comment to the conversation, where GitHub lets
you: nothing is sent until you click Reply, Resolve or Comment (or press ⌘↩), and what you
write is kept while gg runs if it fails or you look elsewhere. After you push a branch that has no
pull request, the tab opens by itself (a setting). Open pull requests show as markers beside
their branches and worktrees. Without `gh`, gg pushes the branch and opens GitHub's compare
page instead. gg never reads or stores a GitHub token.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/pr-reply-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/pr-reply-light.png">
  <img alt="A reply being written in the code comment's thread on README.md:3 in the Pull request tab, under grace's question and ada's answer, with Resolve and Reply buttons below the box" src="docs/screens/pr-reply-light.png" width="640">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/pr-create-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/pr-create-light.png">
  <img alt="The Pull request tab for fix/typo, which has no pull request yet: the title and description suggested from its commit, into main of octo/cat, Create as draft, and the button Push and create PR, which pushes first to origin/fix/typo" src="docs/screens/pr-create-light.png" width="640">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screens/pull-requests-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/screens/pull-requests-light.png">
  <img alt="Pull request markers such as #1284 and #1295 beside branch labels in the graph and beside a worktree in the side panel's Worktrees tab" src="docs/screens/pull-requests-light.png" width="640">
</picture>

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
- The current branch's ↓behind and ↑ahead counts in the toolbar, just before Fetch: one click
  pulls or pushes it.
- Push sets the upstream on a first push. A push the remote rejects offers fetch, pull or a
  separate **force push**, which always uses `--force-with-lease` pinned to the remote commit
  you were shown.

**Switching branches**

Press ⌘P (or click the branch name at the top of the window) for the branch picker: type a
few letters of a branch, local or remote, and press Enter to check it out. Each branch shows
when its last commit was made, by whom, and its subject; the newest come first. A remote
branch becomes a local one that tracks it. A branch checked out in another worktree makes
that worktree active instead. Type a name no branch has and press Enter to create it at HEAD
and check it out, or choose **Create new branch from…** to start it at another branch.

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
- The graph and the side panel refresh by themselves when the repository changes on disk
  (a commit in a terminal, an agent working in a worktree), keeping your scroll position, and
  gg fetches from your remotes in the background, every 5 minutes by default.

**Settings and themes**

**Settings** (⌘,) holds the theme (follow the system, light or dark), the date format, the
background fetch interval (or turning it off), the editor and terminal gg opens, where new
worktrees go, whether a push opens the Pull request tab, and the `gg` command. The button at
the top right of the window cycles the theme.

**Keyboard**

On Windows and Linux, Ctrl stands for ⌘. Press **?** in the app for the full list.

| Keys | Does |
| --- | --- |
| ⌘O | Open a repository |
| ⌘F | Search commits (Enter / Shift+Enter: next / previous) |
| ⌘⇧F | Fetch from all remotes |
| ⌘R | Refresh (and the pull requests) |
| ⌘P | Switch branch, or create one (the branch picker) |
| ⌘K | Commit (the side panel's Commit tab, its message focused); ⌘Enter commits |
| ⌘⇧P | The side panel's Pull request tab, for the current branch |
| ⌘B | Show or hide the side panel, on the tab last used (Commit, Pull request or Worktrees) |
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
- No telemetry and no account. gg itself uses the network only to look for updates: shortly
  after it starts and about once a day it asks GitHub, over HTTPS, for a small file describing
  the latest release (`latest.json`; GitHub answers through a redirect to its file host), and
  when there is a new version it downloads it from the same release. These requests carry
  nothing about you, what is on your machine or your repositories beyond what any web request
  does: your IP address, and a user agent naming the updater. Turn the checks off in
  Settings → About. Otherwise the network is git talking to your own remotes, and `gh` when you
  use pull requests.
- **Generate with AI** sends something off your machine only when you click it, and gg does
  not send it: it runs the Claude Code or Codex you installed, which sends the prompt to its
  provider (Anthropic or OpenAI) under your account, as it does in your terminal. The prompt
  holds the branch's name, the list of files and their diff (for a commit, only the ticked
  files; for a pull request, the branch's commits and diff since its base, and the title and
  description as the form has them), cut to about 200 KB; binary files are named, never read.
  It goes on the tool's standard input, never on a command line or into the command log. The
  tool runs in a temporary folder, never in the repository, with no tools (Claude Code) or
  with its commands, image viewer, web search and your `config.toml` (its MCP servers) off and its sandbox read-only
  (Codex), so it cannot change the repository or read other files; it keeps no session. gg
  never reads, stores or passes on a token for it. A small notice says this before a tool's
  first use.
- Updates are signed. gg installs one only if its signature verifies with the public key
  built into the app and was made for the version offered; anything else is refused and
  nothing is installed. See [SECURITY.md](SECURITY.md#updates).
- Credentials in URLs (`https://user:token@host/…`) are shown as `***` in everything gg
  displays: notices, errors and the command log.

## Requirements

- macOS 12 or later. (Windows and Linux builds exist; see [Install](#windows-and-linux).)
- git 2.29 or later. With an older git, gg says so once and skips the background fetch.
- Optional: the GitHub CLI `gh`, signed in, for pull requests.
- Optional: Claude Code or Codex, signed in, for Generate with AI.

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

gg is at version 0.5.3. It is used every day on macOS. The Windows and Linux builds are built
and tested on CI on every change, but have not been used for real yet.

## Contributing

Bug reports, ideas and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
Security issues: see [SECURITY.md](SECURITY.md).

## Licence

[MIT](LICENSE). Parts come from other projects; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Credits

gg is inspired by the Git Graph extension for VS Code. Generate with AI's prompts come from
[Orca](https://github.com/stablyai/orca).

Built by [Shakirul](https://shakirul.com).
