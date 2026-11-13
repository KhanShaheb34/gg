// The commit details panel against real repositories: what it shows, moving between commits,
// closing it, and its height.
import { execFileSync } from "node:child_process";
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { expect, makeRepo, test } from "./fixtures";

test.use({ timezoneId: "UTC" });

async function open(page: Page, dir: string) {
  page.once("dialog", (dialog) => dialog.accept(dir));
  await page.getByRole("button", { name: "Open repository" }).first().click();
  await expect(page.getByRole("tab", { name: path.basename(dir) })).toHaveAttribute("aria-selected", "true");
}

const grid = (page: Page) => page.getByRole("grid", { name: "Commits" });
const rowWith = (page: Page, subject: string) =>
  grid(page)
    .locator('[role="row"][data-index]')
    .filter({ has: page.locator(".subject", { hasText: new RegExp(`^${subject.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`) }) });
const selectedRow = (page: Page) => grid(page).locator('[role="row"][aria-selected="true"]');
const panel = (page: Page) => page.getByRole("region", { name: "Commit details" });
const fileRow = (p: Locator, filePath: string) => p.locator(`[data-file="${filePath}"]`);

type Git = { git(name: string, ...args: string[]): string };
/** Hash of the commit whose subject is `subject`. */
const hashOf = (gg: Git, subject: string) =>
  gg.git("details", "log", "--all", "--format=%H", "--fixed-strings", `--grep=${subject}`).trim().split("\n")[0];

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
function utcDate(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
}

test("shows the commit: full hash, parents, author, date and the whole message", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await expect(panel(page)).toHaveCount(0);

  const hash = hashOf(gg, "Rework peaches routes");
  const [parent, at, message] = gg.git("details", "show", "-s", "--format=%P%x00%at%x00%B", hash).split("\0");
  await rowWith(page, "Rework peaches routes").click();

  const p = panel(page);
  await expect(p).toBeVisible();
  await expect(p.getByTestId("details-hash")).toHaveText(hash);
  await expect(p.getByTestId("details-parents").getByRole("button")).toHaveText([parent.slice(0, 8)]);
  await expect(p.getByTestId("details-author")).toHaveText("Ada Lovelace <ada@example.com>");
  await expect(p.getByTestId("details-committer")).toHaveCount(0); // same as the author
  await expect(p.getByTestId("details-date")).toContainText(utcDate(Number(at)));
  await expect(p.getByTestId("details-message")).toHaveText(message.trimEnd());
  // Whitespace inside the message is kept.
  expect(await p.getByTestId("details-message").innerText()).toContain("    Indented line keeps   its spacing.");

  // Content is selectable, chrome is not.
  const userSelect = (l: Locator) => l.evaluate((el) => getComputedStyle(el).userSelect);
  expect(await userSelect(p.getByTestId("details-message"))).toBe("text");
  expect(await userSelect(p.getByTestId("details-hash"))).toBe("text");
  expect(await userSelect(fileRow(p, "app/routes/r.py").locator(".name"))).toBe("text");
  expect(await userSelect(p.getByText("Author", { exact: true }))).toBe("none");
});

test("lists the changed files with status, counts, renames and binaries, as a tree or a list", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Rework peaches routes").click();
  const p = panel(page);

  await expect(p.getByTestId("details-files-count")).toHaveText("4 files changed");
  const expected = [
    { path: "app/peaches/b.py", status: "A", counts: "+2 | −0" },
    { path: "app/routes/r.py", status: "M", counts: "+2 | −1" },
    { path: "assets/logo.bin", status: "A", counts: "binary" },
    { path: "docs/new-name.txt", status: "R", counts: "+0 | −0" },
  ];
  for (const f of expected) {
    await expect(fileRow(p, f.path)).toHaveAttribute("data-status", f.status);
    await expect(fileRow(p, f.path).locator(".counts")).toHaveText(f.counts);
  }

  // Tree: folders first, single-folder chains collapsed, a rename shows where it came from.
  const tree = p.getByRole("tree", { name: "Changed files" });
  await expect(tree.locator("[data-row]")).toHaveText([
    /^app$/,
    /^peaches/,
    /^b\.py/,
    /^routes/,
    /^r\.py/,
    /^assets/,
    /^logo\.bin/,
    /^docs/,
    /^notes\/old-name\.txt → new-name\.txt/,
  ]);

  // Collapsing a folder hides what is in it.
  await tree.getByRole("treeitem", { name: "app" }).click();
  await expect(fileRow(p, "app/routes/r.py")).toHaveCount(0);
  await tree.getByRole("treeitem", { name: "app" }).click();
  await expect(fileRow(p, "app/routes/r.py")).toBeVisible();

  // A flat list, remembered across commits and reloads.
  await p.getByRole("button", { name: "List" }).click();
  await expect(p.getByRole("button", { name: "List" })).toHaveAttribute("aria-pressed", "true");
  const list = p.getByRole("list", { name: "Changed files" });
  await expect(list.locator("[data-row]")).toHaveText([
    /^app\/peaches\/b\.py/,
    /^app\/routes\/r\.py/,
    /^assets\/logo\.bin/,
    /^notes\/old-name\.txt → docs\/new-name\.txt/,
  ]);
  await page.reload();
  await rowWith(page, "Rework peaches routes").click();
  await expect(panel(page).getByRole("list", { name: "Changed files" })).toBeVisible();
});

test("a merge commit lists its changes against the first parent", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  const merge = hashOf(gg, "Merge branch 'side'");
  const parents = gg.git("details", "show", "-s", "--format=%P", merge).trim().split(" ");
  await rowWith(page, "Merge branch 'side'").click();

  const p = panel(page);
  await expect(p.getByTestId("details-parents").getByRole("button")).toHaveText(parents.map((h) => h.slice(0, 8)));
  await expect(p.getByTestId("details-files-count")).toHaveText("2 files changed");
  await expect(fileRow(p, "side/feature.txt")).toHaveAttribute("data-status", "A");
  await expect(fileRow(p, "side/more.txt")).toHaveAttribute("data-status", "A");
  await expect(fileRow(p, "main.txt")).toHaveCount(0);
  await expect(p.getByTestId("details-files-note")).toHaveText(`Compared with the first parent ${parents[0].slice(0, 8)}`);
});

test("clicking a parent selects that commit and scrolls to it", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Rework peaches routes").click();
  const parent = hashOf(gg, "Initial import");

  await panel(page).getByTestId("details-parents").getByRole("button", { name: parent.slice(0, 8) }).click();
  await expect(selectedRow(page).locator(".subject")).toHaveText("Initial import");
  await expect(panel(page).getByTestId("details-hash")).toHaveText(parent);
  await expect(panel(page).getByTestId("details-parents")).toHaveText(/root commit/);
});

test("the hash is copied when clicked", async ({ gg, context }) => {
  const { page } = gg;
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Main work").click();
  await panel(page).getByTestId("details-hash").click();
  await expect(page.getByText("Copied", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(hashOf(gg, "Main work"));
});

test("clicking through rows quickly ends on the last one clicked, however late the others answer", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("small"));
  const hashes = gg.git("small", "log", "--all", "--date-order", "--format=%H%x09%s").trim().split("\n").slice(2, 7);
  const [lastHash, lastSubject] = hashes[4].split("\t");

  // Every answer but the last one's comes back late.
  await page.route("**/invoke", async (route) => {
    const body = route.request().postDataJSON() as { cmd?: string; args?: { hash?: string } } | null;
    if (body?.cmd === "commit_details" && body.args?.hash !== lastHash) await new Promise((r) => setTimeout(r, 800));
    await route.continue();
  });
  for (const line of hashes) await rowWith(page, line.split("\t")[1]).click();

  const p = panel(page);
  await expect(p.getByTestId("details-hash")).toHaveText(lastHash);
  await expect(p.getByTestId("details-message")).toHaveText(lastSubject);
  await expect(p.getByTestId("details-files-count")).toHaveText("1 file changed");
  await page.waitForTimeout(1200); // the late answers arrive
  await expect(p.getByTestId("details-hash")).toHaveText(lastHash);
  await expect(p.getByTestId("details-message")).toHaveText(lastSubject);
});

test("Escape or the close button closes the panel and clears the selection", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));

  await rowWith(page, "Main work").click();
  await expect(panel(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(panel(page)).toHaveCount(0);
  await expect(selectedRow(page)).toHaveCount(0);

  // From inside the panel too; the focus goes back to the graph.
  await rowWith(page, "Main work").click();
  await panel(page).getByRole("button", { name: "List" }).focus();
  await page.keyboard.press("Escape");
  await expect(panel(page)).toHaveCount(0);
  await expect(grid(page)).toBeFocused();

  await rowWith(page, "Main work").click();
  await panel(page).getByRole("button", { name: "Close details" }).click();
  await expect(panel(page)).toHaveCount(0);
  await expect(selectedRow(page)).toHaveCount(0);
});

test("the uncommitted changes row opens no panel", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  const dir = gg.repoPath("details");
  await open(page, dir);
  gg.git("details", "rm", "-q", "main.txt");
  await page.keyboard.press("ControlOrMeta+r");
  const uncommitted = grid(page).locator('[role="row"][data-index="0"]');
  await expect(uncommitted.locator(".subject")).toHaveText(/^Uncommitted changes/);
  await uncommitted.click();
  await expect(uncommitted).toHaveAttribute("aria-selected", "true");
  await expect(panel(page)).toHaveCount(0);
});

test("the panel's height can be changed and survives a reload", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Main work").click();
  const height = async () => (await panel(page).boundingBox())!.height;
  await expect.poll(height).toBe(280);

  const splitter = page.getByRole("separator", { name: "Resize details panel" });
  const box = (await splitter.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 - 60, { steps: 4 });
  await page.mouse.up();
  await expect.poll(height).toBe(340);
  await splitter.focus();
  await page.keyboard.press("ArrowDown");
  await expect.poll(height).toBe(332);

  await page.reload();
  await rowWith(page, "Main work").click();
  await expect.poll(height).toBe(332);
});

test("opening the panel keeps the selected row in view without moving the others", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
  const scrollTop = () => grid(page).evaluate((el) => el.scrollTop);

  // Near the top: nothing moves.
  await grid(page).locator('[data-index="2"]').click();
  await expect(panel(page)).toBeVisible();
  expect(await scrollTop()).toBe(0);
  await page.keyboard.press("Escape");
  await expect(panel(page)).toHaveCount(0);

  // The last row in view would end up under the panel: the graph scrolls just enough.
  const last = await grid(page).evaluate((el) => {
    const bottom = el.getBoundingClientRect().bottom;
    const rows = [...el.querySelectorAll<HTMLElement>('[role="row"][data-index]')].filter(
      (r) => r.getBoundingClientRect().bottom <= bottom,
    );
    return Math.max(...rows.map((r) => Number(r.dataset.index)));
  });
  const target = grid(page).locator(`[data-index="${last}"]`);
  await target.click();
  await expect(panel(page)).toBeVisible();
  await expect
    .poll(async () => {
      const g = await grid(page).boundingBox();
      const r = await target.boundingBox();
      return g && r ? Math.round(g.y + g.height - (r.y + r.height)) : `row ${last} not shown, scrollTop ${await scrollTop()}`;
    })
    .toBe(0);
  await expect(target).toHaveAttribute("aria-selected", "true");
});

test("a commit with ten thousand files shows at once and draws only the rows in view", async ({ gg }) => {
  const { page } = gg;
  const COUNT = 10_000;
  const dir = makeRepo(
    "huge",
    (git, _write, cwd) => {
      git("init", "-q", "-b", "main");
      const who = "Ada Lovelace <ada@example.com> 1767603600 +0000";
      const parts = ["commit refs/heads/main\n", `author ${who}\n`, `committer ${who}\n`, "data 13\nHuge commit\n\n"];
      for (let i = 0; i < COUNT; i++) parts.push(`M 644 inline src/m${i % 50}/f${i}.txt\ndata 2\n${i % 10}\n\n`);
      execFileSync("git", ["fast-import", "--quiet"], { cwd, input: parts.join(""), stdio: ["pipe", "ignore", "pipe"] });
      git("reset", "-q", "--hard");
    },
    path.dirname(gg.repoPath("empty")),
  );
  await page.goto("/");
  await open(page, dir);
  await rowWith(page, "Huge commit").click();

  const p = panel(page);
  // The row's own information first, the files when they come.
  await expect(p.getByTestId("details-message")).toHaveText("Huge commit");
  await expect(p.getByTestId("details-files-count")).toHaveText("10,000 files changed", { timeout: 15_000 });
  const tree = p.getByRole("tree", { name: "Changed files" });
  expect(await tree.locator("[data-row]").count()).toBeLessThan(100);

  // Scrolled to the end, the last file is there; the page kept answering throughout.
  await tree.evaluate((el) => (el.scrollTop = el.scrollHeight));
  await expect(fileRow(p, "src/m49/f9999.txt")).toBeVisible();
  await p.getByRole("button", { name: "List" }).click();
  await expect(p.getByRole("list", { name: "Changed files" }).locator("[data-row]").first()).toBeVisible();
  expect(await p.getByRole("list", { name: "Changed files" }).locator("[data-row]").count()).toBeLessThan(100);
});
