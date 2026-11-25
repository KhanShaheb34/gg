import { beforeEach, describe, expect, it, vi } from "vitest";
import { pullRequest, snap, worktree } from "../menus/menuTestData";
import type { PrState, PrSuggestion, RepoSnapshot } from "../types";

vi.mock("../ipc", () => ({
  api: {
    getRepoPrefs: vi.fn(async () => ({ sidePanel: true })),
    setRepoPrefs: vi.fn(async () => {}),
  },
}));
// The views the helpers find, by repository.
const views = vi.hoisted(() => new Map<number, unknown>());
vi.mock("./repo.svelte", () => ({ findRepoView: (repoId: number) => views.get(repoId) }));
const appState = vi.hoisted(() => ({ app: { homeDir: "/home/ada", activeRepoId: 1 as number | null, settings: { openPrTabAfterPush: true } } }));
vi.mock("./app.svelte", () => ({ app: appState.app, reportError: vi.fn(), toast: vi.fn() }));
const overlayState = vi.hoisted(() => ({ overlays: { dialog: null as unknown } }));
vi.mock("./overlays.svelte", () => ({ overlays: overlayState.overlays, openMenu: vi.fn(), showDialog: vi.fn() }));
vi.mock("./ops.svelte", () => ({ runOp: vi.fn() }));
vi.mock("./editor", () => ({ openExternal: vi.fn() }));
vi.mock("../menus/dispatch", () => ({ pickPullRequest: vi.fn() }));
vi.mock("../components/dialogs/CreateWorktreeDialog.svelte", () => ({ default: {} }));
vi.mock("../components/dialogs/DeleteWorktreeDialog.svelte", () => ({ default: {} }));
vi.mock("../components/dialogs/PushDialog.svelte", () => ({ default: {} }));

import { PanelState } from "./panelState.svelte";
import {
  afterPush,
  applySuggestion,
  baseRemoteOf,
  defaultBranch,
  dropPrDraft,
  forgetSuggestions,
  keepSuggestion,
  keptSuggestion,
  knownPr,
  opensAfterPush,
  openPullRequestTab,
  prBackToCurrent,
  prDraft,
  prNumberOf,
  prSubject,
  prTabTitle,
  rememberMade,
  subjectOf,
  suggestionStamp,
  SUGGESTION_FRESH_MS,
  unavailableHere,
  type AfterPush,
} from "./pullRequest.svelte";
import type { RepoView } from "./repo.svelte";
import { panelState, setPanelShown, showSideTab, sidePanel, sideShows } from "./worktrees.svelte";

const ready = (loadedAtMs = 5): PrState => ({ status: "ready", reason: null, loadedAtMs, partial: false });

/** A view of repository `repoId` on `main`, with branches main, feature/x and fix/y, pull requests listed. */
function view(overrides: Partial<RepoSnapshot> = {}, repoId = 1): RepoView {
  const v = {
    repoId,
    panel: new PanelState(null),
    snapshot: snap({
      repoId,
      rootPath: `/r${repoId}`,
      activeWorktree: `/r${repoId}`,
      branches: ["feature/x", "fix/y", "main"],
      remoteBranches: ["origin/feature/x", "origin/main"],
      worktrees: [worktree({ path: `/r${repoId}`, branch: "main", isMain: true, active: true })],
      prs: ready(),
      ...overrides,
    }),
  };
  views.set(repoId, v);
  return v as unknown as RepoView;
}

function suggestion(overrides: Partial<PrSuggestion> = {}): PrSuggestion {
  return {
    title: "Add x",
    body: "- Add x",
    base: "main",
    baseRemote: "origin",
    existing: null,
    compareUrl: "https://github.com/octo/cat/compare/main...feature/x?expand=1",
    repo: "octo/cat",
    pushRemote: "origin",
    pushTo: "origin/feature/x",
    needsPush: false,
    ...overrides,
  };
}

beforeEach(() => {
  views.clear();
  sidePanel.windowW = 1280;
  sidePanel.overlayOpen = false;
  appState.app.activeRepoId = 1;
  appState.app.settings.openPrTabAfterPush = true;
  overlayState.overlays.dialog = null;
});

describe("the branch the tab is about", () => {
  it("is the active worktree's branch, and follows it as it changes", () => {
    expect(prSubject(null, snap({ head: { branch: "main", hash: "h" } }))).toBe("main");
    expect(prSubject(null, snap({ head: { branch: "feature/x", hash: "h" } }))).toBe("feature/x");
  });

  it("is the branch a menu asked for while it is a local branch, else the current one again", () => {
    const s = snap({ branches: ["main", "feature/x"], head: { branch: "main", hash: "h" } });
    expect(prSubject("feature/x", s)).toBe("feature/x");
    expect(prSubject("gone", s)).toBe("main");
  });

  it("is none on a detached HEAD, unless a branch was asked for", () => {
    const detached = snap({ branches: ["main", "feature/x"], head: { branch: null, hash: "h" } });
    expect(prSubject(null, detached)).toBeNull();
    expect(prSubject("feature/x", detached)).toBe("feature/x");
  });

  it("opening the tab for a branch pins it (not the current one); the shortcut and the way back follow the current one", () => {
    const v = view();
    openPullRequestTab(v, "fix/y", true);
    expect(sideShows(v)).toBe("pr");
    expect(panelState(1).prFor).toBe("fix/y");
    expect(subjectOf(v)).toBe("fix/y");
    // Asked for the current branch: nothing pinned.
    openPullRequestTab(v, "main", true);
    expect(panelState(1).prFor).toBeNull();
    openPullRequestTab(v, "fix/y", false);
    openPullRequestTab(v, null, true);
    expect(subjectOf(v)).toBe("main");
    openPullRequestTab(v, "fix/y", false);
    const asked = panelState(1).prFocus;
    prBackToCurrent(v);
    expect(subjectOf(v)).toBe("main");
    expect(panelState(1).prFocus).toBe(asked + 1);
  });

  it("the focus goes into the tab only when the user asked for it", () => {
    const v = view();
    const before = panelState(1).prFocus;
    openPullRequestTab(v, "fix/y", false);
    expect(panelState(1).prFocus).toBe(before);
    openPullRequestTab(v, "fix/y", true);
    expect(panelState(1).prFocus).toBe(before + 1);
  });
});

describe("tab memory", () => {
  it("the panel shown again comes back on the Pull request tab when it was the last used, per repository", () => {
    const v = view();
    openPullRequestTab(v, null, false);
    setPanelShown(1, false);
    expect(sideShows(v)).toBe("hidden");
    setPanelShown(1);
    expect(sideShows(v)).toBe("pr");
    // Another repository keeps its own.
    const other = view({}, 2);
    expect(sideShows(other)).toBe("commit");
    // Leaving it for another tab, and back, keeps the branch asked for.
    openPullRequestTab(v, "fix/y", false);
    showSideTab(v, "worktrees");
    showSideTab(v, "pr");
    expect(subjectOf(v)).toBe("fix/y");
  });
});

describe("the default branch", () => {
  it("is what the base remote's HEAD names; else its main or master", () => {
    expect(defaultBranch(snap({ remotes: ["origin"], remoteHeads: { origin: "trunk" }, remoteBranches: ["origin/main"] }))).toBe("trunk");
    expect(defaultBranch(snap({ remotes: ["origin"], remoteHeads: {}, remoteBranches: ["origin/main"] }))).toBe("main");
    expect(defaultBranch(snap({ remotes: ["origin"], remoteHeads: {}, remoteBranches: ["origin/master"] }))).toBe("master");
    expect(defaultBranch(snap({ remotes: ["origin"], remoteHeads: {}, remoteBranches: ["origin/dev"] }))).toBeNull();
    expect(defaultBranch(snap({ remotes: [], remoteHeads: {}, remoteBranches: [] }))).toBeNull();
  });

  it("is the base repository's: upstream before origin, as the core picks it; or the remote named", () => {
    const fork = snap({ remotes: ["origin", "upstream"], remoteHeads: { origin: "mine", upstream: "main" }, remoteBranches: [] });
    expect(baseRemoteOf(fork.remotes)).toBe("upstream");
    expect(defaultBranch(fork)).toBe("main");
    expect(defaultBranch(fork, "origin")).toBe("mine");
    expect(baseRemoteOf(["zeta", "alpha"])).toBe("zeta");
  });
});

describe("why none can be made", () => {
  it("no remote, a remote not on GitHub, gh missing or signed out; a failed load is not a reason", () => {
    expect(unavailableHere(ready(), [])).toBe("no_remote");
    expect(unavailableHere({ status: "unavailable", reason: "not_github", loadedAtMs: null, partial: false }, ["origin"])).toBe("not_github");
    expect(unavailableHere({ status: "unavailable", reason: "no_gh", loadedAtMs: null, partial: false }, ["origin"])).toBe("no_gh");
    expect(unavailableHere({ status: "unavailable", reason: "signed_out", loadedAtMs: null, partial: false }, ["origin"])).toBe("signed_out");
    expect(unavailableHere({ status: "unavailable", reason: "failed", loadedAtMs: null, partial: false }, ["origin"])).toBeNull();
    expect(unavailableHere({ status: "loading", reason: null, loadedAtMs: null, partial: false }, ["origin"])).toBeNull();
    expect(unavailableHere(ready(), ["origin"])).toBeNull();
  });
});

describe("the forms being filled in", () => {
  it("are kept per repository and branch, the same one whenever the tab shows it, until dropped", () => {
    const d = prDraft("/r1", "feature/x");
    d.title = "Typed";
    d.body = "Why";
    d.base = "dev";
    d.draft = true;
    d.titleTouched = true;
    expect(prDraft("/r1", "feature/x")).toBe(d);
    expect(prDraft("/r1", "feature/x").title).toBe("Typed");
    expect(prDraft("/r1", "fix/y").title).toBe("");
    expect(prDraft("/r2", "feature/x").title).toBe("");
    dropPrDraft("/r1", "feature/x");
    const fresh = prDraft("/r1", "feature/x");
    expect(fresh).not.toBe(d);
    expect(fresh).toMatchObject({ title: "", body: "", base: "", draft: false, pending: false, failure: null });
  });

  it("a suggestion fills only what the user has not changed; the base stays the user's in the same repository", () => {
    const d = prDraft("/r9", "a");
    applySuggestion(d, suggestion(), "origin");
    expect(d).toMatchObject({ title: "Add x", body: "- Add x", base: "main" });
    d.title = "Mine";
    d.titleTouched = true;
    d.base = "release";
    d.baseTouched = true;
    applySuggestion(d, suggestion({ title: "Other", body: "- Other", base: "dev" }), "origin");
    expect(d).toMatchObject({ title: "Mine", body: "- Other", base: "release" });
    // Another repository than guessed: its base, whatever was picked.
    applySuggestion(d, suggestion({ base: "trunk", baseRemote: "upstream" }), "origin");
    expect(d.base).toBe("trunk");
    dropPrDraft("/r9", "a");
  });
});

describe("what the core suggested", () => {
  it("is kept while the branch, where it was pushed and the list hold, for five minutes", () => {
    const s = suggestion();
    keepSuggestion("/k", "feature/x", "A", s, 1000);
    expect(keptSuggestion("/k", "feature/x", "A", 2000)).toBe(s);
    expect(keptSuggestion("/k", "feature/x", "B", 2000)).toBeNull();
    expect(keptSuggestion("/k", "feature/x", "A", 1000 + SUGGESTION_FRESH_MS + 1)).toBeNull();
    expect(keptSuggestion("/k", "fix/y", "A", 2000)).toBeNull();
    forgetSuggestions("/k");
    expect(keptSuggestion("/k", "feature/x", "A", 2000)).toBeNull();
  });

  it("depends on the branch's commit, its remote namesake's, and when the list was loaded", () => {
    const base = snap({
      branches: ["feature/x"],
      branchTips: [{ hash: "1", time: 0, author: "", subject: "" }],
      remoteBranches: ["origin/feature/x"],
      remoteBranchTips: [{ hash: "1", time: 0, author: "", subject: "" }],
      prs: ready(5),
    });
    const stamp = suggestionStamp(base, "feature/x");
    expect(suggestionStamp({ ...base, branchTips: [{ hash: "2", time: 0, author: "", subject: "" }] }, "feature/x")).not.toBe(stamp);
    expect(suggestionStamp({ ...base, remoteBranchTips: [{ hash: "2", time: 0, author: "", subject: "" }] }, "feature/x")).not.toBe(stamp);
    expect(suggestionStamp({ ...base, prs: ready(6) }, "feature/x")).not.toBe(stamp);
    expect(suggestionStamp({ ...base, seq: 99 }, "feature/x")).toBe(stamp);
  });
});

describe("the branch's pull request", () => {
  it("is the list's; one just made stands in until a list loaded since comes", () => {
    const s = snap({ rootPath: "/m", prs: ready(5), branchPrs: { "feature/x": pullRequest() } });
    expect(knownPr(s, "feature/x")?.number).toBe(12);
    expect(knownPr(s, "fix/y")).toBeNull();
    rememberMade(s, "fix/y", pullRequest({ number: 21, head: "fix/y" }));
    expect(knownPr(s, "fix/y")?.number).toBe(21);
    // Reloaded since: the list says (here, it has it under another number).
    expect(knownPr({ ...s, prs: ready(6), branchPrs: { "fix/y": pullRequest({ number: 22 }) } }, "fix/y")?.number).toBe(22);
    expect(knownPr({ ...s, prs: ready(6) }, "fix/y")).toBeNull();
  });

  it("its number comes from its page's address", () => {
    expect(prNumberOf("https://github.com/octo/cat/pull/21")).toBe(21);
    expect(prNumberOf("https://github.com/octo/cat/pull/21/files")).toBe(21);
    expect(prNumberOf("https://github.com/octo/cat/compare/main...x")).toBeNull();
  });

  it("the tab's tooltip says it, or none yet, with the shortcut", () => {
    const s = snap({ prs: ready(), branchPrs: { "feature/x": pullRequest(), "fix/y": pullRequest({ number: 13, state: "draft" }) } });
    expect(prTabTitle(s, "feature/x", "⌘⇧P")).toBe("Pull request · #12 open (⌘⇧P)");
    expect(prTabTitle(s, "fix/y", "⌘⇧P")).toBe("Pull request · #13 draft (⌘⇧P)");
    expect(prTabTitle(s, "main", "Ctrl+Shift+P")).toBe("Pull request · none yet (Ctrl+Shift+P)");
    expect(prTabTitle({ ...s, prs: { status: "unknown", reason: null, loadedAtMs: null, partial: false } }, "main", "⌘⇧P")).toBe(
      "Pull request (⌘⇧P)",
    );
    expect(prTabTitle(s, null, "⌘⇧P")).toBe("Pull request (⌘⇧P)");
    // With its checks, when known.
    expect(prTabTitle(s, "feature/x", "⌘⇧P", "checks failing")).toBe("Pull request · #12 open · checks failing (⌘⇧P)");
    expect(prTabTitle(s, "main", "⌘⇧P", "checks failing")).toBe("Pull request · none yet (⌘⇧P)");
  });
});

describe("after a push", () => {
  const base = (): AfterPush => ({
    setting: true,
    dialogOpen: false,
    activeRepo: true,
    branch: "feature/x",
    snap: snap({
      rootPath: "/p",
      branches: ["feature/x", "main"],
      remotes: ["origin"],
      remoteBranches: ["origin/feature/x", "origin/main"],
      remoteHeads: {},
      prs: ready(),
      branchPrs: {},
    }),
  });

  it("shows the tab for a local branch with no pull request where pull requests are listed", () => {
    expect(opensAfterPush(base())).toBe(true);
    // An older list is taken as it is.
    expect(opensAfterPush({ ...base(), snap: { ...base().snap, prs: { ...ready(), reason: "failed" } } })).toBe(true);
  });

  it("not with the setting off", () => {
    expect(opensAfterPush({ ...base(), setting: false })).toBe(false);
  });

  it("not while a dialog is open, nor for a repository the window does not show", () => {
    expect(opensAfterPush({ ...base(), dialogOpen: true })).toBe(false);
    expect(opensAfterPush({ ...base(), activeRepo: false })).toBe(false);
  });

  it("not when the branch has a pull request (listed, or just made)", () => {
    expect(opensAfterPush({ ...base(), snap: { ...base().snap, branchPrs: { "feature/x": pullRequest() } } })).toBe(false);
    rememberMade(base().snap, "feature/x", pullRequest());
    expect(opensAfterPush(base())).toBe(false);
    rememberMade({ ...base().snap, prs: ready(1) }, "feature/x", pullRequest());
  });

  it("not for the default branch (its remote's HEAD, else main)", () => {
    expect(opensAfterPush({ ...base(), branch: "main" })).toBe(false);
    const trunk = { ...base().snap, remoteHeads: { origin: "feature/x" } };
    expect(opensAfterPush({ ...base(), snap: trunk })).toBe(false);
    expect(opensAfterPush({ ...base(), snap: trunk, branch: "main" })).toBe(true);
  });

  it("not where pull requests are not listed: unknown, loading, unavailable", () => {
    for (const prs of [
      { status: "unknown", reason: null, loadedAtMs: null, partial: false },
      { status: "loading", reason: null, loadedAtMs: null, partial: false },
      { status: "unavailable", reason: "no_gh", loadedAtMs: null, partial: false },
      { status: "unavailable", reason: "signed_out", loadedAtMs: null, partial: false },
      { status: "unavailable", reason: "not_github", loadedAtMs: null, partial: false },
    ] as PrState[]) {
      expect(opensAfterPush({ ...base(), snap: { ...base().snap, prs } })).toBe(false);
    }
  });

  it("not for a branch that is not a local one", () => {
    expect(opensAfterPush({ ...base(), branch: "origin/feature/x" })).toBe(false);
  });

  it("shows the panel on the tab for the branch pushed, without the focus; or leaves everything as it was", () => {
    const v = view();
    setPanelShown(1, false);
    const focus = panelState(1).prFocus;
    afterPush(1, "feature/x");
    expect(sideShows(v)).toBe("pr");
    expect(subjectOf(v)).toBe("feature/x");
    expect(panelState(1).prFocus).toBe(focus);

    // The setting off: nothing changes.
    showSideTab(v, "commit");
    appState.app.settings.openPrTabAfterPush = false;
    afterPush(1, "fix/y");
    expect(sideShows(v)).toBe("commit");
    // A dialog open: nothing changes.
    appState.app.settings.openPrTabAfterPush = true;
    overlayState.overlays.dialog = {};
    afterPush(1, "fix/y");
    expect(sideShows(v)).toBe("commit");
    // Another repository's push.
    overlayState.overlays.dialog = null;
    view({}, 2);
    afterPush(2, "fix/y");
    expect(sideShows(v)).toBe("commit");
    expect(panelState(2).tab).toBe("commit");
  });

  it("a push of the tab's own branch has its pull request's checks asked for again; another branch's does not", () => {
    const v = view();
    openPullRequestTab(v, "feature/x", false);
    const before = panelState(1).prPushed;
    afterPush(1, "feature/x");
    expect(panelState(1).prPushed).toBe(before + 1);
    afterPush(1, "fix/y");
    expect(panelState(1).prPushed).toBe(before + 1);
  });
});
