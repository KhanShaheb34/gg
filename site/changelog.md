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
