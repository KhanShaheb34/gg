// gg updating itself, end to end against the real core with the devserver's stand-in for the
// updater (nothing is downloaded, nothing restarts): a newer version is downloaded in the
// background once the first rows are on screen and a quiet notice offers the restart;
// dismissed, it comes back at the next start, and Settings → About keeps saying it; a failed
// check says so in Settings only; a copy that cannot update itself says a version is available,
// why, and where to get it; the automatic checks follow their setting.
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, type GG, openFolder, test } from "./fixtures";

test.use({ viewport: { width: 1280, height: 800 } });

/** The first check runs a few seconds after the first rows. */
const CHECKED = { timeout: 15_000 };

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await openFolder(page);
  await expect(page.getByRole("tab", { name: path.basename(dir), exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("grid", { name: "Commits" }).locator('[data-index="0"] .subject')).not.toBeEmpty();
}

const notices = (page: Page) => page.getByRole("region", { name: "Notifications" });
const notice = (page: Page, text: string) => notices(page).locator(".toast").filter({ hasText: text });
const sheet = (page: Page) => page.getByTestId("settings");
const about = async (page: Page) => {
  await page.keyboard.press("ControlOrMeta+,");
  await sheet(page).getByRole("button", { name: "About", exact: true }).click();
  return sheet(page);
};
const stored = (gg: GG) => {
  const file = path.join(gg.home, "config", "state.json");
  return existsSync(file) ? ((JSON.parse(readFileSync(file, "utf8")) as { settings?: Record<string, unknown> }).settings ?? {}) : {};
};

test("a newer version is downloaded in the background, then a notice offers the restart, and Settings says so too", async ({ gg }) => {
  const { page } = gg;
  gg.updates.running("0.4.0");
  gg.updates.offer("0.9.0");
  gg.updates.holdDownloads();
  await page.goto("/");
  await open(page, gg.repoPath("small"));

  // Downloading (held halfway): said in Settings only, never in a notice.
  const s = await about(page);
  const status = s.getByTestId("settings-update-status");
  await expect(status).toHaveText("Downloading gg 0.9.0… 50%", CHECKED);
  await expect(s.getByTestId("settings-update-check")).toBeDisabled();
  await expect(notices(page).locator(".toast")).toHaveCount(0);
  gg.updates.openDownloadGate();
  await expect(status).toHaveText("gg 0.9.0 is ready. Restart to update.", CHECKED);
  await expect(s.getByTestId("settings-update-restart")).toBeVisible();
  await page.keyboard.press("Escape");

  // The notice: quiet (a status, not an alert), with the restart and a way to dismiss it.
  const ready = notice(page, "gg 0.9.0 is ready");
  await expect(ready).toHaveAttribute("role", "status");
  await expect(ready.getByRole("button", { name: "Restart to update" })).toBeVisible();
  await expect(ready.getByRole("button", { name: "Dismiss notification" })).toBeVisible();
  // It stays: an update waits for the user.
  await page.waitForTimeout(11_000);
  await expect(ready).toBeVisible();
  expect(gg.updates.restarts()).toEqual([]);

  // Restart: asked of the core once (the stand-in records it; nothing restarts here).
  await ready.getByRole("button", { name: "Restart to update" }).click();
  await expect.poll(() => gg.updates.restarts()).toEqual(["0.4.0"]);
});

test("dismissed, the notice comes back at the next start; Settings keeps the restart", async ({ gg }) => {
  const { page } = gg;
  gg.updates.running("0.4.0");
  gg.updates.offer("0.9.0");
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const ready = notice(page, "gg 0.9.0 is ready");
  await expect(ready).toBeVisible(CHECKED);
  // From the keyboard: F6 goes to the newest notice, its first button the restart.
  await page.getByRole("grid", { name: "Commits" }).focus();
  await page.keyboard.press("F6");
  await expect(ready.getByRole("button", { name: "Restart to update" })).toBeFocused();
  await ready.getByRole("button", { name: "Dismiss notification" }).click();
  await expect(ready).toHaveCount(0);

  // Check now finds it ready again: no second notice this run.
  const s = await about(page);
  await s.getByTestId("settings-update-check").click();
  await expect(s.getByTestId("settings-update-status")).toHaveText("gg 0.9.0 is ready. Restart to update.", CHECKED);
  await page.waitForTimeout(500);
  await expect(notices(page).locator(".toast")).toHaveCount(0);
  await s.getByTestId("settings-update-restart").click();
  await expect.poll(() => gg.updates.restarts()).toEqual(["0.4.0"]);

  // The next start: the tab is back, and so is the notice.
  await gg.relaunch();
  await expect(page.getByRole("tab", { name: "small" })).toHaveAttribute("aria-selected", "true");
  await expect(notice(page, "gg 0.9.0 is ready")).toBeVisible(CHECKED);
});

test("a failed check is quiet: Settings says what happened, and Check now tries again", async ({ gg }) => {
  const { page } = gg;
  gg.updates.running("0.4.0");
  gg.updates.checkFails("offline", "error sending request for url (https://github.com/…/latest.json)");
  await page.goto("/");
  // No repository: the checks start from the welcome screen.
  await expect(page.getByRole("button", { name: "Open repository" }).first()).toBeVisible();
  const s = await about(page);
  const status = s.getByTestId("settings-update-status");
  await expect(status).toHaveText("Could not check for updates. GitHub could not be reached.", CHECKED);
  await expect(s.getByTestId("settings-update-detail")).toHaveText("error sending request for url (https://github.com/…/latest.json)");
  await expect(notices(page).locator(".toast")).toHaveCount(0);

  // The same version: up to date, never offered.
  gg.updates.offer("0.4.0");
  await s.getByTestId("settings-update-check").click();
  await expect(status).toHaveText(/^gg is up to date\. Checked at \d\d:\d\d:\d\d\.$/, CHECKED);
  await expect(s.getByTestId("settings-update-detail")).toHaveCount(0);

  // A download that fails its signature check installs nothing, and says so in Settings only.
  gg.updates.offer("0.9.0");
  gg.updates.download({ kind: "signature", detail: "the signature does not verify" });
  await s.getByTestId("settings-update-check").click();
  await expect(status).toHaveText(
    "Could not download gg 0.9.0. Its signature does not match gg’s, so nothing was installed.",
    CHECKED,
  );
  await expect(s.getByTestId("settings-update-restart")).toHaveCount(0);
  await expect(notices(page).locator(".toast")).toHaveCount(0);
});

test("a copy that cannot update itself says a version is available, why, and where to get it", async ({ gg }) => {
  const { page } = gg;
  gg.updates.running("0.4.0");
  gg.updates.offer("0.9.0");
  gg.updates.blocker("translocated");
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const available = notice(page, "gg 0.9.0 is available");
  await expect(available).toBeVisible(CHECKED);
  await expect(available).toContainText("Move gg to the Applications folder and open it from there.");
  const before = gg.launches().length;
  await available.getByRole("button", { name: "Download page" }).click();
  await expect.poll(() => gg.launches().slice(before).map((l) => l.at(-1))).toEqual(["https://gg.shakirul.com"]);

  const s = await about(page);
  await expect(s.getByTestId("settings-update-status")).toHaveText("gg 0.9.0 is available.");
  await expect(s.getByTestId("settings-update-reason")).toContainText("macOS runs this copy of gg from a temporary place");
  await expect(s.getByTestId("settings-update-restart")).toHaveCount(0);
  await s.getByTestId("settings-update-download").click();
  await expect.poll(() => gg.launches().length).toBe(before + 2);
  expect(gg.updates.restarts()).toEqual([]);
});

test("with automatic checks off nothing is asked at start; Check now still works", async ({ gg }) => {
  const { page } = gg;
  gg.updates.running("0.4.0");
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const s = await about(page);
  const auto = s.getByTestId("settings-check-updates");
  await expect(auto).toBeChecked();
  await expect(s.getByTestId("settings-update-status")).toHaveText(/^gg is up to date\./, CHECKED);
  // From the keyboard.
  await auto.focus();
  await page.keyboard.press("Space");
  await expect(auto).not.toBeChecked();
  await expect(s.getByRole("status").filter({ hasText: /^Saved$/ })).toBeVisible();
  await expect.poll(() => stored(gg).checkForUpdates).toBe(false);

  // The next start: nothing is checked, though a newer version is there.
  gg.updates.offer("0.9.0");
  await gg.relaunch();
  await expect(page.getByRole("tab", { name: "small" })).toHaveAttribute("aria-selected", "true");
  await page.waitForTimeout(6_000);
  await expect(notices(page).locator(".toast")).toHaveCount(0);
  const again = await about(page);
  await expect(again.getByTestId("settings-check-updates")).not.toBeChecked();
  await expect(again.getByTestId("settings-update-status")).toHaveText("Automatic checks are off.");
  await again.getByTestId("settings-update-check").click();
  await expect(again.getByTestId("settings-update-status")).toHaveText("gg 0.9.0 is ready. Restart to update.", CHECKED);
  await expect(notice(page, "gg 0.9.0 is ready")).toBeVisible();
});

test("turning automatic checks off stops the download they started; Check now is the user's own", async ({ gg }) => {
  const { page } = gg;
  gg.updates.running("0.4.0");
  gg.updates.offer("0.9.0");
  gg.updates.holdDownloads();
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const s = await about(page);
  const status = s.getByTestId("settings-update-status");
  await expect(status).toHaveText("Downloading gg 0.9.0… 50%", CHECKED);
  await s.getByTestId("settings-check-updates").uncheck();
  await expect(status).toHaveText("Automatic checks are off.");
  // The download it started does not come back to life.
  gg.updates.openDownloadGate();
  await page.waitForTimeout(1_000);
  await expect(status).toHaveText("Automatic checks are off.");
  await expect(notices(page).locator(".toast")).toHaveCount(0);
  await expect(s.getByTestId("settings-update-restart")).toHaveCount(0);
  // Check now, whatever the setting.
  await s.getByTestId("settings-update-check").click();
  await expect(status).toHaveText("gg 0.9.0 is ready. Restart to update.", CHECKED);
});
