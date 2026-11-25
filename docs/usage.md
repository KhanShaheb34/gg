# Using gg

A reference to what gg does and how it behaves, for when the [README](../README.md)'s
overview is not enough. On Windows and Linux, Ctrl stands for ⌘ throughout.

## The basics

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
  ("Conflict markers", "Check this file" for one with no markers to go by, "Resolved"; a file
  opens in your editor from there), Abort…, Open in editor and Continue…. Continue is enabled
  once every file is resolved; "Continue anyway…" goes ahead after listing what is not. The
  app never edits a conflicted file. When the commit being applied has no changes left (they
  are already there), the banner and the notice of the continue offer "Skip this commit"
  instead (rebase, cherry-pick, revert). Skip leaves out that commit only: if git has moved
  on to another, it is refused. Skipping a rebase's commit puts every tracked file back as
  HEAD has it, so with other uncommitted changes it asks first, listing them.
- Opening a folder of another worktree of a repository already open shows that
  repository's tab with that worktree made active (if an action is running there, a notice
  says the tab still works where it was).
- Git 2.29 or newer is needed. With an older one the app says so once, when it starts, and
  skips the background fetch; some actions fail until git is updated.
- git is looked up on your login shell's `PATH` (read when the app starts), falling back
  to the app's own `PATH`, so a git your shell finds is the one used even when the app is
  started from the Finder.

## Worktrees, committing and pull requests

- The side panel (⌘B, or the button at the right of the toolbar, which counts the files to
  commit) has two icon tabs, **Commit** (with a dot when there are files to commit) and
  **Worktrees**, and opens on the one last used in the repository (←/→ move between them).
  Worktrees lists the repository's worktrees as git does, including ones made in a terminal or
  by another tool: each with its branch, its uncommitted
  changes (kept up to date as files change), how it stands with its upstream, and what is
  unusual (locked, folder missing, a merge stopped there). In a window narrower than 840 px
  the panel lies over the graph. The **active** worktree, where every action
  runs, is marked there and named, with its branch, at the top of the window (the branch
  opens the branch picker; while the panel is hidden, the worktree count beside it shows the
  panel); double-click another (or "Make active")
  to switch. ⌘⌥1…⌘⌥9 switch to a worktree by its place in the list (Ctrl+Shift+1…9 on
  Windows and Linux).
- **Add worktree…** (the panel's + or menu, or a branch or commit menu) makes a folder for a
  new or existing branch, by default beside the repository in `<repo>.worktrees/<branch>`
  (set the pattern in Settings).
- **Delete…** removes a worktree. A clean one goes after one plain question (files git
  ignores, if any, are listed in it); one with uncommitted changes (edits to files marked
  `assume-unchanged` or `skip-worktree` included, which `git status` does not show), untracked
  files or commits on no branch needs a red confirmation listing them. One holding another
  worktree or repository inside it, at any depth, is refused, and so is one with submodules
  (git does not delete it; the notice says what a forced delete in a terminal would lose).
  Its branch is deleted only if you tick that. **Prune** forgets worktrees whose folder is
  gone, after a confirmation naming them; if you moved a worktree's folder, run
  `git worktree repair <new path>` instead.
- The **commit panel** (the Commit tab; ⌘K, or a worktree's change count) commits the files
  you tick in the active worktree. Opened from another worktree's change count or menu, it
  lists that worktree's files under a line naming it, with a button back to the active
  worktree's, and offers to make it active. **Commit & Push** sends the commit to the
  branch's upstream; when a merge
  is being concluded, its message is filled in. With nothing ticked, **Commit with no changes** makes an empty commit
  (`git commit --allow-empty`), with or without the push. The notice after a commit offers the push. Known limit: if the app quits or
  crashes between making a commit and updating the staging area (during a slow
  `post-commit` hook, say), the staging area still holds the committed files as they were
  before the commit, so `git status` shows staged changes that would undo it; `git reset`
  puts the staging area back as HEAD has it (your files are not touched).
- **Create pull request…** (a branch's or worktree's menu) needs `gh`, signed in: it suggests a
  title and description from the commits, pushes the branch first when it needs to, and
  says so on its button. Without `gh`, it pushes and opens GitHub's compare page. Open pull
  requests show as markers beside their branches and worktrees, loaded with each fetch (at
  most once a minute). The app never reads or stores a GitHub token. Note that `gh`, by its
  own design, sends a token it finds in your environment (`GH_TOKEN`, `GITHUB_TOKEN`) to
  github.com and to any `*.ghe.com` host (GitHub's own enterprise cloud): a repository whose
  remote names a `*.ghe.com` host gets gh's requests with it. gg removes the enterprise
  token variables (`GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`) for every host other
  than github.com, but leaves these two to gh.
- **Settings** (⌘,): theme (system, light or dark), date format, background fetch, the
  editor and terminal the worktree menu opens, the worktree location pattern, and the `gg`
  command. **?** lists the keyboard shortcuts.

## Switching branches

The branch picker (⌘P, Ctrl+P on Windows and Linux, or a click on the branch name at the top
of the window) works on the active tab's repository, in its active worktree.

- It lists the local branches, then the remote ones (whether or not the graph shows remote
  branches), each group newest commit first. Each row shows the branch, how long ago its
  last commit was made, and on a second line that commit's author and subject. The branch
  checked out is marked with a ring; one checked out in another worktree says "in worktree
  *folder*".
- Typing keeps the branches whose name contains the text, ignoring case, anywhere in it
  (`log` finds `feature/login` and `origin/backlog`); the matching part is marked. A branch
  named exactly as typed comes first. ↑/↓, Page Up/Down and Home/End move, Enter (or a
  click) chooses; typing puts you back on the first row.
- **A local branch** is checked out as its menu's **Check out** does: one behind its upstream
  asks whether to fast-forward it first. **The current branch** just closes the picker. **A
  branch checked out in another worktree** makes that worktree active instead (no git runs).
- **A remote branch** checks out the local branch of the same name if there is one (`origin/x`
  → `x`); otherwise it makes that local branch, tracking the remote one, and checks it out.
- **Create new branch ‘*name*’** makes the branch typed at HEAD and checks it out (one
  `git checkout -b`). It is the first row when no branch matches, so a new name and Enter
  are all it takes. A name that is a local branch's offers that branch instead. A name git
  would refuse says why under the row and in red under the field, and nothing runs. With the
  field empty, **Create new branch…** asks for the name in the same field.
- **Create new branch from…** lists HEAD and the branches to start from, filtered the same
  way; choosing one makes the branch there (at that commit; a remote branch chosen is not
  made its upstream) and checks it out, asking for the name first if none was typed.
- Esc, or Backspace in an empty field, goes back a step; Esc in the first step closes the
  picker, as does a click outside it.
- Uncommitted changes in the way, conflicts, a merge or rebase in progress, an action already
  running: the same notices and refusals as the same checkout from the graph.

## Actions

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
  **Push** *branch* **to** *remote/branch*; a push the remote rejects offers Fetch, Pull and
  a separate Force push…. So the staging routine is: check out `staging`, merge your branch
  into it, push — three clicks from the graph.
- **Push** goes to the branch of the same name on the remote, like git's own default
  (`push.default=simple`). A branch whose upstream has another name (made with
  `git switch -c feature origin/main`, say) is pushed there only if you pick it in the Push
  dialog, which names where the push goes; the merge notice then offers no one-click push.
  A branch without an upstream gets the branch pushed to as its upstream; an existing
  upstream is never changed.
- **Pull** on a branch that has diverged from its upstream asks whether to merge or rebase.
- Actions on one repository run one at a time, in the order asked; a second waits (the
  toolbar shows what runs and how many wait). A busy indicator shows when one takes more
  than 150 ms. An action on the current branch (reset, merge, rebase, cherry-pick, revert,
  pull, continue, abort, skip) runs only on the branch it was asked for: if another branch
  was checked out meanwhile (by an action queued before it, or in a terminal), it is refused
  with a plain notice and nothing changes.
- **Stop** appears beside the busy indicator once an action has run for 5 seconds (a hook
  that hangs, say): it ends the git command and everything it started, and the action fails
  as "Stopped.". git runs without a terminal and without input, so it never waits for a
  password or a prompt there.
- Because git runs without a terminal, anything that would ask on one fails at once instead,
  even when the app was started from a terminal: commit signing with a loopback pinentry
  (`pinentry-mode loopback`), an SSH signing key whose passphrase is not in an agent, and
  hooks that read `/dev/tty`. Use an agent (gpg-agent, ssh-agent, 1Password) or a GUI
  pinentry such as pinentry-mac; credential helpers and askpass programs keep working.
- Names (branches, tags) are checked as you type and again by the core: a name git would
  refuse, one that is taken, or one starting with `-` is refused before git runs.
- Network actions (push, pull, fetch into a branch, deleting on a remote) stop after 120 s
  without output or 2 hours in all; git never waits for a password prompt in a terminal.

## Safety confirmations

These run only after a red confirmation that says what would be lost; the core refuses them
otherwise, whatever the UI sends:

| Action | Why it is confirmed |
| --- | --- |
| Force push | It replaces the remote branch's commits; the confirmation lists the commits that would be lost there. It always uses `--force-with-lease` pinned to the remote commit you were shown: if the remote moved meanwhile, it asks again. |
| Reset hard | Uncommitted changes to tracked files are discarded and cannot be got back, and an untracked file where the target commit has a file is overwritten. The confirmation lists both; other untracked files are kept. |
| Delete a branch that is not merged | Its commits would be reachable from no branch (they stay in the reflog for a while). The confirmation says when they are safe in the current branch. |
| Delete a branch on a remote | Other people's clones lose it at their next fetch. The delete is pinned to the remote commit you were shown: if someone pushed to it since your last fetch, it is refused. |
| Delete a tag on a remote | Other people's clones lose it at their next fetch. |
| Drop a stash | Its changes are gone from the stash list. |
| Abort a merge, rebase, cherry-pick or revert | The changes made to resolve its conflicts are lost. |
| Skip a rebase's commit with other changes | git puts every tracked file back: those changes go too. |

Each confirmation is for what it showed: if the branch, the commit or the files it is about
changed before the action ran, it asks again with them as they are now (or, for another
branch checked out, refuses).

While git is stopped in a merge, rebase, cherry-pick or revert, actions that would start
another one, check out something else, apply a stash or save one are refused with a plain
reason; reset stays allowed (it is a way out). The same actions are refused (and their
menu items disabled, saying why) while files still have conflicts from an earlier step,
such as a stash that did not apply cleanly.

## Keyboard shortcuts

The same list is in the app: press **?** (⌘/ on macOS, F1 on Windows and Linux).

| Keys | Does |
| --- | --- |
| ⌘O | Open a repository |
| ⌘W | Close the tab (with none open, the window) |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous tab |
| ⌘1…⌘8, ⌘9 | A tab, the last tab |
| ⌘, | Settings |
| ?, ⌘/ (F1 on Windows and Linux) | Keyboard shortcuts |
| ⌘F | Search commits (Enter, Shift+Enter: next, previous match) |
| ↑/↓, Page Up/Down, Home/End | Select commits in the graph |
| Shift+F10, Menu key | The selected commit's menu (arrow keys, Enter, Esc in it) |
| Esc | Clear the selection, close the search, a dialog, a menu or a panel |
| ⌘⇧F | Fetch from all remotes |
| ⌘R | Refresh (and the pull requests) |
| ⌘P | The branch picker: switch branch, or create one |
| ⌘K | The side panel's Commit tab, its message focused |
| ⌘Enter | Commit what is ticked (in the commit message) |
| Space, ⌘A | Tick or untick a file, tick all (in the commit panel's list) |
| ⌘B | Show or hide the side panel, on the tab last used (Commit or Worktrees) |
| ⌘⇧L | Show or hide the command log |
| F6, Shift+F6 | Next region (graph, newest notice, side panel), the region before |
| ⌘⌥1…⌘⌥9 (Ctrl+Shift+1…9) | Make a worktree active, by its place in the side panel |

In the command log: ↑/↓, Home/End move between rows, Enter or Space opens one, C copies its
command line.

## The `gg` command

`gg` in a terminal opens the repository you are in; `gg <path>` the one that holds `path`
(a subfolder opens its repository, a linked worktree opens as itself, and `gg -- <path>`
takes a path that starts with `-`). It returns at once. If gg is already running, the
repository opens there (or its tab comes to the front) and the window comes forward; only
one copy of the app runs. `gg --help` and `gg --version` say what they say.

To install the command, choose **gg → Install ‘gg’ Command…** on macOS or **File → Install
‘gg’ Command…** on Windows and Linux (or "Install the command…" on the welcome screen, or
**Settings → Command line**). Pick one of the offered folders: they are already on your login shell's
`PATH`, writable by you and not inside a repository (`~/.local/bin`, `/opt/homebrew/bin` and
`/usr/local/bin` first). The app writes one small script there, named `gg`, and changes
nothing else: no administrator rights, and no edits to your shell files unless you ask for
the one line described below. It never replaces a
`gg` it did not write, and the same dialog removes the one it did. If no folder is offered,
add one such as `~/.local/bin` to your `PATH` in your shell's profile and open the dialog
again.

Installing gg with Homebrew (`brew install --cask khanshaheb34/tap/gg`) sets up the command
too: the app carries the launcher at `gg.app/Contents/Resources/gg`, and Homebrew links `gg`
in its `bin` folder (`/opt/homebrew/bin` on Apple silicon, `/usr/local/bin` on Intel) to it,
then removes the link when you uninstall gg. The app shows the command as installed by
Homebrew (on the welcome screen and in **Settings → Command line**) and leaves the link
alone: it offers neither to install over it nor to remove it. A link to the launcher made
some other way counts as installed too, and is left alone the same way.

If your shell already has a `gg` (an alias such as Oh My Zsh's `gg` for `git gui
citool`, a function, or another program earlier on `PATH`), the dialog says so: that one
wins until you remove it or rename it. Past an alias or a function, `command gg` reaches
this command anyway. The dialog learns this by asking your login shell (zsh, bash or a
POSIX sh); with fish it cannot tell, so no warning is shown there: run `type gg` in fish to
check.

When that `gg` is an alias or a function and your shell is zsh or bash, gg can turn it off
for you, but only if you ask: tick **Turn off that alias for me** in the dialog (it starts
unticked), or choose the same in **Settings → Command line**, which offers it also when
Homebrew installed the command. gg then adds one line to the end of your shell's start-up
file, where it runs after whatever made the alias:

```sh
unalias gg 2>/dev/null || :  # added by gg: lets the gg command run; gg Settings > Command line takes it out
```

(`unset -f gg 2>/dev/null || :` for a function.) `|| :` keeps a start-up file that sets
`set -e` going once the alias has gone. The file is `.zshrc` in `$ZDOTDIR` as your
`.zshenv` and `.zprofile` set it (gg asks a login zsh; a `ZDOTDIR` that `.zshrc` itself
sets for later does not count), else `~/.zshrc`, made if there is none; if zsh does not
say, nothing is offered. For bash it is the file a new terminal's bash reads: on macOS,
where terminals start login shells, the first of `~/.bash_profile`, `~/.bash_login` and
`~/.profile` that exists and can be read; on Linux, where they do not, `~/.bashrc`. gg never
makes a bash start-up file: with none, nothing is offered. Only a plain file is read or
written (a FIFO or device in its place is left alone). Nothing else in the file changes: the
line is appended (through a symbolic link, to the file it leads to, which keeps its
permissions, owner and ACLs), never added twice, and follows the file's line ends; a file
that did not end with a line break gets one before the line, and that line says so. If the
file changes while gg is adding the line, nothing is written. gg then asks your shell again:
if something that runs later (a `.zlogin`, say) still makes `gg`, gg says so rather than that
it worked. It takes effect in new terminal windows.

gg takes the line out when you remove the command in gg, or when you choose **Take the line
out** in Settings (the way back when Homebrew installed the command). Only that exact line
goes, with the line break gg added before it, if any: the file is shortened in place and is
left as it was before gg touched it (a `~/.zshrc` gg made is left empty, not deleted). If
you have added lines of your own after it since, gg does not rewrite the file: it leaves it
as it is and tells you which line to delete. If the file changes while gg is taking the line
out, it is left as it is. If you have changed the line, it is yours: gg leaves it alone, says
so, and adds no other. With fish or another shell, or
when the other `gg` is a program, nothing is offered.

Two `gg` commands run within the same instant while the app is not running yet can each
start a window of their own; once the app runs, every `gg` goes to it.

On macOS the command finds the app by its identifier, wherever it is. On Linux it starts the
app from where it was when the command was installed; after moving the app, install the
command again (the command says so when it cannot find the app). On Windows the command is
`gg.cmd`, written into a folder of your own that is already on your `PATH` (such as
`%LOCALAPPDATA%\Microsoft\WindowsApps`); it too starts the app from where it was installed.

## Updates

gg looks for a newer version shortly after its first rows are on screen (so the check never
slows its start), and then about once a day while it runs. It asks GitHub for a small file
describing the latest release (`latest.json`; GitHub answers through a redirect to its file
host). The request carries nothing about you, what is on your machine or your repositories
beyond what any web request does: your IP address, and a user agent naming the updater. When a
newer version is there it is downloaded in the background and its signature checked against
the key built into gg; then a notice says **gg X.Y.Z is ready.** with **Restart to update**.

- gg never restarts by itself. Dismiss the notice and it comes back the next time gg starts;
  **Settings → About** keeps saying it meanwhile, with its own **Restart to update**.
- Restart is refused, with the reason, while git is working in an open repository (an
  action, or a fetch you asked for): restart once it is done. Once you restart, gg starts no
  new action until it has restarted (or the install failed): "gg is restarting to update." After the restart the new
  version runs from the same place, with the repositories and tabs that were open.
- A check or download that fails says nothing in a notice: **Settings → About** says what
  happened (GitHub could not be reached, the download was not found, its signature did not
  match, …) and gg tries again an hour later. A download whose signature does not match gg's
  key is never installed.
- Only a newer version is ever offered: never the one running, an older one or a
  pre-release.
- **Settings → About → Check for updates automatically** turns the checks off (a download
  they started stops too); **Check now** looks at once, whatever the setting.

Where gg cannot replace itself, a notice says **gg X.Y.Z is available.**, why, and links the
download page (gg.shakirul.com):

- on macOS, when gg runs from its disk image, or from where it was downloaded (macOS then
  runs a temporary copy of it): drag gg to **Applications** and open it from there;
- when gg's folder (or the app itself) cannot be written to by you;
- on Linux, when gg was installed from the `.deb` package: update it with your package
  manager (only the AppImage replaces itself).

On macOS, an app installed with Homebrew updates itself like any other (it is in
**Applications**); if it cannot, the notice and **Settings → About** also say
`brew upgrade --cask gg` (when Homebrew set up the `gg` command, which is how gg knows). On Windows the
installer runs to put the new version in place (a small progress window) and starts gg again.

## Upgrading from a build made before the rename

The app was renamed to gg, which gives it a new identity on macOS. Your settings, recent
repositories and open tabs are copied over once, but column widths, panel sizes and each repository's
view filters start fresh, and macOS asks again before the app may read folders such as
Documents or Desktop.

