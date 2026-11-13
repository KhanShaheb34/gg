// The commit details panel against real repositories: what it shows, moving between commits,
// closing it, and its height.
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
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
  await tree.getByRole("treeitem", { name: "app", exact: true }).click();
  await expect(fileRow(p, "app/routes/r.py")).toHaveCount(0);
  await tree.getByRole("treeitem", { name: "app", exact: true }).click();
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

// --- Fix round 1 ---------------------------------------------------------------------------

test("git text is isolated from its neighbours and controls in paths are shown", async ({ gg }) => {
  const { page } = gg;
  const RLO = "‮";
  const dir = makeRepo(
    "bidi",
    (git, write) => {
      git("init", "-q", "-b", "main");
      write(`evil${RLO}txt.exe`, "x\n");
      write("odd\nname.txt", "y\n");
      git("add", "-A");
      git("commit", "-q", "--author", `Mallory ${RLO}evil <m${RLO}@example.com>`, "-m", `Subject ${RLO}reversed`);
    },
    path.dirname(gg.repoPath("empty")),
  );
  await page.goto("/");
  await open(page, dir);
  await rowWith(page, `Subject ${RLO}reversed`).click();
  const p = panel(page);
  await expect(p.getByTestId("details-files-count")).toHaveText("2 files changed");

  /** Every text node with `needle` in it sits in its own bidi isolate inside `l`. */
  const isolated = (l: Locator, needle: string) =>
    l.evaluate((el, n) => {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      const found: boolean[] = [];
      for (let t = walker.nextNode(); t; t = walker.nextNode()) {
        if (!t.textContent!.includes(n)) continue;
        let e = t.parentElement;
        let ok = false;
        // An isolate inside the field, around this piece: the field itself does not count.
        while (e && e !== el) {
          if (getComputedStyle(e).unicodeBidi === "isolate") ok = true;
          e = e.parentElement;
        }
        found.push(ok);
      }
      return found.length > 0 && found.every(Boolean);
    }, needle);

  expect(await isolated(p.getByTestId("details-author"), "Mallory")).toBe(true);
  expect(await isolated(p.getByTestId("details-author"), "example.com")).toBe(true);
  expect(await isolated(p.getByTestId("details-committer"), "Ada Lovelace")).toBe(true);
  expect(await isolated(p.getByTestId("details-message"), "Subject")).toBe(true);

  // The override is shown by name, the newline as ␊; the tooltip keeps the exact path.
  const evil = fileRow(p, `evil${RLO}txt.exe`);
  await expect(evil.locator(".name")).toHaveText("evil‹U+202E›txt.exe");
  expect(await isolated(evil, "evil")).toBe(true);
  await expect(evil).toHaveAttribute("title", `Added: evil${RLO}txt.exe`);
  const odd = p.locator("[data-row]").filter({ hasText: "odd␊name.txt" });
  await expect(odd).toHaveAttribute("data-file", "odd\nname.txt");
  await p.getByRole("button", { name: "List" }).click();
  expect(await isolated(fileRow(p, `evil${RLO}txt.exe`), "evil")).toBe(true);
});

test("the list is in the same natural order as the tree", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Generate the module scaffolding").click();
  const p = panel(page);
  const order = (l: Locator) =>
    l.evaluateAll((rows) => rows.map((r) => r.getAttribute("data-file")).filter((f) => f?.startsWith("packages/module-01/")));
  await expect(fileRow(p, "packages/module-01/src/file-8.ts")).toBeVisible();
  const inTree = await order(p.getByRole("tree").locator("[data-file]"));
  await p.getByRole("button", { name: "List" }).click();
  await expect(fileRow(p, "packages/module-01/src/file-8.ts")).toBeVisible();
  const inList = await order(p.getByRole("list").locator("[data-file]"));
  expect(inList).toEqual(inTree);
  expect(inList.indexOf("packages/module-01/src/file-8.ts")).toBeLessThan(
    inList.indexOf("packages/module-01/src/file-16.ts"),
  );
});

test("the tree's folders line up with the header; counts line up down the list", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Rework peaches routes").click();
  const p = panel(page);
  const header = (await p.getByTestId("details-files-count").boundingBox())!;
  const chevron = (await p.locator('[role="treeitem"][aria-level="1"] .chevron').first().boundingBox())!;
  expect(Math.abs(chevron.x - header.x)).toBeLessThanOrEqual(1);
  // A file sits where its folder's name starts, one level in.
  const folderName = (await p.getByRole("treeitem", { name: "app", exact: true }).locator(".name").boundingBox())!;
  const peaches = (await p.getByRole("treeitem", { name: "peaches", exact: true }).locator(".chevron").boundingBox())!;
  expect(Math.abs(peaches.x - (chevron.x + 14))).toBeLessThanOrEqual(1);
  // Rows start at the tree's left edge; a file's name is 29 px + 14 px per level in.
  const edge = (await p.getByRole("tree", { name: "Changed files" }).boundingBox())!.x;
  const levelOneFile = (await p.locator('[role="treeitem"][aria-level="2"][data-file] .name').first().boundingBox())!;
  expect(Math.abs(levelOneFile.x - (edge + 29 + 14))).toBeLessThanOrEqual(1);
  expect(Math.abs(folderName.x - (edge + 29))).toBeLessThanOrEqual(1);
  expect(folderName.x).toBeGreaterThan(chevron.x);

  const bars = await p.locator("[data-file] .bar").evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().x)));
  expect(bars.length).toBeGreaterThan(1);
  expect(new Set(bars).size).toBe(1);
});

test("counts of a thousand lines and more still line up with the small ones", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  const dir = gg.repoPath("details");
  writeFileSync(path.join(dir, "big.txt"), Array.from({ length: 12_345 }, (_, i) => `line ${i}\n`).join(""));
  writeFileSync(path.join(dir, "small.txt"), "one\n");
  gg.git("details", "add", "-A");
  gg.git("details", "commit", "-q", "-m", "Add a big file");
  gg.git("details", "rm", "-q", "big.txt");
  writeFileSync(path.join(dir, "small.txt"), "one\ntwo\n");
  gg.git("details", "commit", "-q", "-am", "Remove the big file");
  await page.keyboard.press("ControlOrMeta+r");
  const p = panel(page);
  const bars = () =>
    p.locator("[data-file] .bar").evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().x)));
  for (const [subject, counts] of [
    ["Add a big file", "+12345 | −0"],
    ["Remove the big file", "+0 | −12345"],
  ]) {
    await rowWith(page, subject).click();
    await expect(fileRow(p, "big.txt").locator(".counts")).toHaveText(counts);
    expect(await bars()).toHaveLength(2);
    expect(new Set(await bars()).size, subject).toBe(1);
  }
});

test("the information column's fade leaves the column divider alone", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Generate the module scaffolding").click();
  const p = panel(page);
  const info = p.getByTestId("details-message").locator("..");
  await expect(info).toHaveClass(/more/);
  // The divider belongs to the files column, which is not faded.
  expect(await info.evaluate((el) => getComputedStyle(el).borderRightWidth)).toBe("0px");
  const files = p.getByTestId("details-files-count").locator("../..");
  expect(await files.evaluate((el) => getComputedStyle(el).borderLeftWidth)).toBe("1px");
  expect(await files.evaluate((el) => getComputedStyle(el).maskImage)).toBe("none");
});

test("tree items tell assistive technology where they are and what changed", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Rework peaches routes").click();
  const tree = panel(page).getByRole("tree", { name: "Changed files" });
  const b = tree.getByRole("treeitem", { name: "added, app/peaches/b.py, 2 added, 0 deleted", exact: true });
  await expect(b).toHaveAttribute("aria-level", "3");
  await expect(b).toHaveAttribute("aria-posinset", "1");
  await expect(b).toHaveAttribute("aria-setsize", "1");
  await expect(tree.getByRole("treeitem", { name: "assets", exact: true })).toHaveAttribute("aria-posinset", "2");
  await expect(tree.getByRole("treeitem", { name: "assets", exact: true })).toHaveAttribute("aria-setsize", "3");
  await expect(tree.getByRole("treeitem", { name: "added, assets/logo.bin, binary", exact: true })).toBeVisible();
  await expect(
    tree.getByRole("treeitem", { name: "renamed, notes/old-name.txt to docs/new-name.txt, 0 added, 0 deleted", exact: true }),
  ).toBeVisible();
});

test("a parent's hash in the merge note goes to it; failures to go are told", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Merge branch 'side'").click();
  const p = panel(page);
  await p.getByTestId("details-files-note").getByRole("button", { name: hashOf(gg, "Main work").slice(0, 8) }).click();
  await expect(selectedRow(page).locator(".subject")).toHaveText("Main work");

  // The core cannot look the commit up: said so, not taken for "not shown".
  await page.route("**/invoke", async (route) => {
    const body = route.request().postDataJSON() as { cmd?: string } | null;
    if (body?.cmd !== "find_row") return route.continue();
    await route.fulfill({ json: { err: { code: "git_failed", message: "index is locked", detail: null } } });
  });
  await panel(page).getByTestId("details-parents").getByRole("button").first().click();
  await expect(page.getByText(/Could not go to that commit: index is locked/)).toBeVisible();
  await expect(page.getByText("That commit is not shown in the graph")).toHaveCount(0);

  await page.unroute("**/invoke");
  await page.route("**/invoke", async (route) => {
    const body = route.request().postDataJSON() as { cmd?: string } | null;
    if (body?.cmd !== "find_row") return route.continue();
    await route.fulfill({ json: { ok: null } });
  });
  await panel(page).getByTestId("details-parents").getByRole("button").first().click();
  await expect(page.getByText("That commit is not shown in the graph")).toBeVisible();
});

test("when the selected commit goes away with the focus in the panel, the focus goes to the graph", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Generate the module scaffolding").click();
  await panel(page).getByRole("button", { name: "List" }).focus();
  gg.git("details", "reset", "-q", "--hard", "HEAD~1");
  await page.keyboard.press("ControlOrMeta+r");
  await expect(panel(page)).toHaveCount(0);
  await expect(grid(page)).toBeFocused();
});

test("the panel's height fits the window: a stored height too tall, and a smaller window", async ({ gg }) => {
  const { page } = gg;
  await page.addInitScript(() => localStorage.setItem("gg.details.v1", JSON.stringify({ height: 5000, files: "tree" })));
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Main work").click();
  const sizes = () =>
    page.evaluate(() => {
      const panel = document.querySelector<HTMLElement>('[aria-label="Commit details"]')!;
      const body = panel.parentElement!;
      const sep = panel.querySelector('[role="separator"]')!;
      return {
        panel: Math.round(panel.getBoundingClientRect().height),
        body: Math.round(body.getBoundingClientRect().height),
        valuenow: Number(sep.getAttribute("aria-valuenow")),
      };
    });
  let s = await sizes();
  expect(s.panel).toBe(s.body - 160);
  expect(s.valuenow).toBe(s.panel);

  await page.setViewportSize({ width: 1280, height: 560 });
  await expect
    .poll(async () => {
      const now = await sizes();
      return [now.panel, now.valuenow];
    })
    .toEqual([s.body - 160 - 160, s.body - 160 - 160]);
});

test("resizing the panel keeps the graph's rows (no new row elements) and its top edge leaves the last row clickable", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("big"));
  await expect(grid(page).locator('[data-index="0"] .subject')).not.toBeEmpty();
  await grid(page).locator('[data-index="1"]').click();
  await expect(panel(page)).toBeVisible();
  await grid(page).evaluate((el) => el.querySelectorAll('[role="row"]').forEach((r) => ((r as HTMLElement).dataset.mark = "1")));
  const marked = () => grid(page).evaluate((el) => el.querySelectorAll('[role="row"][data-mark]').length);
  const before = await marked();

  const splitter = page.getByRole("separator", { name: "Resize details panel" });
  const box = (await splitter.boundingBox())!;
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  for (const dy of [-40, -120, -60, 80, 200]) await page.mouse.move(x, y + dy, { steps: 6 });
  await page.mouse.up();
  expect(await marked()).toBe(before);

  // The strip overlaps the graph by 2 px at most.
  const strip = (await splitter.boundingBox())!;
  const top = (await panel(page).boundingBox())!.y;
  expect(top - strip.y).toBeLessThanOrEqual(2);
});

test("the column divider stays put from commit to commit, short message or long", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  // The scaffolding commit has a long message; the merge and "Main work" short ones.
  await rowWith(page, "Generate the module scaffolding").click();
  const p = panel(page);
  const divider = async () => (await p.getByTestId("details-files-count").locator("../..").boundingBox())!.x;
  const at = await divider();
  for (const subject of ["Merge branch 'side'", "Main work"]) {
    await page.keyboard.press("ArrowDown");
    await expect(p.getByTestId("details-message")).toContainText(subject);
    expect(await divider()).toBe(at);
  }
  // 44 % of the panel by default.
  const width = (await p.boundingBox())!.width;
  expect(Math.abs(at - (await p.boundingBox())!.x - Math.round(width * 0.44))).toBeLessThanOrEqual(1);
});

test("the column divider can be dragged, keeps both minimums, is remembered and resets", async ({ gg }) => {
  const { page } = gg;
  await page.goto("/");
  await open(page, gg.repoPath("details"));
  await rowWith(page, "Main work").click();
  const p = panel(page);
  const handle = p.getByRole("separator", { name: "Resize details columns" });
  const infoWidth = async () => (await p.getByTestId("details-message").locator("..").boundingBox())!.width;
  const drag = async (dx: number) => {
    const box = (await handle.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + dx, box.y + box.height / 2, { steps: 4 });
    await page.mouse.up();
  };
  const before = await infoWidth();
  await drag(-100);
  await expect.poll(infoWidth).toBeCloseTo(before - 100, 0);
  await drag(-2000);
  await expect.poll(infoWidth).toBeCloseTo(320, 0);
  await drag(4000);
  const panelW = (await p.boundingBox())!.width;
  await expect.poll(infoWidth).toBeCloseTo(panelW - 280, 0);
  await drag(-300);
  const chosen = await infoWidth();
  await page.reload();
  await rowWith(page, "Main work").click();
  await expect.poll(infoWidth).toBeCloseTo(chosen, 0);
  await handle.dblclick();
  await expect.poll(infoWidth).toBeCloseTo(Math.round(panelW * 0.44), 0);
});
