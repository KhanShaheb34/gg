// The Pull requests page's rows and its pull request's page, as they behave: a selection that
// shows only while the list has the focus, back and forward, the row's Open in browser and pin,
// the state each icon says (colour and words), headings laid out evenly, a merged pull request's
// page, the files when the details cannot be read, rows grouped by repository. Against the real
// core and the devserver's fake gh (never the real one, never the network).
import type { Page } from "@playwright/test";
import { expect, test, type GG } from "./fixtures";
import { MIX, replyFiles, replyInbox, replyDetails, type Node } from "./inboxData";
import { answer, threads, view } from "./prDetailsData";

test.use({ viewport: { width: 1180, height: 820 }, ggDevserverArgs: ["--pr-details-every-ms", "0"] });

const DAY = 24 * 60;
const button = (page: Page) => page.getByTestId("inbox-button");
const row = (page: Page, key: string) => page.locator(`[data-testid="inbox-pr"][data-key="${key}"]`);
const list = (page: Page) => page.getByRole("listbox", { name: "Pull requests" });
const selected = (page: Page) => page.locator('[data-testid="inbox-pr"][aria-selected="true"]');

async function openPage(gg: GG) {
  await gg.page.goto("/");
  await expect(gg.page.getByTestId("welcome")).toBeVisible();
  await button(gg.page).click();
  await expect(gg.page.getByTestId("inbox")).toBeVisible();
}

test("the selection shows only while the list has the focus: Escape or a click on no row puts it away, the keys go on from it", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  await openPage(gg);
  // Focus selects the first; ↓ is the second.
  await list(page).focus();
  await page.keyboard.press("ArrowDown");
  await expect(selected(page)).toHaveAttribute("data-key", "github.com/octo/cat#29");
  // A click on the page's header (no row): gone.
  await page.locator("#inbox-title").click();
  await expect(selected(page)).toHaveCount(0);
  await expect(list(page)).not.toHaveAttribute("aria-activedescendant");
  // Back in the list, the keys go on from where it was.
  await list(page).focus();
  await page.keyboard.press("ArrowDown");
  await expect(selected(page)).toHaveAttribute("data-key", "github.com/hub/dog#42");
  // Escape: gone, the focus kept; ↓ goes on.
  await page.keyboard.press("Escape");
  await expect(selected(page)).toHaveCount(0);
  await expect(list(page)).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(selected(page)).toHaveAttribute("data-key", "github.com/hub/owl#7");
});

test("back and forward: ⌘[ and ⌘← to the list, ⌘] and ⌘→ to the page just left; not in a reply box; the back button an icon", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  answer(gg, 42, view(42, { comments: [], reviews: [] }), threads([{ id: "PRRT_x", path: "a.ts", line: 1, comments: [{ id: "c1", login: "ada", body: "Why?", minutesAgo: 5 }] }]));
  await openPage(gg);
  await row(page, "github.com/hub/dog#42").click();
  const back = page.getByTestId("inbox-back");
  await expect(back).toHaveText("");
  await expect(back).toHaveAttribute("aria-label", "Back to pull requests");
  await expect(back).toHaveAttribute("title", /^Back to pull requests \(Esc, (⌘\[ or ⌘←|Ctrl\+\[ or Ctrl\+←)\)$/);
  const prPage = page.getByTestId("inbox-pr-page");
  await page.keyboard.press("ControlOrMeta+BracketLeft");
  await expect(prPage).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+BracketRight");
  await expect(page.getByTestId("pr-view-title")).toHaveText("Speed up the search index");
  await page.keyboard.press("ControlOrMeta+ArrowLeft");
  await expect(prPage).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+ArrowRight");
  await expect(prPage).toBeVisible();
  // In the reply box ⌘← moves the caret: the page stays.
  const field = page.getByTestId("pr-reply-box-field");
  await field.click();
  await field.fill("Because");
  await page.keyboard.press("ControlOrMeta+ArrowLeft");
  await expect(prPage).toBeVisible();
  // Another pull request opened from the list: nothing to go forward to after.
  await back.click();
  await page.keyboard.press("ControlOrMeta+ArrowRight");
  await expect(prPage).toBeVisible();
});

test("a row's Open in browser (beside its pin, its room kept), its menu and O open its page, nothing else", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, MIX);
  await openPage(gg);
  const r = row(page, "github.com/hub/dog#42");
  const open = r.getByTestId("inbox-open-browser");
  const pin = r.getByTestId("inbox-pin");
  await expect(open).toHaveAttribute("aria-label", "Open in browser");
  // Hidden but there: the review chip sits where it will be on hover.
  const chipBefore = (await r.getByTestId("inbox-review").boundingBox())!;
  await r.hover();
  await expect(open).toHaveCSS("opacity", "1");
  expect((await r.getByTestId("inbox-review").boundingBox())!.x).toBe(chipBefore.x);
  // Left of the pin.
  expect((await open.boundingBox())!.x).toBeLessThan((await pin.boundingBox())!.x);
  await open.click();
  await expect.poll(() => gg.launches().filter((l) => l.includes("https://github.com/hub/dog/pull/42")).length).toBe(1);
  await expect(page.getByTestId("inbox-pr-page")).toHaveCount(0);
  await expect(selected(page)).toHaveCount(0);
  // The menu, and O on the one selected.
  await r.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Open in browser" }).click();
  await expect.poll(() => gg.launches().filter((l) => l.includes("https://github.com/hub/dog/pull/42")).length).toBe(2);
  await list(page).focus();
  await page.keyboard.press("Home");
  await page.keyboard.press("o");
  await expect.poll(() => gg.launches().filter((l) => l.includes("https://github.com/octo/cat/pull/31")).length).toBe(1);
  await expect(page.getByTestId("inbox-pr-page")).toHaveCount(0);
  await expect(selected(page)).toHaveAttribute("data-key", "github.com/octo/cat#31");
});

test("each row's icon says its state by colour and words; by status, the same rules; by repository, rows leave out the repository", async ({ gg }) => {
  const { page } = gg;
  const at = { minutesAgo: 30 };
  const nodes: Node[] = [
    { number: 1, title: "Failing", checks: "FAILURE", review: null, ...at },
    { number: 2, title: "Conflict", checks: "SUCCESS", review: null, conflict: true, ...at },
    { number: 3, title: "Changes", checks: "SUCCESS", review: "CHANGES_REQUESTED", ...at },
    { number: 4, title: "Running", checks: "PENDING", review: "APPROVED", ...at },
    { number: 5, title: "Passing", checks: "SUCCESS", review: "REVIEW_REQUIRED", ...at },
    { number: 6, title: "Nothing", checks: null, review: null, ...at },
    { number: 7, title: "Draft", checks: "FAILURE", review: null, draft: true, ...at },
  ];
  replyInbox(gg, { review: nodes });
  replyDetails(gg, nodes);
  await openPage(gg);
  const want: [number, string, string][] = [
    [1, "danger", "Checks failing"],
    [2, "danger", "Merge conflict"],
    [3, "danger", "Changes requested"],
    [4, "warning", "Checks running"],
    [5, "success", "Checks passing"],
    [6, "neutral", "Open · no checks"],
    [7, "muted", "Draft · checks failing"],
  ];
  for (const [n, tone, label] of want) {
    const icon = row(page, `github.com/octo/cat#${n}`).getByTestId("inbox-state-icon");
    await expect(icon).toHaveAttribute("data-tone", tone);
    await expect(icon).toHaveAttribute("title", label);
  }
  // The unread dot shows on every colour.
  await page.getByTestId("inbox-group-by").getByRole("radio", { name: "Status" }).click();
  await expect(page.getByTestId("inbox-group")).toHaveText([/Needs attention\s*3/, /Checks running\s*1/, /Checks passing\s*1/, /No checks\s*1/, /Drafts\s*1/]);
  await page.getByTestId("inbox-group-by").getByRole("radio", { name: "Repository" }).click();
  await expect(page.getByTestId("inbox-group").first()).toContainText("octo/cat");
  await expect(row(page, "github.com/octo/cat#1").locator(".repo")).toHaveCount(0);
  await expect(row(page, "github.com/octo/cat#1").locator(".number")).toHaveText("#1");
});

test("a folding heading's highlight is centred on its words, as a plain heading's are laid out", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, { mine: [{ number: 1, minutesAgo: 60 }, { number: 2, minutesAgo: 45 * DAY }] });
  await openPage(gg);
  const fold = page.locator('[data-testid="inbox-group"][data-group="older"]');
  await fold.hover();
  for (const head of [fold, page.locator('[data-testid="inbox-group"][data-group="mine"]')]) {
    const box = (await head.boundingBox())!;
    const words = (await head.locator("h2").boundingBox())!;
    expect(Math.abs(words.y + words.height / 2 - (box.y + box.height / 2))).toBeLessThanOrEqual(1);
  }
  const chevron = (await fold.locator(".chevron").boundingBox())!;
  const box = (await fold.boundingBox())!;
  expect(Math.abs(chevron.y + chevron.height / 2 - (box.y + box.height / 2))).toBeLessThanOrEqual(1.5);
  // Its words where a plain heading's are.
  const plain = (await page.locator('[data-testid="inbox-group"][data-group="mine"] h2').boundingBox())!;
  expect(Math.abs((await fold.locator("h2").boundingBox())!.x - plain.x)).toBeLessThanOrEqual(1);
});

test("a pull request's page: the pin an icon, a merged pinned one says Merged, the files shown when the details cannot be read", async ({ gg }) => {
  const { page } = gg;
  replyInbox(gg, { review: [{ number: 3, repo: "hub/dog" }, { number: 4, repo: "hub/dog" }] });
  gg.gh.reply("pr-view-3", 1, "", "HTTP 502: Bad gateway");
  replyFiles(gg, 3, [[{ filename: "a.ts", status: "modified", additions: 1, deletions: 0, changes: 1, patch: "@@" }]]);
  answer(gg, 4, view(4, { state: "MERGED", comments: [], reviews: [] }));
  replyFiles(gg, 4, [[]]);
  await openPage(gg);
  await row(page, "github.com/hub/dog#3").click();
  await expect(page.getByTestId("pr-details-failed")).toBeVisible();
  await expect(page.getByTestId("pr-file")).toHaveCount(1);
  const pin = page.getByTestId("inbox-pr-pin");
  await expect(pin).toHaveText("");
  await expect(pin).toHaveAttribute("aria-label", "Pin to top");
  await expect(pin).toHaveAttribute("aria-pressed", "false");
  await pin.click();
  await expect(pin).toHaveAttribute("aria-pressed", "true");
  await expect(pin).toHaveAttribute("aria-label", "Unpin");
  await page.getByTestId("inbox-back").click();
  await row(page, "github.com/hub/dog#4").click();
  await expect(page.getByTestId("pr-state")).toHaveAttribute("data-state", "merged");
  await expect(page.getByTestId("pr-state")).toHaveText("Merged");
});

test("a pull request's page whose details give a state gg does not know keeps the list's: closed stays Closed", async ({ gg }) => {
  const { page } = gg;
  const shut: Node = { number: 5, repo: "hub/dog", state: "CLOSED" };
  replyInbox(gg, { review: [shut] });
  replyDetails(gg, [shut]);
  answer(gg, 5, view(5, { state: "SOMETHING_NEW", comments: [], reviews: [] }));
  replyFiles(gg, 5, [[]]);
  await openPage(gg);
  await expect(row(page, "github.com/hub/dog#5").locator("[data-state]")).toHaveAttribute("data-state", "closed");
  await row(page, "github.com/hub/dog#5").click();
  await expect(page.getByTestId("pr-merge-status")).toBeVisible();
  await expect(page.getByTestId("pr-state")).toHaveAttribute("data-state", "closed");
  await expect(page.getByTestId("pr-state")).toHaveText("Closed");
});
