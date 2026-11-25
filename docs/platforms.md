# gg on macOS, Linux and Windows

gg is built and used on macOS. The code is written for Linux and Windows too, and every
platform-specific path has a counterpart (or says plainly that something is not available),
but until the CI workflow (`.github/workflows/ci.yml`) has run on all three, Linux and Windows
are **compiled, not run**. This page says what was verified where, as of November 2026.

- **Built and tested** — built and its tests (Rust, UI, end-to-end) run on a Mac.
- **Compiled** — `cargo check` and `cargo clippy -D warnings` pass for that target, cross-compiled
  on a Mac (`x86_64-pc-windows-msvc`, `x86_64-pc-windows-gnu`, `x86_64-unknown-linux-gnu`);
  nothing ran.
- **Unit-tested logic** — the platform's decisions are pure functions taking the platform as an
  argument, and their tests run on every platform (so on the Mac too).
- **CI** — tests written for that platform that only CI runs; unverified until the first push.

## What was verified where

| Area | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Core: reading repositories, the graph, operations (`gg-core`) | Built and tested | Compiled; CI | Compiled; CI |
| Children: no terminal, stdin null, the whole process tree ended by Stop, a time limit or the app's exit | Built and tested (session + process group, `setsid`/`killpg`; libnotify set up before the first fork, so no child dies in libSystem's fork handler) | Compiled (same Unix code); CI | Compiled: a job object per child (`CREATE_SUSPENDED`, assigned, resumed; kill on close), `CREATE_NO_WINDOW`; CI test `the_whole_tree_ends_with_the_job` |
| Finding git, gh, ssh, the editor and terminal on `PATH` (absolute folders only, never the current one) | Built and tested | Compiled; CI | Unit-tested logic (`PATHEXT`, quoted entries, `.exe`/`.com` only, a `.cmd` shim said not run); compiled |
| `PATH` for children | The login shell's (`$SHELL -ilc`) | The login shell's | The app's own (inherited); no login shell |
| The `gg` command: the launcher | Built and tested (`tests/launcher.rs`) | Same script; CI | `gg.cmd`: static checks and fill-in tests on the Mac; run through cmd only in CI (`tests/launcher_windows.rs`) |
| The `gg` command: install / uninstall / what else is `gg` | Built and tested | Compiled; CI | Unit-tested logic (folders of the user's own on `PATH`, `where`-style lookup); compiled |
| Working-tree watch | Built and tested (one recursive FSEvents watch) | Folder-by-folder mode built and tested on the Mac (FSEvents, non-recursive); inotify itself only in CI | Compiled (one recursive watch); CI |
| A stale `index.lock`: does a git process on this computer have the worktree (or its git folder) as its working folder? | Built and tested (libproc) | Compiled (`/proc`); CI | Not available: gg says it could not check, and still asks |
| File identity (conflict judging), process liveness (commit temp folders) | Built and tested | Compiled; CI | Compiled (`GetFileInformationByHandle`, `OpenProcess`); CI unit tests |
| The app (`src-tauri`) | Built (overlay title bar, macOS menu bar) | Not compiled here: the GTK/WebKit/D-Bus system libraries cannot be cross-compiled from a Mac; CI | Compiled (`cargo check` and clippy, with `llvm-rc` for the resource file); CI build |
| Window chrome and menus | Overlay title bar, traffic lights; app menu with Settings, Install ‘gg’ Command, Keyboard Shortcuts | Native title bar (`tauri.linux.conf.json`); File/Help window menu with the same items; CI build only | Native title bar (`tauri.windows.conf.json`); File/Help window menu; CI build only |
| Keyboard shortcuts | ⌘ shortcuts; built and tested | Ctrl shortcuts, F1 for the shortcuts sheet: unit-tested (the table is tested for both) | as Linux |
| Bundles | `.app` (and the dmg of `scripts/release-macos.sh`) | `.deb`, AppImage: CI build only | NSIS installer: CI build only |
| Updating itself | Built and tested end to end with a local update source (`scripts/update-proof.sh`: check, download, signature, replace in place, restart, tabs restored; refused when tampered, signed with another key or for another version; nothing offered at the same or an older version). Not yet with a real release, nor from `/Applications` | AppImage: the plugin replaces the file and gg restarts; the updater logic and the release's signature are tested, the replacement itself only by the plugin's authors. A `.deb` install does not update itself (says so) | NSIS installer, run by the plugin in passive mode, which restarts gg; compiled; not exercised |
| Icons | `icon.icns` from the padded macOS artwork | `icons/flat/*` (no macOS margin): rendered, not seen in a desktop | `icons/flat/icon.ico`: rendered, not seen in Explorer |
| UI checks and unit tests (`pnpm check`, `pnpm test`) | Run | CI | CI |
| End-to-end tests (`pnpm e2e`, Chromium against the dev server) | Run | CI | **Skipped** (see below) |

## Known gaps

### Windows

- **Stop ends git at once.** Windows has no request to end for a program without a window (Unix
  sends SIGTERM first, so git removes its lock files). A stopped git may leave `index.lock`
  behind; gg names it and offers to remove it once it is ten minutes old.
- **gg cannot tell whether another git is working in a repository** (a `git commit` waiting in a
  terminal's editor holds `index.lock`). Before removing a stale lock it says it could not
  check, and the confirmation is red as everywhere. (On macOS and Linux the check covers git
  processes whose working folder is the worktree or its git folder; a git started with
  `--git-dir` from elsewhere, another user's or a container's git, and other programs that
  lock the staging area are not seen, and the confirmation says so.)
- **Only programs are run.** git, gh, ssh, the editor and the terminal must be `.exe` (or `.com`)
  files: a `.cmd` or `.bat` would run through `cmd.exe`, which parses its arguments (a path
  among them) again (R19). When only a `git.cmd` shim is found, gg says so and names it. VS
  Code's `code` on `PATH` is `code.cmd` (Cursor's `cursor.cmd`): gg runs the `Code.exe`
  (`Cursor.exe`) it starts, found beside it, instead; when that is not there, the message
  names the `.exe` to set as the editor.
- **Stop can leave locks behind**: git is ended at once, so besides `index.lock` a ref lock
  (`refs/heads/<branch>.lock`, `packed-refs.lock`) may stay; gg offers to remove only a stale
  `index.lock`, and git's own message names any other lock.
- **No terminal for children** is `CREATE_NO_WINDOW`: nothing flashes a console, but a program
  that insists on asking in a console waits there until the time limit or Stop, where on Unix
  it fails at once. For ssh, gg runs git with `GIT_SSH_COMMAND="ssh -o BatchMode=yes"`, so a
  passphrase, password or new host key fails at once instead, unless the user chose their ssh:
  `GIT_SSH_COMMAND` or `GIT_SSH` set in the environment, or `core.sshCommand` set for the
  repository (read when the repository is opened). Their choice is left alone, and if it asks
  in a console, it waits there as above.
- **Very many changes at once can go unseen until the window gets the focus**: Windows
  reports a burst that overflows the watcher's buffer as nothing at all (the `notify` crate
  gets an empty buffer and passes nothing on), so gg cannot tell that it missed changes; it
  reads the working tree again when the window gains the focus, as everywhere.
- **A repository's folder is held for a moment after its tab closes**: the watcher lets go of
  the folder just after the tab is gone, so deleting the folder in Explorer at that instant
  can say it is in use; a second later it goes.
- **`PATH` is the app's own**: started from the Start menu, gg sees the user's `PATH` as it was
  when Windows started the session; a `PATH` changed since needs the app restarted.
- **What else is `gg`** is found as `where gg` finds it; a doskey macro or a PowerShell alias
  named `gg` cannot be seen from outside the shell.
- **The end-to-end tests do not run on Windows**: their stand-ins for `gh`, `ssh` and the login
  shell are POSIX `sh` scripts, and gg runs only `.exe` programs there. The Windows paths are
  covered by the Rust tests instead.
- Paths longer than 260 characters work as far as git does (`core.longpaths`); gg does nothing
  special for them.

### Linux

- The working tree is watched folder by folder (inotify gives one watch per folder either way):
  `.git` and folders git ignores are left out, and the watches follow folders as they come and
  go. Past the user's inotify limit (`fs.inotify.max_user_watches`), gg says once that live
  updates are off for the repository and reads changes when the window gets the focus.
- Looking for a running git reads `/proc`; another user's processes cannot be looked into.
- The window's title bar and menu are GTK's, as the desktop draws them. The AppImage needs FUSE
  on the user's system.
- The `gg` command starts the program it was installed from (the AppImage, or the installed
  binary); moved, the command says to install it again.

### macOS

- The app is not signed with an Apple developer certificate (see the README for opening it).

## What CI is expected to show first

These are written but have never run; the first CI run is their test:

- the Windows job object (`platform::job`), program lookup, `gg.cmd` through cmd, and every
  Rust test not marked Unix-only, on Windows;
- inotify (folder-by-folder watching), `/proc`, the Tauri build with the GTK libraries, and the
  end-to-end suite, on Linux;
- the NSIS, `.deb` and AppImage bundles.
