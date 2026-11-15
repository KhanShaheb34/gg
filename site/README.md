# gg.shakirul.com

The landing page for gg: plain HTML, CSS and a little JavaScript in `public/`, served by
Cloudflare Workers static assets (`wrangler.jsonc`, no Worker script). No build step.

From this folder:

```sh
npx wrangler@4 dev       # preview on http://localhost:8787 (applies _headers and _redirects)
npx wrangler@4 deploy    # publish to gg.shakirul.com (needs `wrangler login` once)
```

## What lives where

- `public/latest.json`: the release shown on the page (`version`, `file`, `sizeMb`,
  `sha256`). A release script rewrites it; the HTML needs no change. The size and the SHA-256
  appear only when set. The page links `/download/<file>`; put the DMG at
  `public/download/<file>` before deploying. `index.html` carries the version too, as the
  fallback for visitors without JavaScript.
- `public/_redirects`: `/github` points at the source repository. Every GitHub link on the
  page goes through it, so a rename of the repository is a one-line change there.
- `public/_headers`: security headers (a strict CSP: no third-party anything) and caching.
- `public/img/`: crops of the app's screenshots, made by `scripts/screens.sh` from
  `docs/perf/screens/` (needs ImageMagick and cwebp).
- `og/og.html`: the social preview; `node site/scripts/images.mjs` (from the repository
  root, Playwright installed) renders it to `public/og.png` along with the PNG icons.
- `node site/scripts/preview.mjs [url]` screenshots the served page at 1440, 768 and 375 px in
  both themes into `.preview/` (git-ignored) and prints overflow and bytes loaded.
