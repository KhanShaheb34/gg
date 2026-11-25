import { beforeEach, describe, expect, it, vi } from "vitest";
import { snap, worktree } from "../menus/menuTestData";
import type { RepoSnapshot } from "../types";

vi.mock("../ipc", () => ({
  api: {
    getRepoPrefs: vi.fn(async () => ({ sidePanel: true })),
    setRepoPrefs: vi.fn(async () => {}),
  },
}));
// The views the panel helpers find, by repository.
const views = vi.hoisted(() => new Map<number, unknown>());
vi.mock("./repo.svelte", () => ({ findRepoView: (repoId: number) => views.get(repoId) }));
vi.mock("./app.svelte", () => ({ app: { homeDir: "/home/ada" }, reportError: vi.fn(), toast: vi.fn() }));
vi.mock("./overlays.svelte", () => ({ openMenu: vi.fn(), showDialog: vi.fn() }));
vi.mock("./ops.svelte", () => ({ runOp: vi.fn() }));
vi.mock("./editor", () => ({ openExternal: vi.fn() }));
vi.mock("../menus/dispatch", () => ({ pickPullRequest: vi.fn() }));
vi.mock("../components/dialogs/CreateWorktreeDialog.svelte", () => ({ default: {} }));
vi.mock("../components/dialogs/DeleteWorktreeDialog.svelte", () => ({ default: {} }));
vi.mock("../components/dialogs/PushDialog.svelte", () => ({ default: {} }));

import { api } from "../ipc";
import { PanelState, SIDE_TABS, tabForKey } from "./panelState.svelte";
import type { RepoView } from "./repo.svelte";
import {
  closeCommit,
  commitPath,
  holdCommit,
  committingIn,
  openCommit,
  panelShown,
  panelState,
  setPanelShown,
  showSideTab,
  sidePanel,
  sideShows,
} from "./worktrees.svelte";

const MAIN = "/r";
const TOPIC = "/r.worktrees/topic";

/** A view of repository 1 with two worktrees, `MAIN` active; its panel shown on its first tab. */
function view(overrides: Partial<RepoSnapshot> = {}): RepoView {
  const v = {
    repoId: 1,
    panel: new PanelState(null),
    snapshot: snap({
      activeWorktree: MAIN,
      worktrees: [
        worktree({ path: MAIN, branch: "main", isMain: true, active: true }),
        worktree({ path: TOPIC, branch: "topic" }),
      ],
      ...overrides,
    }),
  };
  views.set(1, v);
  return v as unknown as RepoView;
}

beforeEach(() => {
  views.clear();
  sidePanel.windowW = 1280;
  sidePanel.overlayOpen = false;
  vi.mocked(api.setRepoPrefs).mockClear();
});

describe("tabForKey", () => {
  it("moves with the arrows, round the ends, and to the ends with Home and End", () => {
    // Commit · Pull request · Worktrees.
    expect(SIDE_TABS).toEqual(["commit", "pr", "worktrees"]);
    expect(tabForKey(SIDE_TABS, "commit", "ArrowRight")).toBe("pr");
    expect(tabForKey(SIDE_TABS, "pr", "ArrowRight")).toBe("worktrees");
    expect(tabForKey(SIDE_TABS, "worktrees", "ArrowRight")).toBe("commit");
    expect(tabForKey(SIDE_TABS, "commit", "ArrowLeft")).toBe("worktrees");
    expect(tabForKey(SIDE_TABS, "worktrees", "ArrowLeft")).toBe("pr");
    expect(tabForKey(SIDE_TABS, "pr", "End")).toBe("worktrees");
    expect(tabForKey(["a", "b", "c"], "b", "Home")).toBe("a");
    expect(tabForKey(["a", "b", "c"], "b", "End")).toBe("c");
  });

  it("takes no other key", () => {
    expect(tabForKey(SIDE_TABS, "commit", "ArrowDown")).toBeNull();
    expect(tabForKey(SIDE_TABS, "commit", "Enter")).toBeNull();
    expect(tabForKey([], "commit", "ArrowRight")).toBeNull();
  });
});

describe("the side panel's tabs", () => {
  it("opens on the Commit tab, the active worktree's", () => {
    const v = view();
    expect(panelState(1).tab).toBe("commit");
    expect(sideShows(v)).toBe("commit");
    expect(commitPath(v)).toBe(MAIN);
    expect(committingIn(v)?.branch).toBe("main");
  });

  it("shown again, it comes back on the tab last used, per repository", () => {
    const v = view();
    showSideTab(v, "worktrees");
    setPanelShown(1, false);
    expect(sideShows(v)).toBe("hidden");
    setPanelShown(1);
    expect(sideShows(v)).toBe("worktrees");
    expect(api.setRepoPrefs).toHaveBeenLastCalledWith(1, { sidePanel: true });

    // Another repository keeps its own.
    const other = { ...v, repoId: 2, panel: new PanelState(null) };
    views.set(2, other);
    expect(sideShows(other as unknown as RepoView)).toBe("commit");
  });

  it("over the graph (a narrow window), shown for this session only, on the same tab", () => {
    const v = view();
    showSideTab(v, "worktrees");
    sidePanel.windowW = 800;
    expect(panelShown(1)).toBe(false);
    setPanelShown(1);
    expect(sideShows(v)).toBe("worktrees");
    expect(api.setRepoPrefs).not.toHaveBeenCalled();
  });

  it("a tab chosen shows the panel; the focus goes into the list only when asked for", () => {
    const v = view();
    setPanelShown(1, false);
    const asked = panelState(1).focusRequest;
    showSideTab(v, "worktrees");
    expect(sideShows(v)).toBe("worktrees");
    expect(panelState(1).focusRequest).toBe(asked);
    setPanelShown(1, false);
    showSideTab(v, "worktrees", true);
    expect(panelState(1).focusRequest).toBe(asked + 1);
    // Never for the Commit tab.
    showSideTab(v, "commit", true);
    expect(panelState(1).focusRequest).toBe(asked + 1);
  });

  it("the Worktrees tab is the way back from a commit panel: its worktree selected in the list", () => {
    const v = view();
    openCommit(v, TOPIC);
    showSideTab(v, "worktrees");
    expect(panelState(1).selected).toBe(TOPIC);
    // The Commit tab holds the active worktree's again.
    showSideTab(v, "commit");
    expect(commitPath(v)).toBe(MAIN);
  });
});

describe("the commit panel", () => {
  it("of the active worktree: the Commit tab, shown, the focus asked for its message", () => {
    const v = view();
    showSideTab(v, "worktrees");
    setPanelShown(1, false);
    const focus = panelState(1).commitFocus;
    openCommit(v);
    expect(sideShows(v)).toBe("commit");
    expect(commitPath(v)).toBe(MAIN);
    expect(panelState(1).commitFocus).toBe(focus + 1);
  });

  it("shown, it holds its worktree: another made active under it does not change what it commits", () => {
    const v = view();
    holdCommit(v);
    expect(panelState(1).commitFor).toBe(MAIN);
    (v as { snapshot: RepoSnapshot }).snapshot = { ...v.snapshot, activeWorktree: TOPIC };
    holdCommit(v);
    expect(commitPath(v)).toBe(MAIN);
    // Cmd+K: the active worktree's, now.
    openCommit(v);
    expect(commitPath(v)).toBe(TOPIC);
  });

  it("hidden, or on the Worktrees tab, it holds nothing: shown, the active worktree's", () => {
    const v = view();
    showSideTab(v, "worktrees");
    holdCommit(v);
    expect(panelState(1).commitFor).toBeNull();
    showSideTab(v, "commit");
    setPanelShown(1, false);
    holdCommit(v);
    expect(panelState(1).commitFor).toBeNull();
    // Nothing to hold before the repository is read.
    const unread = view({ activeWorktree: "" });
    holdCommit(unread);
    expect(panelState(1).commitFor).toBeNull();
  });

  it("of another worktree: on the Commit tab too, until the active one's is asked for (Cmd+K, its line's button)", () => {
    const v = view();
    showSideTab(v, "worktrees");
    openCommit(v, TOPIC);
    expect(sideShows(v)).toBe("commit");
    expect(commitPath(v)).toBe(TOPIC);
    expect(committingIn(v)?.branch).toBe("topic");
    openCommit(v);
    expect(commitPath(v)).toBe(MAIN);
  });

  it("closed (Escape, nothing left to commit): the list, its worktree selected, the focus asked for", () => {
    const v = view();
    openCommit(v, TOPIC);
    const asked = panelState(1).focusRequest;
    closeCommit(v);
    expect(sideShows(v)).toBe("worktrees");
    expect(panelState(1).selected).toBe(TOPIC);
    expect(panelState(1).focusRequest).toBe(asked + 1);
    // The Commit tab holds the active worktree's again.
    expect(commitPath(v)).toBe(MAIN);
    // A worktree no longer listed is not selected.
    openCommit(v, "/gone");
    closeCommit(v);
    expect(panelState(1).selected).toBe(TOPIC);
  });

  it("before the worktrees are read: the active worktree's path, no worktree yet", () => {
    const v = view({ worktrees: [] });
    expect(commitPath(v)).toBe(MAIN);
    expect(committingIn(v)).toBeNull();
  });
});
