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
- **Generate with AI.** Any way for gg to run an AI tool without the user's click, to send it
  more than is said below, to put a repository's text on its command line or in the command
  log, or to let the tool run a command, read a file or change the repository (see below).

## Generate with AI

gg itself talks to no AI service. When the user clicks Generate with AI (never otherwise),
gg runs the command-line tool the user installed and signed in to, Claude Code (`claude`) or
Codex (`codex`), found as `gh` is (the login shell's `PATH`; never a program inside an open
repository's folder or one of its worktrees, not even to ask its version), as
an argument array, without a shell or a terminal, in a temporary folder of its own, never in
the repository. gg never reads, stores or passes on a token for it; the tool uses its own
sign-in and sends the prompt to its provider (Anthropic, OpenAI) as it does in a terminal.

- **What is sent.** The prompt (gg's, from Orca, or the user's template from the settings)
  with the branch's name, the list of files, and their diff (for a commit, only the files the
  user ticked; for a pull request, the branch's commits, files and diff since its base, and
  the title and description the form holds), cut to about 200 KB. A binary file is named,
  never sent: any part of the diff that holds a NUL byte is replaced by a line saying so,
  whatever the repository's `.gitattributes` say (a `diff` attribute makes git print a binary
  file as text). Nothing of the repository's configuration runs while gg reads: no external
  diff or text conversion program (`--no-ext-diff`, `--no-textconv`), and no clean or smudge
  filter (each filter driver configured is turned off for those reads with `-c
  filter.<name>.clean=` and the like; git would otherwise run one to read a changed file of the
  working tree). The prompt goes on the tool's standard input; the command line holds only
  fixed words, a model name (checked: letters, digits and `.-_:/@[]`, never starting with `-`
  or `@`) and a path of gg's. The command log keeps neither the prompt nor the diff: any line
  of the tool's stderr that is a line of the prompt is left out, whatever it looks like, and
  Codex is run with `--json`, where it repeats nothing of the prompt and reports its errors as
  events.
- **The diff is not trusted.** A repository's text may try to instruct the model. The prompt
  puts it in code fences it cannot close and says it is data, never instructions; and the
  tool cannot act on it: Claude Code runs with no tools (`--tools ""`), no MCP servers
  (`--strict-mcp-config`), no settings files and so none of their hooks
  (`--setting-sources ""`), no slash commands and no session kept. Codex runs with its shell
  (`shell_tool`, `unified_exec`) and image viewer (`view_image`) turned off, as are web search,
  connectors, plugins, hooks, the browser and the computer; without the user's `config.toml`
  (`--ignore-user-config`, so none of its MCP servers start: `-c mcp_servers={}` does not remove
  the servers a config names, as seen with Codex 0.163 and a dummy server; Codex's sign-in is
  still its own), in its read-only sandbox, with no session kept (`--ephemeral`). Checked
  against Codex 0.163 with a
  stand-in model that asked it to run `cat /etc/hosts`: with the read-only sandbox alone the
  command ran and its output went back to the model; with gg's settings Codex refused it
  ("unsupported call"). Codex still offers its `apply_patch` tool, which the read-only sandbox
  stops from writing. A Codex that does not know one of the switches that keep commands and
  MCP servers off is not run (the user is told to update it).
- **What is left.** The model can still write misleading text into the field from what the diff
  says: gg only fills the field, the user reads it, and nothing is committed or created by it.
  Settings an organization manages for Claude Code (its managed settings and hooks) apply as
  they do anywhere. gg's own status reads (the commit panel's list of changed files) run git
  as a terminal does, filters included; only what is read for a prompt turns them off. A tool
  added to a newer Codex that gg does not know to turn off would be
  offered to the model, inside the read-only sandbox. gg does not scan the diff for secrets
  before sending it: it is the user's own diff, sent to the provider the user already uses for
  that code, on the user's click, after a first-use notice says so.

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

- Vulnerabilities in git, `gh`, Claude Code, Codex, the system's web view (WebKit, WebView2, WebKitGTK) or the
  operating system itself: please report those upstream.
- Attacks that need someone who can already run code as you on your machine.
- The app not being notarized by Apple; that is a known limitation described in the
  README.

## Supported versions

Fixes go into the latest release. Please check that the issue is still present there before
reporting.
