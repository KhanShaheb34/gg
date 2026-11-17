// Screenshots the locally served site and checks its images, into site/.preview/:
// at device scale 1 and 2, widths 2560 to 375, light and dark. For each page it reports
// horizontal overflow, the bytes loaded (all images scrolled into view), where the hero
// screenshot sits, and for each <img> whether it has enough pixels: the file's width >=
// displayed width × devicePixelRatio.
// Usage (from the repository root, with the site served): node site/scripts/preview.mjs [url]
import { chromium } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const url = process.argv[2] ?? "http://127.0.0.1:8790/";
const out = join(dirname(fileURLToPath(import.meta.url)), "..", ".preview");
mkdirSync(out, { recursive: true });

const browser = await chromium.launch();
let failures = 0;

async function visit({ scale, width, scheme, toggle = false }) {
  const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme: scheme, deviceScaleFactor: scale });
  let bytes = 0;
  let firstBytes = 0;
  let loaded = false;
  page.on("response", async (res) => {
    try {
      const n = (await res.body()).length;
      bytes += n;
      if (!loaded) firstBytes += n;
    } catch {}
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.goto(url, { waitUntil: "networkidle" });
  loaded = true;
  if (toggle) {
    await page.click(".theme-toggle");
    await page.waitForLoadState("networkidle");
  }
  // Scroll through so the lazy images load, then back to the top.
  await page.evaluate(async () => {
    for (let y = 0; y < document.body.scrollHeight; y += 500) {
      window.scrollTo(0, y);
      await new Promise((r) => setTimeout(r, 80));
    }
    window.scrollTo(0, 0);
  });
  await page.waitForLoadState("networkidle");
  await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode().catch(() => {}))));
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  const imgs = await page.evaluate(() =>
    Promise.all(
      [...document.querySelectorAll("main img")].map(async (i) => {
        const cs = getComputedStyle(i);
        // The content box: what the image is drawn into (the border is not image).
        const w = i.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
        // naturalWidth of a "2x" candidate is already divided by 2; the file's own pixel
        // width is what has to cover the device pixels.
        const file = new Image();
        file.src = i.currentSrc;
        await file.decode();
        return { src: new URL(i.currentSrc).pathname.replace("/img/", ""), natural: file.naturalWidth, shown: w, dpr: devicePixelRatio };
      }),
    ),
  );
  // Where the hero screenshot sits: its edges against the window, the text column and the lane.
  const hero = await page.evaluate(() => {
    const block = document.querySelector(".hero");
    const b = block.getBoundingClientRect();
    const colLeft = b.left + parseFloat(getComputedStyle(block).paddingLeft);
    const lane = b.left + parseFloat(getComputedStyle(block, "::before").left) + 1;
    const r = document.querySelector(".shot-hero img").getBoundingClientRect(); // border box
    const vw = document.documentElement.clientWidth;
    return { left: r.left, right: vw - r.right, colLeft, colRight: vw - b.right, lane, width: r.width };
  });
  const theme = toggle ? (scheme === "light" ? "dark" : "light") + " (toggled)" : scheme;
  const name = `${width}-${theme.split(" ")[0]}${toggle ? "-toggled" : ""}@${scale}x`;
  // Whole page at 1×; at 2× the top of the page (header and hero), to keep the files sane.
  if (!toggle) {
    await page.screenshot(
      scale === 1 ? { path: join(out, `${name}.png`), fullPage: true } : { path: join(out, `${name}-top.png`), clip: { x: 0, y: 0, width, height: 1800 }, fullPage: true },
    );
  }
  console.log(
    `\n${width} px, ${scale}x, ${theme}: overflow ${overflow}px, first load ${(firstBytes / 1024).toFixed(0)} KB, all images ${(bytes / 1024).toFixed(0)} KB${errors.length ? ", errors: " + errors.join(" | ") : ""}`,
  );
  const px = (n) => `${Math.round(n)}px`;
  console.log(
    `  hero: left ${px(hero.left)}, right ${px(hero.right)} from the window; text column left ${px(hero.colLeft)}, right ${px(hero.colRight)}; lane at ${px(hero.lane)}; ${px(hero.width)} wide with its border`,
  );
  for (const i of imgs) {
    const need = i.shown * i.dpr;
    const ok = i.natural >= need - 0.5;
    if (!ok) failures++;
    console.log(`  ${ok ? "ok  " : "LOW "} ${i.src.padEnd(28)} natural ${String(i.natural).padStart(4)}  shown ${String(i.shown).padStart(4)} × ${i.dpr} = ${String(Math.round(need)).padStart(4)}`);
  }
  await page.close();
}

for (const scale of [1, 2])
  for (const width of [2560, 1920, 1680, 1440, 1280, 768, 375])
    for (const scheme of ["light", "dark"]) await visit({ scale, width, scheme });
// The page's own toggle, against the system's preference.
await visit({ scale: 2, width: 1440, scheme: "light", toggle: true });
await visit({ scale: 2, width: 375, scheme: "dark", toggle: true });
await browser.close();
console.log(failures ? `\n${failures} image(s) with too few pixels` : "\nevery image has enough pixels");
process.exit(failures ? 1 : 0);
