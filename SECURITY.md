# Security policy

## Reporting a vulnerability

Please report security issues privately, not in a public issue or pull request.

Use GitHub's private reporting: open the repository's **Security** tab and choose
**Report a vulnerability** (or go to
https://github.com/KhanShaheb34/gg/security/advisories/new). Only the maintainer can see the
report, and we can discuss and fix it there before anything is made public.

Include what you did, what happened, and what you expected: a repository layout or a
command sequence that shows the problem is ideal. You will get an answer as soon as the
maintainer can look at it; please allow some time before disclosing it elsewhere.

## In scope

gg runs git on repositories that may come from anyone, so these matter most:

- **Running commands derived from repository content.** Anything that lets a repository's
  contents, names, config or remote data make gg run a program, pass an option to git it did
  not intend, or go through a shell. (Hooks and other programs that git itself runs by
  design, as it would in a terminal, are not a gg issue.)
- **Losing work without a confirmation.** Any way for a destructive action (force push, hard
  reset, deleting a branch, tag, stash or worktree, aborting a merge or rebase) to run
  without the confirmation that says what will be lost, or after the state it described has
  changed.
- **Leaking credentials.** Tokens or passwords in remote URLs, credential helper output or
  `gh` shown unmasked anywhere in the app, written to its files or logs, or sent anywhere
  other than the remote they belong to.
- **Shell start-up files.** gg writes to one only when you ask it to turn off a `gg` alias
  or function, and then only its one marked line at the end, which it takes out again. Any
  way for gg to write there without being asked, to write anything else, or to change or lose
  anything already in the file, is in scope.
- The development server (`gg-devserver`) answering anything other than the local machine
  or the allowed origins.
- **Updates.** Any way to make gg install something that is not a release signed with gg's
  key, an older release, or to make it fetch updates from anywhere but the release on
  GitHub (see below).

## Updates

gg updates itself with Tauri's updater plugin, driven by the app's core (the web view has no
access to it):

- **One source.** The app asks one URL, over HTTPS:
  `https://github.com/KhanShaheb34/gg/releases/latest/download/latest.json`. It is compiled
  into the app (`src-tauri/tauri.conf.json`); no setting, argument or environment variable
  changes it. Tests (`src-tauri/tests/update_source.rs` and `lib.rs`'s) check that the shipped
  configuration names that URL and the key below and nothing that loosens the checks; only
  the development build used for performance tests (`--features perf`, bundle identifier
  `com.shakirul.gg.perf`) can be pointed at a local test server, with a test key.
- **Signed.** Each release's update (the macOS `.app.tar.gz`, the Linux AppImage, the Windows
  installer) is signed with a minisign key held only as GitHub Actions secrets
  (`TAURI_SIGNING_PRIVATE_KEY`, password-protected, and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).
  One step of the release workflow sees them: "Sign the updates", in a job that checks nothing
  out and builds nothing, which runs only the release's signer (`crates/gg-release`, built
  beforehand in another job without the secrets) by its absolute path with a clean `PATH`. The
  jobs that build the app, its dependencies and its build scripts never see the key; the
  signer prints only file names, and only the signatures are kept as artifacts. Without the
  secrets that step fails and nothing is released. The app verifies the
  download against the public key built into it before installing anything; a download that
  does not verify, or whose signature was made for another version than the one offered
  (`requireSignedVersion`), is refused and nothing is installed. The public key is:

  ```text
  untrusted comment: minisign public key: 86DEEA715E3B4F95
  RWSVTztecerehnGJxp5eHACEvK0QoZevuuJ/BrG75e6vUuwbELWNkl1g
  ```

- **Checked before it is published.** Before the draft release is created, the workflow
  verifies every update's signature against the key in the app's config, with the same
  verifier the app uses (`crates/gg-release`), and checks that `latest.json` names the
  release's version and only that release's own assets.
- **Never older.** Only a strictly newer version is offered: never the same one, an older
  one or a pre-release.
- **Private.** The check asks GitHub for a small file describing the latest release (GitHub
  answers through a redirect to its file host). It carries nothing about the user, what is on
  their machine or their repositories beyond what any web request does (the IP address, and a
  user agent naming the updater), and can be turned off in Settings → About.

## Out of scope

- Vulnerabilities in git, `gh`, the system's web view (WebKit, WebView2, WebKitGTK) or the
  operating system itself: please report those upstream.
- Attacks that need someone who can already run code as you on your machine.
- The app not being notarized by Apple; that is a known limitation described in the
  README.

## Supported versions

Fixes go into the latest release. Please check that the issue is still present there before
reporting.
