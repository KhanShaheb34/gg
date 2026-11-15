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
- The development server (`gg-devserver`) answering anything other than the local machine
  or the allowed origins.

## Out of scope

- Vulnerabilities in git, `gh`, WebKit or macOS themselves: please report those upstream.
- Attacks that need someone who can already run code as you on your machine.
- The app not being notarized by Apple; that is a known limitation described in the
  README.

## Supported versions

Fixes go into the latest release. Please check that the issue is still present there before
reporting.
