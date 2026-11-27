<!--
The changelog page (gg.shakirul.com/changelog) is made from this file. To add a release:

1. Above the newest entry, add "## X.Y.Z (YYYY-MM-DD)": the version without a "v", and the
   day the GitHub release was published (UTC, as `gh release view vX.Y.Z --json publishedAt`
   gives it).
2. Under it, paste the release notes' opening sentence and their "What changed" section. Leave
   out what every release repeats: the downloads table, opening gg the first time on macOS, and
   the Windows and Linux note. The page links each version to its GitHub release for those.
3. Run `node site/scripts/changelog.mjs`, look at the page, and commit this file together
   with public/changelog.html and public/sitemap.xml.

What this file may hold: paragraphs, "### " headings, "- " lists (a line indented under an
item continues it), **bold**, `code` and [links](/github/...). Point GitHub links at /github/
as the rest of the site does. Anything else stops the script with the line it is on.
`sh scripts/deploy.sh` refuses to publish when the page is out of date with this file, or when
the newest entry here is not the version in public/latest.json.
-->

## 0.5.3 (2026-11-27)

Commit messages and pull request text written by the AI tool you already use, shorter toasts, and a fix for graph lines that could go missing.

### What changed

- **Generate with AI.** The sparkle beside the commit message (or ⌘G in it) writes a commit message from the files you ticked. Beside a pull request's title it writes the title and description from the branch. gg runs the Claude Code or Codex you already installed and signed in to, so there is no key to enter and nothing to pay beyond your own plan. Nothing leaves your machine until you click it; the tool runs with its tools switched off and keeps no session. Cancel stops it, ⌘Z puts back what you had, and **Settings › AI** picks the tool and the model. Codex has so far been tested signed out only; reports are welcome.
- **Shorter toasts.** Every message is now a short headline with a note under it. Branches and commits show as chips, raw links are gone (Open and Copy link remain), and a failure keeps its **Show log**.
- **A pull request's default title comes from the branch's name.** `sam/fix-login-error-wording` becomes "Fix login error wording". A branch with one commit used to get that commit's subject; it now gets the branch-name title too.
- **Fixed: the graph's lines could go missing**, leaving only the dots, until gg was restarted. This happened when the system restarted the process that draws for gg.

## 0.5.2 (2026-11-26)

A status bar at the bottom of the window, and a quieter header.

### What changed

- **A status bar.** A slim bar at the bottom of the window shows the folder you are working in on the left and when the repository was last fetched on the right ("Fetched 2 min ago", with the exact time in its tooltip). Click the folder for **Copy path** and **Reveal in Finder** (Show in File Explorer, Open containing folder). The fetch time also notices a fetch or pull you run in a terminal. Hide the bar with **View › Status bar**.
- **A shorter header.** The repository's name is no longer repeated under its tab, and the path has moved to the status bar, so the header starts with the branch and holds only things you act on.

## 0.5.1 (2026-11-26)

Answer a pull request's review from the side panel, and pull or push the current branch in one click.

### What changed

- **Reply, resolve and comment in the Pull request tab.** Each review thread has a **Reply…** box and a **Resolve** button; a resolved thread has **Unresolve**; and a box at the end of the comments adds a comment to the conversation. Nothing is sent until you press the button or ⌘↩ (Ctrl+Enter). If sending fails, your text stays in the box with the reason under it. Drafts are kept while you switch tabs or commits, until gg quits. What you are not allowed to do on a pull request is not offered. It all goes through your `gh`; gg still never reads or stores a GitHub token.
- **Pull and push from the toolbar.** When the current branch is behind its upstream, a `↓9` count appears in the toolbar just before Fetch; one click pulls. Ahead, it shows `↑2` and one click pushes. Diverged, it shows both, each its own button, and pulling asks whether to merge or rebase as before. In sync, nothing shows. The counts update when a fetch lands.
- **The upstream is read from git, not guessed.** gg now takes each branch's upstream remote and branch from git's configuration, so a remote with a slash in its name can no longer be mistaken for another.

## 0.5.0 (2026-11-26)

Pull requests move into the side panel: create one, then follow its checks and comments, without leaving gg.

### What changed

- **One side panel, three tabs.** The toolbar's Commit and Worktrees buttons are now one side panel button, ⌘B (Ctrl+B), with the count of your uncommitted files. Inside the panel, icon tabs switch between **Commit**, **Pull request** and **Worktrees**; the arrow keys move between them, and the panel opens on the tab you used last in each repository. The Commit tab has a dot when there is something to commit. ⌘K still opens the Commit tab with its message ready.
- **Create a pull request in the panel.** The **Pull request** tab (⌘⇧P, Ctrl+Shift+P) is about the current branch. With no pull request yet, it holds the form that used to be a dialog: a title and description suggested from the branch's commits, the branch it goes into, **Create as draft**, and a button that pushes the branch first when it needs to. What you type is kept per branch while gg runs. **Create pull request…** on a branch or a worktree opens the tab for that branch.
- **It goes into the branch you started from.** The form offers, by default, the branch yours was created from, as git's own records say: a branch cut from `staging` goes into `staging`. You can pick any other.
- **The tab opens after a push.** When you push a branch that has no pull request, other than the default branch, the tab shows by itself, without taking the focus. Turn this off in **Settings › Pull requests**.
- **Checks and comments.** Under an open pull request the tab shows where it stands (Ready to merge, Checks running, Checks failing, Conflicts, Approved, Changes requested…), its checks, failing first, each with how long it took, asked again every 15 seconds while one runs and the window is in front, and its comments: the conversation, review summaries and comments on the code with their replies, with an **All / Humans / Bots** filter. The tab's icon has a dot for the checks: red, amber or green. It is all read through `gh pr view` and one read-only `gh api graphql` query; gg still never reads or stores a GitHub token.
- **Comments are drawn safely.** A comment's Markdown is drawn without its HTML, and nothing in it is loaded from the web. An image shows as its alt text; a link opens in your browser only when it is an `https://` address.
- **Committed on branch.** Under the commit message, the Commit tab lists every file your branch's commits change since it left its base: name, folder, lines added and removed, and status. The base is the pull request's, or else the branch yours was created from, the same one the form offers. The list is open when you have nothing to commit, and folded away when you do.
- **Fixed: git failing before it started, on macOS.** Now and then a git that gg started failed at once with “git exited with code -1” and no message. A race inside macOS when a process starts caused it; gg now prepares for that once, before it starts any git.
- **Smaller things.** While the pull requests first load, the tab says “Looking for a pull request…” instead of offering to make one. The side panel has one button to hide it, the one in the toolbar.

## 0.4.2 (2026-11-24)

A tidier toolbar: one View menu, and a Commit button beside Worktrees.

### What changed

- **A Commit button in the toolbar.** It opens the commit panel of the active worktree, as ⌘K (Ctrl+K) does, and shows a small count of your uncommitted files.
- **Commit and Worktrees are a pair.** The side panel has two views, and each button is highlighted for the one that shows. Pressing the other switches the panel; pressing the highlighted one hides it. The Worktrees button has a new icon, two folders, in place of the side-panel one.
- **One View menu.** The three checkboxes for remote branches, tags and stashes are now one **View** button beside the branch filter. It is coloured while anything is hidden, and its tooltip says what.
- **The branch filter's search field is fixed.** Its search icon sat above the text; they are side by side again.
- **Smaller things.** The worktree count in the header always opens the worktree list. The list no longer takes the keyboard focus each time it reappears.

## 0.4.1 (2026-11-24)

A branch picker: press ⌘P, type, press Enter.

### What changed

- **Switch branches from the keyboard.** ⌘P (Ctrl+P on Windows and Linux), or a click on the branch name in the header, opens a search field at the top of the window. Typing filters your local branches, then the remote ones, newest commit first. Each row shows the branch, when its last commit was, its author and its subject. Enter or a click checks the branch out.
- **Create a branch the same way.** When no branch has the name you typed, **Create new branch ‘name’** is the first row: Enter creates it where you are and checks it out. **Create new branch from…** lets you pick HEAD or any branch to start from. A name git would refuse says why, in place.
- **Remote branches need no dialog.** Choosing `origin/foo` checks out your local `foo`, or creates it tracking the remote if you have none.
- **Worktrees are respected.** A branch that is checked out in another worktree makes that worktree the active one, since git will not check it out twice.
- **It is fast.** Opening the picker and typing run no git at all, and only the rows on screen are drawn: with 15,000 branches a keystroke takes under a millisecond. Creating a branch is now one git command, not two.
- **The header's branch name opens the picker.** It used to move to the worktree list; the worktree count beside it is now a button of its own for that.

If you run 0.4.0, gg offers this update by itself: this is the first release delivered that way.

## 0.4.0 (2026-11-21)

From this release on, gg updates itself. This is the last version you need to download or `brew upgrade` by hand.

### What changed

- **gg updates itself.** Shortly after it starts, and about once a day while it runs, gg looks for a new version, downloads it in the background and says **gg X.Y.Z is ready** with a **Restart to update** button. It never restarts by itself, and your tabs come back after the restart. **Settings → About** shows where updating stands, has **Check now**, and a switch to turn the automatic checks off.
- **Updates are signed.** gg installs an update only if its signature verifies with the public key built into the app, and only if it is newer than the version running. A file that was changed, signed with another key, or signed for another version is refused and nothing is installed. See [SECURITY.md](/github/blob/main/SECURITY.md#updates).
- **What the check sends.** The check asks GitHub for a small file describing the latest release. It carries nothing beyond what any web request does: your IP address and a user agent naming the updater. There is still no telemetry and no account.
- **gg can turn off a shell alias called `gg`, if you ask.** When your shell already has an alias or function named `gg` (Oh My Zsh's git plugin defines one), the install dialog now has a checkbox, off by default: **Turn off that alias for me**. Ticked, gg adds one marked line to the end of your zsh or bash start-up file, then asks your shell again and tells you whether it worked. **Settings → Command line** offers the same, also when Homebrew installed the command, and takes the line out again. gg edits no shell file unless you ask.
- **Empty commits.** With nothing ticked in the commit panel, **Commit with no changes** makes an empty commit, for example to run CI again. Thanks to the contributor who asked for it in [#10](/github/issues/10).

### Where gg cannot update itself

- Installed from the Linux `.deb`: update it with your package manager. The AppImage updates itself.
- Running from a folder you may not write to, or straight from the disk image: gg says a new version is available, why it cannot install it, and links the download page.
- Installed with Homebrew, gg updates itself like any other copy; `brew upgrade --cask gg` works too.

### Not yet tried for real

Updating was tested end to end on macOS against a local test server: download, signature check, replacing the app, restarting with the tabs back. Not yet tried: an update of a copy in `/Applications` on a real Mac (macOS may ask for permission), and self-update on Windows and Linux. The first real update will be from 0.4.0 to the next version. If it goes wrong for you, please open an [issue](/github/issues); downloading the new version by hand always works.

## 0.3.1 (2026-11-20)

A small release that makes installing with Homebrew complete.

### What changed

- **The `gg` terminal command now comes with a Homebrew install.** The macOS app carries the command's launcher inside it, and the Homebrew cask links it into Homebrew's `bin`. After `brew install --cask khanshaheb34/tap/gg`, `gg` in a terminal opens the repository you are in.
- **gg recognises a command it did not install itself.** When the `gg` on your `PATH` is Homebrew's link, Settings → Command line says “Installed by Homebrew” and where, and gg neither replaces nor removes it. Any other link to gg's launcher is shown as a link and left alone too.

Nothing else changes. If you installed the command from the app (**gg → Install ‘gg’ Command…**), it keeps working as before.

If your shell has an alias called `gg` (Oh My Zsh's git plugin defines one), the alias still wins until you remove it, for example with `unalias gg` at the end of your `~/.zshrc`.

## 0.3.0 (2026-11-19)

The first public release of gg, a small, fast desktop Git client.

### What it does

- **Commit graph** of the whole repository, each repository in its own tab, with search and branch filters.
- **Right-click actions** on commits, branches, tags and stashes: check out, create a branch or tag, merge, rebase, cherry-pick, revert, reset, push, pull, and a force push that always uses `--force-with-lease`.
- **Worktrees as first-class citizens**: a side panel lists every worktree of the repository with its branch and uncommitted changes, and adds, switches between, deletes and prunes them.
- **Commit panel**: tick the files to commit, write the message, and commit or commit and push in one step.
- **Pull requests** on GitHub through the `gh` CLI: create one from a branch or worktree, and see open ones as markers beside their branches. gg never reads or stores a GitHub token.
- **Conflicts**: when a merge, rebase, cherry-pick or revert stops, a banner lists the conflicted files and offers to continue, skip or abort.
- **Light and dark themes**, a log of every git command gg ran, and a `gg` command to open a repository from the terminal.

More in the [README](/github#readme) and the [usage guide](/github/blob/main/docs/usage.md).
