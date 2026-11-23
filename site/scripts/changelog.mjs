// Makes the changelog page, public/changelog.html, from changelog.md, and keeps the page's
// line in public/sitemap.xml dated to the newest release. Plain node, no dependencies.
//
//   node site/scripts/changelog.mjs          write both files
//   node site/scripts/changelog.mjs --check  write nothing; fail if either file differs from
//                                            what changelog.md makes, or if the newest entry
//                                            is not the version in public/latest.json
//
// scripts/deploy.sh runs the check, so a page that has drifted from its source, or a release
// without an entry, is not published. changelog.md says how to add a release.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const site = join(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = join(site, "changelog.md");
const PAGE = join(site, "public", "changelog.html");
const SITEMAP = join(site, "public", "sitemap.xml");
const LATEST = join(site, "public", "latest.json");

const ORIGIN = "https://gg.shakirul.com";
const PAGE_URL = `${ORIGIN}/changelog`;
const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

class SourceError extends Error {}

function fail(line, message) {
  throw new SourceError(`changelog.md:${line}: ${message}`);
}

// ---- Reading changelog.md ---------------------------------------------------------------

const RELEASE = /^## (\d+)\.(\d+)\.(\d+) \((\d{4})-(\d{2})-(\d{2})\)$/;

function parse(text) {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  let i = 0;

  // The opening comment, then nothing but blank lines before the first release.
  while (i < lines.length && lines[i].trim() === "") i++;
  if (lines[i]?.startsWith("<!--")) {
    while (i < lines.length && !lines[i].includes("-->")) i++;
    if (i === lines.length) fail(1, "the opening comment has no closing -->");
    i++;
  }

  const releases = [];
  let release = null;
  let open = null; // the paragraph or list item that the next line may continue

  for (; i < lines.length; i++) {
    const n = i + 1;
    const line = lines[i];
    if (line.startsWith("## ") || line.startsWith("##\t")) {
      const m = RELEASE.exec(line.trimEnd());
      if (!m) fail(n, `a release heading is "## X.Y.Z (YYYY-MM-DD)", such as "## 0.4.0 (2026-11-21)"; this is "${line}"`);
      const [, a, b, c, y, mo, d] = m;
      const date = new Date(Date.UTC(+y, +mo - 1, +d));
      if (date.getUTCMonth() !== +mo - 1 || date.getUTCDate() !== +d) fail(n, `${y}-${mo}-${d} is not a date`);
      release = { version: `${+a}.${+b}.${+c}`, parts: [+a, +b, +c], date: `${y}-${mo}-${d}`, line: n, blocks: [] };
      releases.push(release);
      open = null;
      continue;
    }
    if (line.trim() === "") {
      open = null;
      continue;
    }
    if (!release) fail(n, `text before the first release heading: "${line}"`);
    const blocks = release.blocks;
    const last = blocks[blocks.length - 1];
    if (line.startsWith("### ")) {
      blocks.push({ type: "h3", text: line.slice(4).trim(), line: n });
      open = null;
    } else if (line.startsWith("- ")) {
      const item = { text: line.slice(2).trim(), line: n };
      if (last?.type === "ul") last.items.push(item);
      else blocks.push({ type: "ul", items: [item], line: n });
      open = item;
    } else if (/^\s/.test(line) && open && last?.type === "ul") {
      open.text += " " + line.trim();
    } else if (/^(#|\||>|[*+] |\d+[.)] |<|!\[|```|~~~|\t|    )/.test(line)) {
      fail(n, `the page cannot show this (only paragraphs, "### " headings, "- " lists, **bold**, \`code\` and [links](...)): "${line}"`);
    } else if (open) {
      open.text += " " + line.trim();
    } else {
      open = { type: "p", text: line.trim(), line: n };
      blocks.push(open);
    }
  }

  if (releases.length === 0) fail(1, 'no releases; each starts with "## X.Y.Z (YYYY-MM-DD)"');
  const seen = new Set();
  for (let k = 0; k < releases.length; k++) {
    const r = releases[k];
    if (r.blocks.length === 0) fail(r.line, `${r.version} has no text`);
    if (seen.has(r.version)) fail(r.line, `${r.version} is listed twice`);
    seen.add(r.version);
    const prev = releases[k - 1];
    if (prev) {
      if (compare(prev.parts, r.parts) <= 0) fail(r.line, `${r.version} comes after ${prev.version}; the newest release goes first`);
      if (prev.date < r.date) fail(r.line, `${r.version} (${r.date}) is dated after ${prev.version} (${prev.date}), which is above it`);
    }
  }
  return releases;
}

function compare(a, b) {
  for (let k = 0; k < 3; k++) if (a[k] !== b[k]) return a[k] - b[k];
  return 0;
}

// ---- Writing HTML -------------------------------------------------------------------------

function escape(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function attr(s) {
  return escape(s).replace(/"/g, "&quot;");
}

// **bold**, `code` and [text](url), in any nesting the release notes use: code is set aside
// first, so ** and [ inside it stay as they are.
function inline(text, line) {
  const codes = [];
  let s = text.replace(/`([^`]+)`/g, (_, c) => {
    codes.push(`<code>${escape(c)}</code>`);
    return `\u0000${codes.length - 1}\u0000`;
  });
  if (s.includes("`")) fail(line, "a ` without its closing `");
  s = escape(s);
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, label, url) => {
    if (!/^(\/|#|https:\/\/)/.test(url)) fail(line, `link to "${url}": use /github/..., #anchor or https://`);
    return `<a href="${attr(url.replace(/&amp;/g, "&"))}">${label}</a>`;
  });
  if (/\]\(/.test(s)) fail(line, "a link that is not [text](url)");
  s = s.replace(/\*\*(.+?)\*\*/g, "<b>$1</b>");
  if (s.includes("**")) fail(line, "a ** without its closing **");
  return s.replace(/\u0000(\d+)\u0000/g, (_, k) => codes[+k]);
}

function longDate(iso) {
  const [y, m, d] = iso.split("-").map(Number);
  return `${d} ${MONTHS[m - 1]} ${y}`;
}

function renderRelease(r, newest) {
  const id = `v${r.version}`;
  const out = [];
  out.push(`  <article class="block release" id="${id}" aria-labelledby="${id}-title">`);
  out.push(`    <span class="node" aria-hidden="true"></span>`);
  out.push(`    <header class="release-head">`);
  out.push(`      <h2 class="release-version" id="${id}-title"><a class="ref ref-tag" href="#${id}"><span class="sr-only">gg </span>${id}</a></h2>`);
  if (newest) out.push(`      <span class="ref ref-latest">latest</span>`);
  out.push(`      <p class="release-meta"><time datetime="${r.date}">${longDate(r.date)}</time><a href="/github/releases/tag/${id}">Downloads on GitHub</a></p>`);
  out.push(`    </header>`);
  r.blocks.forEach((b, k) => {
    if (b.type === "p") {
      const cls = k === 0 ? ` class="release-lede"` : "";
      out.push(`    <p${cls}>${inline(b.text, b.line)}</p>`);
    } else if (b.type === "h3") {
      out.push(`    <h3>${inline(b.text, b.line)}</h3>`);
    } else {
      out.push(`    <ul class="changes">`);
      for (const item of b.items) out.push(`      <li>${inline(item.text, item.line)}</li>`);
      out.push(`    </ul>`);
    }
  });
  out.push(`  </article>`);
  return out.join("\n");
}

function renderPage(releases) {
  const title = "Changelog · gg";
  const description =
    "What changed in each release of gg, the small, fast desktop git client built around the commit graph. Newest first, with a link to each version's downloads.";
  const social = "What changed in each release of gg, the desktop git client built around the commit graph.";
  return `<!doctype html>
<!-- Made by site/scripts/changelog.mjs from site/changelog.md. Edit that file, not this one. -->
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${title}</title>
<meta name="description" content="${attr(description)}">
<meta name="color-scheme" content="light dark">
<meta name="theme-color" media="(prefers-color-scheme: light)" content="#fbfbfc">
<meta name="theme-color" media="(prefers-color-scheme: dark)" content="#17181c">
<link rel="canonical" href="${PAGE_URL}">
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="icon" href="/favicon.ico" sizes="32x32">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<meta property="og:type" content="website">
<meta property="og:site_name" content="gg">
<meta property="og:url" content="${PAGE_URL}">
<meta property="og:title" content="${title}">
<meta property="og:description" content="${attr(social)}">
<meta property="og:image" content="${ORIGIN}/og.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:image:type" content="image/png">
<meta property="og:image:alt" content="gg: the commit graph, without the editor. A screenshot of its commit graph.">
<meta property="og:locale" content="en_US">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="${title}">
<meta name="twitter:description" content="${attr(social)}">
<meta name="twitter:image" content="${ORIGIN}/og.png">
<meta name="twitter:image:alt" content="gg: the commit graph, without the editor. A screenshot of its commit graph.">
<meta name="author" content="Shakirul">
<meta name="robots" content="index, follow">
<link rel="stylesheet" href="/site.css">
<script src="/theme.js"></script>
<script src="/main.js" defer></script>
</head>
<body>
<a class="skip" href="#main">Skip to content</a>

<header class="top">
  <a class="brand" href="/">
    <svg class="mark" viewBox="0 0 32 32" aria-hidden="true"><use href="#mark"/></svg>
    <span>gg</span>
  </a>
  <a class="top-link" href="/changelog" aria-current="page">Changelog</a>
  <a class="top-link" href="/github">GitHub</a>
  <button class="theme-toggle" type="button" aria-label="Switch to dark theme" title="Switch theme">
    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 1.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor"/></svg>
  </button>
</header>

<div class="log">
<main id="main">

  <section class="block hero changelog-hero" aria-labelledby="title">
    <span class="node node-head" aria-hidden="true"></span>
    <p class="refs" aria-hidden="true"><span class="ref ref-head">main</span></p>
    <h1 id="title" class="pitch">Changelog</h1>
    <p class="lede">What changed in each release of gg, newest first. Each version links to its release on GitHub, with the downloads for every system and their checksums.</p>
  </section>

${releases.map((r, k) => renderRelease(r, k === 0)).join("\n\n")}

</main>

<footer class="block foot">
  <span class="node node-root" aria-hidden="true"></span>
  <p>gg is free and open source, under the MIT licence.</p>
  <p class="foot-links"><a href="/">Home page</a><a href="/github">Source on GitHub</a><a href="https://shakirul.com">Built by Shakirul</a></p>
</footer>
</div>

<svg width="0" height="0" class="sprite" aria-hidden="true">
  <symbol id="mark" viewBox="0 0 32 32"><rect width="32" height="32" rx="7" fill="#17181c"/><path d="M11 7v18" stroke="#6b9bff" stroke-width="2.5" stroke-linecap="round" fill="none"/><path d="M21 10.5c0 6-10 4.5-10 11" stroke="#fb8a4e" stroke-width="2.5" stroke-linecap="round" fill="none"/><circle cx="11" cy="8" r="3.2" fill="#6b9bff"/><circle cx="21" cy="10.5" r="3.2" fill="#fb8a4e"/><circle cx="11" cy="24" r="3.2" fill="#6b9bff"/></symbol>
</svg>
</body>
</html>
`;
}

// The sitemap's changelog line, dated to the newest release; added before </urlset> if absent.
function renderSitemap(current, newest) {
  const entry = `  <url><loc>${PAGE_URL}</loc><lastmod>${newest.date}</lastmod></url>`;
  const lines = current.split("\n");
  const at = lines.findIndex((l) => l.includes(`<loc>${PAGE_URL}</loc>`));
  if (at >= 0) lines[at] = entry;
  else {
    const end = lines.findIndex((l) => l.trim() === "</urlset>");
    if (end < 0) throw new SourceError("public/sitemap.xml has no </urlset> line");
    lines.splice(end, 0, entry);
  }
  return lines.join("\n");
}

// ---- Main ---------------------------------------------------------------------------------

function read(path) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

const check = process.argv.includes("--check");
try {
  const releases = parse(read(SOURCE) ?? fail(1, "missing"));
  const page = renderPage(releases);
  const sitemapNow = read(SITEMAP);
  if (sitemapNow === null) throw new SourceError("public/sitemap.xml is missing");
  const sitemap = renderSitemap(sitemapNow, releases[0]);

  let latest = null;
  try {
    latest = JSON.parse(read(LATEST) ?? "null")?.version ?? null;
  } catch {}
  const mismatch =
    latest !== releases[0].version
      ? `the newest entry in changelog.md is ${releases[0].version}, but public/latest.json says ${latest ?? "nothing"}: ` +
        "the changelog lists released versions only, and every one of them"
      : null;

  if (check) {
    const problems = [];
    if (read(PAGE) !== page) problems.push("public/changelog.html is not what changelog.md makes; run node site/scripts/changelog.mjs and commit the result");
    if (sitemapNow !== sitemap) problems.push("public/sitemap.xml's changelog line is out of date; run node site/scripts/changelog.mjs and commit the result");
    if (mismatch) problems.push(mismatch);
    if (problems.length) {
      for (const p of problems) console.error(`changelog: ${p}`);
      process.exit(1);
    }
    console.log(`changelog: up to date (${releases.length} releases, newest ${releases[0].version})`);
  } else {
    writeFileSync(PAGE, page);
    writeFileSync(SITEMAP, sitemap);
    console.log(`changelog: wrote public/changelog.html (${releases.length} releases, newest ${releases[0].version}) and public/sitemap.xml`);
    if (mismatch) console.warn(`changelog: warning: ${mismatch}. Deploying will refuse until they agree.`);
  }
} catch (e) {
  if (!(e instanceof SourceError)) throw e;
  console.error(`changelog: ${e.message}`);
  process.exit(1);
}
