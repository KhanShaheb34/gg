// Renders the social preview (og/og.html → public/og.png, 1200×630) and the icons from
// public/favicon.svg, with Playwright's Chromium and ImageMagick. Run from the repository
// root after scripts/screens.sh (og.html uses its 2× shot in site/.shots/2x):
//   node site/scripts/images.mjs
import { chromium } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const site = join(dirname(fileURLToPath(import.meta.url)), "..");
const pub = join(site, "public");
const tmp = mkdtempSync(join(tmpdir(), "gg-site-images-"));
const browser = await chromium.launch();

try {
  // Drawn at 2× and scaled down: text and the screenshot come out sharper than a 1× render.
  const og = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 2 });
  await og.goto(pathToFileURL(join(site, "og", "og.html")).href, { waitUntil: "load" });
  await og.screenshot({ path: join(tmp, "og@2x.png") });
  await og.close();
  execFileSync("magick", [join(tmp, "og@2x.png"), "-filter", "Lanczos", "-resize", "1200x630", "-strip", join(pub, "og.png")]);

  const svg = readFileSync(join(pub, "favicon.svg"), "utf8");
  for (const [file, size] of [[join(pub, "apple-touch-icon.png"), 180], [join(tmp, "favicon-32.png"), 32]]) {
    const page = await browser.newPage({ viewport: { width: size, height: size } });
    await page.setContent(`<style>html,body{margin:0;background:transparent}svg{display:block;width:${size}px;height:${size}px}</style>${svg}`);
    await page.screenshot({ path: file, omitBackground: true });
    await page.close();
  }
  execFileSync("magick", [join(tmp, "favicon-32.png"), join(pub, "favicon.ico")]);
} finally {
  await browser.close();
  rmSync(tmp, { recursive: true, force: true });
}
