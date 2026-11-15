# Contributing to gg

Thanks for taking the time. Bug reports, small fixes and well-argued ideas are all welcome.
For anything larger than a small fix, open an issue first so we can agree on the shape
before you spend time on it.

## Setting up

You need [Rust](https://rustup.rs) (stable), [Node.js](https://nodejs.org),
[pnpm](https://pnpm.io), git 2.29 or later, and on macOS Xcode's command line tools.

```sh
pnpm install          # JavaScript dependencies
pnpm tauri dev        # the desktop app, with hot reload of the UI
```

The UI also runs in a plain browser against `gg-devserver`, which is how the end-to-end
tests drive it:

```sh
pnpm dev                                                   # UI on http://127.0.0.1:1420
cargo run -p gg-devserver -- --config-dir /tmp/gg-config   # core on http://127.0.0.1:1421
```

## Where things are

[docs/architecture.md](docs/architecture.md) maps the code: the Rust core in
`crates/gg-core`, the Tauri shell in `src-tauri`, the dev server in `crates/gg-devserver`,
and the Svelte UI in `src/`.

## Rules the code keeps

- **The UI never builds git commands.** It calls typed core commands ("merge this branch
  into the current one, with these options") and renders typed results. Every git command
  is built in `crates/gg-core` and run through `GitRunner`.
- **git output is parsed only in the readers**, and only in machine-readable forms
  (`--porcelain`, `-z`, an explicit `--format`).
- **git never waits for input.** No prompts, no editors, no terminal; something that would
  ask fails with a plain message.
- **Anything that can lose work is a danger checked by the core** (`ops/danger.rs`), with a
  test showing it cannot run unconfirmed.
- **Nothing is written to a repository unless the user asked for it**, apart from the
  background fetch.
- Messages the user reads are plain sentences that say what happened and, where it helps,
  what to do next.

## Checks a change must pass

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace     # Rust unit and integration tests
pnpm check                 # svelte-check and tsc
pnpm test                  # UI unit tests (Vitest)
pnpm e2e                   # end-to-end tests (Playwright, Chromium)
```

`pnpm e2e` builds `gg-devserver` once and starts a fresh one for each test; the first run
also needs `pnpm exec playwright install chromium`.

Add tests with the change: a reader or an operation gets a Rust test on a real throwaway
repository (`gg_core::testutil`), a UI behaviour gets a Vitest or Playwright test. A change
that could affect speed should be measured as described in
[docs/performance.md](docs/performance.md).

## Commits and pull requests

- Keep a pull request to one change, and say what it changes for the user and how you
  checked it.
- Commit subjects follow the conventional style used in the history, for example
  `fix(push): …`, `feat(ui): …`, `docs: …`, `test(e2e): …`.
- Screenshots help for anything visible, in both light and dark themes.

## Reporting a bug

Open an issue with the bug report template. The most useful thing you can attach is the
**command log**: press ⌘⇧L (or the terminal button in the toolbar), find the operation that
went wrong, and copy its command lines (each has a Copy button) and the output shown under
them. Credentials in URLs are already masked there, but check the text for paths or names
you would rather not share before you post it.

Please also give your macOS version, `git --version`, and the gg version (shown in the app
menu's About window).

Security issues go through [SECURITY.md](SECURITY.md), not public issues.
