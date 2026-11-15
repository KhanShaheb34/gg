// Renders the social preview (og/og.html → public/og.png) and the PNG icons from
// public/favicon.svg, with Playwright's Chromium. Run from the repository root:
//   node site/scripts/images.mjs
// then make the .ico: magick site/public/favicon-32.png site/public/favicon.ico
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const site = join(dirname(fileURLToPath(import.meta.url)), "..");
const pub = join(site, "public");
const browser = await chromium.launch();

const og = await browser.newPage({ viewport: { width: 1200, height: 630 } });
await og.goto(pathToFileURL(join(site, "og", "og.html")).href, { waitUntil: "load" });
await og.screenshot({ path: join(pub, "og.png") });
await og.close();

const svg = readFileSync(join(pub, "favicon.svg"), "utf8");
for (const [name, size] of [["apple-touch-icon.png", 180], ["favicon-32.png", 32]]) {
  const page = await browser.newPage({ viewport: { width: size, height: size } });
  await page.setContent(`<style>html,body{margin:0;background:transparent}svg{display:block;width:${size}px;height:${size}px}</style>${svg}`);
  await page.screenshot({ path: join(pub, name), omitBackground: true });
  await page.close();
}
await browser.close();
