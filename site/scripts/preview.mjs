// Screenshots the locally served site at 1440, 768 and 375 px in light and dark, into
// site/.preview/, and reports horizontal overflow and what the page loaded.
// Usage (from the repository root, with the site served): node site/scripts/preview.mjs [url]
import { chromium } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const url = process.argv[2] ?? "http://127.0.0.1:8790/";
const out = join(dirname(fileURLToPath(import.meta.url)), "..", ".preview");
mkdirSync(out, { recursive: true });

const browser = await chromium.launch();
for (const scheme of ["light", "dark"]) {
  for (const width of [1440, 768, 375]) {
    const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme: scheme, deviceScaleFactor: 2 });
    let bytes = 0;
    const files = [];
    page.on("response", async (res) => {
      try {
        const body = await res.body();
        bytes += body.length;
        files.push(`${new URL(res.url()).pathname} ${body.length}`);
      } catch {}
    });
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
    await page.goto(url, { waitUntil: "networkidle" });
    // Scroll through so lazy images load, then back.
    await page.evaluate(async () => {
      for (let y = 0; y < document.body.scrollHeight; y += 600) {
        window.scrollTo(0, y);
        await new Promise((r) => setTimeout(r, 60));
      }
      window.scrollTo(0, 0);
    });
    await page.waitForLoadState("networkidle");
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    await page.screenshot({ path: join(out, `${width}-${scheme}.png`), fullPage: true });
    console.log(`${width} ${scheme}: overflow ${overflow}px, ${files.length} files, ${(bytes / 1024).toFixed(1)} KB${errors.length ? ", errors: " + errors.join(" | ") : ""}`);
    if (width === 1440 && scheme === "light") console.log("  " + files.join("\n  "));
    await page.close();
  }
}
await browser.close();
