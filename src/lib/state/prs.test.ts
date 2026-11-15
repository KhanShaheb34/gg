import { beforeEach, describe, expect, it, vi } from "vitest";
import { pullRequest, snap } from "../menus/menuTestData";
import type { PrState } from "../types";

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn(() => () => {}),
    api: {
      prSuggest: vi.fn(),
      openExternal: vi.fn(async () => ({ note: null })),
      saveSession: vi.fn(async () => {}),
    },
  };
});
vi.mock("./overlays.svelte", () => ({ openDialog: vi.fn(), closeDialog: vi.fn(), showDialog: vi.fn() }));
vi.mock("../components/dialogs/CreatePullRequestDialog.svelte", () => ({ default: { name: "CreatePullRequestDialog" } }));

import { api } from "../ipc";
import { app } from "./app.svelte";
import { showDialog } from "./overlays.svelte";
import { openComparePage, openCreatePullRequest, openPullRequest, prTooltip, staleSince, STALE_AFTER_MS } from "./prs.svelte";

const MIN = 60_000;
const ready = (loadedAtMs: number, reason: PrState["reason"] = null): PrState => ({ status: "ready", reason, loadedAtMs, partial: false });

beforeEach(() => {
  vi.mocked(api.openExternal).mockClear();
  vi.mocked(api.prSuggest).mockReset();
  app.toasts = [];
});

describe("staleSince", () => {
  const at = new Date(2026, 10, 16, 14, 5).getTime();

  it("fresh markers say nothing; older than 10 minutes, or a failed reload, say as of when", () => {
    expect(staleSince(ready(at), at + 5 * MIN)).toBeNull();
    expect(staleSince(ready(at), at + STALE_AFTER_MS)).toBeNull();
    expect(staleSince(ready(at), at + STALE_AFTER_MS + 1)).toBe("as of 14:05");
    expect(staleSince(ready(at, "failed"), at + MIN)).toBe("as of 14:05");
  });

  it("past a day, how long ago too", () => {
    expect(staleSince(ready(at), at + 3 * 24 * 60 * MIN)).toBe("as of 14:05, 3 days ago");
  });

  it("no list, no markers: nothing to say", () => {
    for (const status of ["unknown", "loading", "unavailable"] as const) {
      expect(staleSince({ status, reason: null, loadedAtMs: null, partial: false }, at)).toBeNull();
    }
  });
});

describe("prTooltip", () => {
  it("the title, base ← head, and what else is so: draft, as of when", () => {
    expect(prTooltip(pullRequest(), null)).toBe("#12 Add the parser\nmain ← feature/x");
    expect(prTooltip(pullRequest({ state: "draft" }), "as of 14:05")).toBe("#12 Add the parser\nmain ← feature/x\nDraft · as of 14:05");
  });
});

describe("opening", () => {
  it("a pull request opens at the address the core gave, through open_external", () => {
    openPullRequest(1, pullRequest());
    expect(api.openExternal).toHaveBeenCalledWith(1, { target: "url", url: "https://github.com/octo/cat/pull/12" });
  });

  it("the compare page is asked of the core only when chosen, and opened as it gave it", async () => {
    const url = "https://github.com/octo/cat/compare/main...feature/x?expand=1";
    vi.mocked(api.prSuggest).mockResolvedValue({ title: "t", body: "", base: "main", baseRemote: "origin", existing: null, compareUrl: url });
    await openComparePage(1, "feature/x", snap());
    expect(api.prSuggest).toHaveBeenCalledWith(1, "feature/x");
    expect(api.openExternal).toHaveBeenCalledWith(1, { target: "url", url });
  });

  it("no compare page: says why, opens nothing", async () => {
    vi.mocked(api.prSuggest).mockResolvedValue({ title: "t", body: "", base: "main", baseRemote: "origin", existing: null, compareUrl: null });
    const signedOut = snap({ prs: { status: "unavailable", reason: "signed_out", loadedAtMs: null, partial: false } });
    await openComparePage(1, "feature/x", signedOut);
    expect(api.openExternal).not.toHaveBeenCalled();
    expect(app.toasts).toMatchObject([{ kind: "info", message: "gh is not signed in — run `gh auth login` in a terminal." }]);
  });

  it("Create pull request… opens the dialog for the branch named when the menu opened", () => {
    openCreatePullRequest(snap({ repoId: 4 }), "feature/x");
    expect(showDialog).toHaveBeenCalledWith({ name: "CreatePullRequestDialog" }, { repoId: 4, branch: "feature/x", homeDir: null });
  });
});
