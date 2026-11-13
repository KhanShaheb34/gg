# GitGraph

## Dev commands

```sh
pnpm install                 # install JS dependencies
pnpm dev                     # vite dev server on http://127.0.0.1:1420
pnpm test                    # unit tests (vitest)
pnpm e2e                     # browser tests (Playwright, chromium)
pnpm check                   # type-check (svelte-check)
pnpm tauri dev               # run the desktop app
pnpm tauri build --debug     # build a debug app bundle
cargo test --workspace       # Rust tests
```

The browser build talks to `gg-devserver` (dev/test only) on http://127.0.0.1:1421:

```sh
cargo run -p gg-devserver -- --config-dir /tmp/gg-config [--open <repo>] [--allow-origin <origin>]
curl -s -XPOST 127.0.0.1:1421/invoke -H 'content-type: application/json' \
  -d '{"cmd":"get_app_state","args":{}}'
```

It answers only loopback `Host` names, and browsers only from http://127.0.0.1:1420,
http://localhost:1420 and each `--allow-origin`; `/invoke` takes only `application/json`.
