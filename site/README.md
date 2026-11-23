# gg.shakirul.com

The landing page for gg: plain HTML, CSS and a little JavaScript in `public/`, served by
Cloudflare Workers static assets (`wrangler.jsonc`, no Worker script). No build step: the one
generated page, the changelog, is committed as it is served.

From this folder:

```sh
npx wrangler@4 dev       # preview on http://localhost:8787 (applies _headers and _redirects)
sh scripts/deploy.sh      # publish to gg.shakirul.com; checks the download is present and matches latest.json
```

## What lives where

- `public/latest.json`: the release shown on the page (`version`, `file`, `sizeMb`,
  `sha256`). A release script rewrites it; the HTML needs no change. The size and the SHA-256
  appear only when set. The page links `/download/<file>`; put the DMG at
  `public/download/<file>` before deploying. `index.html` carries the version too, as the
  fallback for visitors without JavaScript.
- `changelog.md`: what changed in each release, newest first, shown at `/changelog`.
  `node site/scripts/changelog.mjs` makes `public/changelog.html` from it and dates the page's
  line in `public/sitemap.xml`; both are committed. To add a release, with each release of the
  app:

  1. At the top of `changelog.md`, add `## X.Y.Z (YYYY-MM-DD)` (the GitHub release's
     publication day, UTC) and under it the release notes' opening sentence and their "What
     changed" section, without the downloads, first-launch and Windows/Linux parts that every
     release repeats. GitHub links go through `/github/...`.
  2. Run `node site/scripts/changelog.mjs`, look at `/changelog` in `wrangler dev`, and commit
     `changelog.md`, `public/changelog.html` and `public/sitemap.xml` together.

  The script stops, naming the line, on anything the page cannot show or on releases out of
  order. `deploy.sh` runs `node scripts/changelog.mjs --check` first and refuses to publish
  when the page differs from `changelog.md`, or when the newest entry is not
  `public/latest.json`'s version (so a release cannot go out without its entry, and an
  unreleased version cannot be listed).
- `public/_redirects`: `/github` points at the source repository. Every GitHub link on the
  page goes through it, so a rename of the repository is a one-line change there.
- `public/_headers`: security headers (a strict CSP: no third-party anything) and caching.
- `public/img/`: the app's screenshots, each cropped into `<name>-<theme>-1x.webp` and
  `-2x.webp` (lossless) for `srcset`. One command, from the repository root, remakes them all:

  ```sh
  bash site/scripts/screens.sh   # capture at 1× and 2×, then crop; --no-capture to only crop
  node site/scripts/images.mjs   # then the social preview (public/og.png) and the icons
  ```

  The capture (`scripts/shots.mjs`, `scripts/shots.config.ts`) runs a copy of the app's own
  `e2e/screens.spec.ts` at device scale 1 and 2 against gg-devserver and the Vite dev server,
  as the e2e tests do, and writes the shots to `.shots/` (git-ignored). It needs
  `pnpm install`, cargo (it builds gg-devserver into this checkout's `target/`; delete that
  folder afterwards, it is large), ImageMagick and cwebp, and port 1420 free.
- `og/og.html`: the social preview, drawn from the 2× capture at twice its size and scaled to
  1200×630.
- `node site/scripts/preview.mjs [url]` (site served) screenshots the page at device scale 1
  and 2, 1920 to 375 px wide, both themes, into `.preview/` (git-ignored); prints overflow,
  bytes loaded, and for every image whether it has at least as many pixels as it is shown at.
