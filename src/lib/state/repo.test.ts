import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CommitRow, RepoSnapshot, ViewOptions } from "../types";
import { readOptions, sameOptions } from "./viewOptionsStore";

// A fake core: the rows of the current generation are a list of hashes.
const core = vi.hoisted(() => ({
  generation: 1,
  hashes: [] as string[],
  handler: null as ((payload: unknown) => void) | null,
}));

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn((_name: string, handler: (payload: unknown) => void) => {
      core.handler = handler;
      return () => (core.handler = null);
    }),
    api: {
      getSnapshot: vi.fn(),
      getRows: vi.fn(),
      findRow: vi.fn(async (_repoId: number, hash: string) => {
        const i = core.hashes.indexOf(hash);
        return i < 0 ? null : i;
      }),
      perfMark: vi.fn(async () => {}),
      searchCommits: vi.fn(async () => []),
      searchBodies: vi.fn(async () => ({ generation: 1, rows: [] })),
      setViewOptions: vi.fn(),
      fetch: vi.fn(async () => {}),
      refresh: vi.fn(),
    },
  };
});

function row(index: number, hash: string): CommitRow {
  return {
    index,
    kind: "commit",
    hash,
    parents: [],
    author: "",
    email: "",
    time: 0,
    subject: hash,
    lane: 0,
    color: 0,
    edges: [],
    labels: [],
    isHead: false,
    isMerge: false,
    detachedWorktrees: [],
  };
}

function snapshot(overrides: Partial<RepoSnapshot> = {}): RepoSnapshot {
  return {
    repoId: 7,
    name: "r",
    rootPath: "/r",
    activeWorktree: "/r",
    generation: core.generation,
    totalRows: core.hashes.length,
    loading: false,
    maxLane: 0,
    head: { branch: "main", hash: core.hashes[0] ?? null },
    branches: ["main"],
    remoteBranches: [],
    remotes: ["origin"],
    worktrees: [],
    status: { changed: 0, untracked: 0, conflicted: 0, conflictedPaths: [] },
    viewOptions: { branches: null, showRemotes: true, showTags: true, showStashes: true },
    fetch: { running: false, lastError: null, lastErrorDetail: null, lastAtMs: null },
    op: null,
    inProgress: null,
    seq: 0,
    ...overrides,
  };
}

/** The core gains `n` new commits on top: a new generation. */
function commitOnTop(n: number) {
  core.generation++;
  core.hashes = [...Array.from({ length: n }, (_, i) => `new${core.generation}-${i}`), ...core.hashes];
}

const settle = () => new Promise((r) => setTimeout(r, 0));

type Ipc = typeof import("../ipc");
let ipc: Ipc;
let repo: typeof import("./repo.svelte");

/** A Map-backed localStorage; the unit tests run without a DOM. */
function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => items.clear(),
    getItem: (k) => items.get(k) ?? null,
    key: (i) => [...items.keys()][i] ?? null,
    removeItem: (k) => void items.delete(k),
    setItem: (k, v) => void items.set(k, String(v)),
  };
}

beforeEach(async () => {
  vi.resetModules();
  vi.stubGlobal("localStorage", memoryStorage());
  core.generation = 1;
  core.hashes = Array.from({ length: 1000 }, (_, i) => `c${i}`);
  ipc = await import("../ipc");
  repo = await import("./repo.svelte");
  vi.mocked(ipc.api.getSnapshot).mockReset().mockImplementation(async () => snapshot());
  vi.mocked(ipc.api.getRows)
    .mockReset()
    .mockImplementation(async (_repoId: number, generation: number, start: number, count: number) => {
      if (generation !== core.generation) {
        throw new ipc.CoreError({ code: "stale_generation", message: "stale", detail: null });
      }
      const rows = core.hashes.slice(start, start + count).map((hash, i) => row(start + i, hash));
      return { generation, start, rows };
    });
  vi.mocked(ipc.api.searchCommits).mockReset().mockImplementation(async () => []);
  vi.mocked(ipc.api.searchBodies)
    .mockReset()
    .mockImplementation(async () => ({ generation: core.generation, rows: [] }));
  vi.mocked(ipc.api.setViewOptions).mockReset();
  vi.mocked(ipc.api.fetch).mockReset().mockImplementation(async () => {});
  vi.mocked(ipc.api.refresh).mockReset();
});

/** A view of the fake repository with rows 0..399 loaded, scrolled to `scrollTop`. */
async function openView(scrollTop: number) {
  const view = repo.repoView(7, snapshot());
  await settle();
  await view.cache.ensure(0, 0);
  await view.cache.ensure(Math.floor(scrollTop / 26), Math.floor(scrollTop / 26) + 30);
  view.scrollTop = scrollTop;
  view.pendingScroll = null;
  return view;
}

describe("RepoView.applySnapshot", () => {
  it("keeps cached rows when only the row count grows", async () => {
    core.hashes = core.hashes.slice(0, 300);
    const view = await openView(0);
    const calls = vi.mocked(ipc.api.getRows).mock.calls.length;
    const cache = view.cache;
    core.hashes = [...core.hashes, ...Array.from({ length: 200 }, (_, i) => `old${i}`)];
    await view.applySnapshot(snapshot({ loading: true }));
    expect(view.cache).toBe(cache);
    expect(view.snapshot.totalRows).toBe(500);
    expect(view.cache.get(10)?.hash).toBe("c10");
    expect(vi.mocked(ipc.api.getRows).mock.calls.length).toBe(calls);
  });

  it("keeps the first fully visible commit at the same offset when rows are added above", async () => {
    const view = await openView(26 * 40 + 5); // row 41 is the first fully visible, 21 px down
    const changed = vi.fn();
    view.onChange = changed;
    commitOnTop(3);
    await view.applySnapshot(snapshot());
    expect(view.pendingScroll).toBe(26 * 43 + 5);
    expect(view.scrollTop).toBe(26 * 43 + 5);
    expect(view.snapshot.generation).toBe(2);
    expect(view.cache.get(44)?.hash).toBe("c41"); // loaded before the swap
    expect(changed).toHaveBeenCalled();
  });

  it("serves the old rows until the new ones have arrived", async () => {
    const view = await openView(26 * 40);
    commitOnTop(1);
    const applying = view.applySnapshot(snapshot());
    expect(view.snapshot.generation).toBe(1);
    expect(view.cache.get(40)?.hash).toBe("c40");
    await applying;
    expect(view.cache.get(41)?.hash).toBe("c40");
  });

  it("stays at the top when scrolled to the top, so new commits show", async () => {
    const view = await openView(0);
    commitOnTop(2);
    await view.applySnapshot(snapshot());
    expect(view.pendingScroll).toBe(0);
    expect(view.cache.get(0)?.hash).toBe("new2-0");
  });

  it("falls back to the same row index when the anchored commit is gone", async () => {
    const view = await openView(26 * 100);
    core.generation++;
    core.hashes = core.hashes.filter((h) => h !== "c100");
    await view.applySnapshot(snapshot());
    expect(view.pendingScroll).toBe(26 * 100);
  });

  it("keeps the selection by hash, and drops it when the commit is gone", async () => {
    const view = await openView(0);
    view.select("c5", 5);
    commitOnTop(2);
    await view.applySnapshot(snapshot());
    expect(view.selectedHash).toBe("c5");
    expect(view.selectedIndex).toBe(7);

    core.generation++;
    core.hashes = core.hashes.filter((h) => h !== "c5");
    await view.applySnapshot(snapshot());
    expect(view.selectedHash).toBeNull();
    expect(view.selectedIndex).toBeNull();
  });

  it("ignores a snapshot older than the one shown", async () => {
    const view = await openView(0);
    commitOnTop(1);
    await view.applySnapshot(snapshot());
    await view.applySnapshot(snapshot({ generation: 1 }));
    await view.applySnapshot(snapshot({ totalRows: 3 }));
    expect(view.snapshot.generation).toBe(2);
    expect(view.snapshot.totalRows).toBe(1001);
  });

  it("drops a snapshot whose seq is lower than the one it holds, whatever else it says (R14)", async () => {
    const view = await openView(0);
    const op = { id: 3, title: "Merge feature/x into main", startedAtMs: 1, queued: 0 };
    await view.applySnapshot(snapshot({ seq: 5, op }));
    // An operation's outcome that predates the event above: same rows, older state.
    await view.applySnapshot(snapshot({ seq: 4, op: null }));
    expect(view.snapshot.seq).toBe(5);
    expect(view.snapshot.op).toEqual(op);
    await view.applySnapshot(snapshot({ seq: 6, op: null }));
    expect(view.snapshot.op).toBeNull();

    // Older rows too: a generation being swapped in is not replaced by an older snapshot of it.
    commitOnTop(2);
    const applying = view.applySnapshot(snapshot({ seq: 8, loading: true }));
    await view.applySnapshot(snapshot({ seq: 7, totalRows: core.hashes.length + 5 }));
    await applying;
    expect(view.snapshot.seq).toBe(8);
    expect(view.snapshot.loading).toBe(true);
  });

  it("findRepoView finds an open repository's view without making one", async () => {
    expect(repo.findRepoView(7)).toBeUndefined();
    const view = repo.repoView(7, snapshot());
    expect(repo.findRepoView(7)).toBe(view);
    repo.disposeRepoView(7);
    expect(repo.findRepoView(7)).toBeUndefined();
  });

  it("follows repo-changed events for its own repository only", async () => {
    const view = await openView(0);
    commitOnTop(1);
    core.handler?.(snapshot({ repoId: 8 }));
    await settle();
    expect(view.snapshot.generation).toBe(1);
    core.handler?.(snapshot());
    await vi.waitFor(() => expect(view.snapshot.generation).toBe(2));
  });

  it("swallows stale_generation from getRows and moves to the fresh snapshot", async () => {
    const view = await openView(0);
    commitOnTop(1); // the core moved on; no event seen yet
    const onError = vi.spyOn(console, "warn").mockImplementation(() => {});
    await view.cache.ensure(800, 810);
    await vi.waitFor(() => expect(view.snapshot.generation).toBe(2));
    expect(onError).not.toHaveBeenCalled();
    onError.mockRestore();
  });

  it("stops following events once disposed", async () => {
    const view = await openView(0);
    repo.disposeRepoView(7);
    expect(core.handler).toBeNull();
    expect(view.cache.get(0)).toBeUndefined();
  });

  it("never shows an empty or mixed generation when the rows change again mid-refresh", async () => {
    const view = await openView(26 * 300 + 9); // row 301 is the first fully visible, 17 px down
    const anchorHash = "c301";
    const renders: { generation: number; cache: unknown; anchorRow: string | undefined; loaded: boolean }[] = [];
    view.onChange = () => {
      const top = view.pendingScroll ?? view.scrollTop;
      const first = Math.ceil(top / 26);
      renders.push({
        generation: view.snapshot.generation,
        cache: view.cache,
        anchorRow: view.cache.get(first)?.hash,
        loaded: view.cache.loaded(Math.floor(top / 26), Math.floor(top / 26) + 25),
      });
    };
    const cacheBefore = view.cache;

    // The core answers for a snapshot after a round trip, as over IPC.
    vi.mocked(ipc.api.getSnapshot).mockImplementation(async () => {
      await new Promise((r) => setTimeout(r, 5));
      return snapshot();
    });
    // Generation 2 arrives; right after the anchor is looked up, the core moves on to 3.
    commitOnTop(2);
    vi.mocked(ipc.api.findRow).mockImplementationOnce(async (_id, hash) => {
      const i = core.hashes.indexOf(hash);
      commitOnTop(5);
      return i;
    });
    await view.applySnapshot(snapshot());
    await vi.waitFor(() => expect(view.snapshot.generation).toBe(3));
    await settle();

    // Every render showed loaded rows; the cache was swapped exactly once, straight to 3.
    expect(renders.length).toBeGreaterThan(0);
    for (const r of renders) expect(r.loaded).toBe(true);
    const caches = new Set(renders.map((r) => r.cache));
    caches.delete(cacheBefore);
    expect(caches.size).toBe(1);
    expect(renders.some((r) => r.generation === 2)).toBe(false);
    // The anchored commit is still 17 px below the top: 7 new commits above it.
    expect(view.scrollTop).toBe(26 * 308 - 17);
    expect(renders[renders.length - 1].anchorRow).toBe(anchorHash);
  });

  it("after a few failed attempts shows the new rows anyway, where it can, and says so", async () => {
    const view = await openView(26 * 50);
    const notice = vi.fn();
    view.onNotice = notice;
    commitOnTop(1);
    vi.mocked(ipc.api.getRows).mockImplementation(async () => {
      throw new ipc.CoreError({ code: "stale_generation", message: "stale", detail: null });
    });
    await view.applySnapshot(snapshot());
    await new Promise((r) => setTimeout(r, 20));
    // The old rows cannot be served any more either: the newest generation is shown.
    expect(view.snapshot.generation).toBe(2);
    expect(view.scrollTop).toBe(26 * 51); // the anchor's place, as findRow said
    expect(notice).toHaveBeenCalledTimes(1);
    expect(notice.mock.calls[0][0]).toBe("Could not keep your place in the history");
    expect(vi.mocked(ipc.api.getSnapshot).mock.calls.length).toBeLessThan(8);
  });

  it("when the new rows cannot be loaded, says why (not that commits kept changing) and recovers later", async () => {
    const view = await openView(26 * 50);
    const notice = vi.fn();
    view.onNotice = notice;
    commitOnTop(1);
    const working = vi.mocked(ipc.api.getRows).getMockImplementation()!;
    const warned = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(ipc.api.getRows).mockImplementation(async () => {
      throw new ipc.CoreError({ code: "git_failed", message: "Could not read the history", detail: "fatal: bad object" });
    });
    await view.applySnapshot(snapshot());
    await vi.waitFor(() => expect(notice).toHaveBeenCalledTimes(1));
    expect(notice).toHaveBeenCalledWith("Could not load the commits", "Could not read the history\nfatal: bad object");
    expect(view.snapshot.generation).toBe(2);

    // git works again: the next event (or a refresh) loads the rows.
    vi.mocked(ipc.api.getRows).mockImplementation(working);
    core.handler?.(snapshot());
    await view.cache.ensure(51, 60);
    expect(view.cache.get(51)?.hash).toBe(core.hashes[51]);
    expect(warned).toHaveBeenCalled();
    warned.mockRestore();
  });
});

const ALL: ViewOptions = { branches: null, showRemotes: true, showTags: true, showStashes: true };

describe("RepoView search", () => {
  it("run stores the hits and selects the first, scrolled into view", async () => {
    const view = await openView(0);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([30, 100, 400]);
    await view.search.run("fix");
    expect(ipc.api.searchCommits).toHaveBeenCalledWith(7, "fix");
    expect(view.search.hits).toEqual([30, 100, 400]);
    expect(view.search.index).toBe(0);
    expect(view.selectedHash).toBe("c30");
    expect(view.selectedIndex).toBe(30);
    const top = view.pendingScroll ?? view.scrollTop;
    expect(30 * 26).toBeGreaterThanOrEqual(top);
    expect(31 * 26).toBeLessThanOrEqual(top + view.viewportH);
  });

  it("next wraps from the last hit to the first, prev the other way", async () => {
    const view = await openView(0);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([3, 10, 40]);
    await view.search.run("fix");
    await view.search.next();
    expect(view.selectedHash).toBe("c10");
    await view.search.next();
    expect(view.selectedHash).toBe("c40");
    expect(view.search.index).toBe(2);
    await view.search.next();
    expect(view.selectedHash).toBe("c3");
    expect(view.search.index).toBe(0);
    await view.search.prev();
    expect(view.selectedHash).toBe("c40");
  });

  it("an empty query clears the hits without asking the core", async () => {
    const view = await openView(0);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([3, 10]);
    await view.search.run("fix");
    await view.search.run("   ");
    expect(view.search.hits).toEqual([]);
    expect(view.search.index).toBe(-1);
    expect(ipc.api.searchCommits).toHaveBeenCalledTimes(1);
  });

  it("ignores the answer to a query that was replaced meanwhile", async () => {
    const view = await openView(0);
    let answerFirst: (hits: number[]) => void = () => {};
    vi.mocked(ipc.api.searchCommits)
      .mockImplementationOnce(() => new Promise((r) => (answerFirst = r)))
      .mockResolvedValueOnce([7]);
    const first = view.search.run("fi");
    await view.search.run("fix");
    answerFirst([1, 2, 3]);
    await first;
    expect(view.search.hits).toEqual([7]);
    expect(view.selectedHash).toBe("c7");
  });

  it("looks the hits up again when the rows change, keeping the selection", async () => {
    const view = await openView(0);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([3, 10]);
    await view.search.run("fix");
    commitOnTop(2);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([5, 12]);
    await view.applySnapshot(snapshot());
    await vi.waitFor(() => expect(view.search.hits).toEqual([5, 12]));
    expect(view.selectedHash).toBe("c3");
    expect(view.search.index).toBe(0);
    expect(view.search.generation).toBe(2);
  });

  it("merges the commits whose message body matches when git has found them, keeping the selection", async () => {
    const view = await openView(0);
    let answer: (hits: { generation: number; rows: number[] }) => void = () => {};
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([10, 40]);
    vi.mocked(ipc.api.searchBodies).mockImplementation(() => new Promise((r) => (answer = r)));
    await view.search.run("fix");
    // Subject, author, hash and ref name hits come at once.
    expect(view.search.hits).toEqual([10, 40]);
    expect(view.selectedHash).toBe("c10");
    expect(ipc.api.searchBodies).toHaveBeenCalledWith(7, "fix");
    answer({ generation: 1, rows: [3, 10, 70] });
    await vi.waitFor(() => expect(view.search.hits).toEqual([3, 10, 40, 70]));
    expect(view.selectedHash).toBe("c10");
    expect(view.search.index).toBe(1);
  });

  it("says the message search is still running until its answer comes", async () => {
    const view = await openView(0);
    let answer: (hits: { generation: number; rows: number[] }) => void = () => {};
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([]);
    vi.mocked(ipc.api.searchBodies).mockImplementation(() => new Promise((r) => (answer = r)));
    await view.search.run("only in a body");
    expect(view.search.bodyPending).toBe(true);
    answer({ generation: 1, rows: [] });
    await vi.waitFor(() => expect(view.search.bodyPending).toBe(false));
    // A failed one ends it too.
    vi.mocked(ipc.api.searchBodies).mockRejectedValue(new ipc.CoreError({ code: "git_failed", message: "x", detail: null }));
    await view.search.run("again");
    await vi.waitFor(() => expect(view.search.bodyPending).toBe(false));
    await view.search.run("");
    expect(view.search.bodyPending).toBe(false);
  });

  it("selects the first body hit when nothing else matched", async () => {
    const view = await openView(0);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([]);
    vi.mocked(ipc.api.searchBodies).mockResolvedValue({ generation: 1, rows: [12, 30] });
    await view.search.run("only in a body");
    await vi.waitFor(() => expect(view.selectedHash).toBe("c12"));
    expect(view.search.hits).toEqual([12, 30]);
  });

  it("drops body hits of another query or of rows no longer shown", async () => {
    const view = await openView(0);
    const answers: ((hits: { generation: number; rows: number[] }) => void)[] = [];
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([10]);
    vi.mocked(ipc.api.searchBodies).mockImplementation(() => new Promise((r) => answers.push(r)));
    await view.search.run("fi");
    await view.search.run("fix");
    answers[0]({ generation: 1, rows: [5] }); // for "fi"
    answers[1]({ generation: 0, rows: [6] }); // older rows
    await settle();
    expect(view.search.hits).toEqual([10]);
  });

  it("does not search bodies while the rows are still loading, but once they are loaded", async () => {
    vi.mocked(ipc.api.getSnapshot).mockImplementation(async () => snapshot({ loading: true }));
    const view = repo.repoView(7, snapshot({ loading: true }));
    await settle();
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([3]);
    await view.search.run("fix");
    expect(ipc.api.searchBodies).not.toHaveBeenCalled();
    await view.applySnapshot(snapshot({ loading: false }));
    await vi.waitFor(() => expect(ipc.api.searchBodies).toHaveBeenCalledWith(7, "fix"));
  });

  it("closeSearch clears the query and the hits", async () => {
    const view = await openView(0);
    view.search.show();
    expect(view.search.open).toBe(true);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([3]);
    await view.search.run("fix");
    view.search.close();
    expect(view.search.open).toBe(false);
    expect(view.search.query).toBe("");
    expect(view.search.hits).toEqual([]);
  });
});

describe("RepoView view options", () => {
  it("setViewOptions sends the merged options and remembers them for the repository path", async () => {
    const view = await openView(0);
    vi.mocked(ipc.api.setViewOptions).mockImplementation(async (_id, options) => snapshot({ viewOptions: options }));
    const done = view.setViewOptions({ showRemotes: false });
    // The toolbar shows the choice at once.
    expect(view.viewOptions).toEqual({ ...ALL, showRemotes: false });
    await done;
    expect(ipc.api.setViewOptions).toHaveBeenCalledWith(7, { branches: null, showRemotes: false, showTags: true, showStashes: true });
    expect(JSON.parse(localStorage.getItem("gg.view.v1:/r")!)).toEqual({
      branches: null,
      showRemotes: false,
      showTags: true,
      showStashes: true,
    });
  });

  it("hiding remote branches sends only the visible selection, and showing them restores it", async () => {
    const s = snapshot({ remoteBranches: ["origin/main"], viewOptions: { ...ALL, branches: ["main", "origin/main"] } });
    vi.mocked(ipc.api.getSnapshot).mockImplementation(async () => s);
    const view = repo.repoView(7, s);
    await settle();
    vi.mocked(ipc.api.setViewOptions).mockImplementation(async (_id, options) =>
      snapshot({ remoteBranches: ["origin/main"], viewOptions: options }),
    );
    await view.setViewOptions({ showRemotes: false });
    expect(ipc.api.setViewOptions).toHaveBeenLastCalledWith(7, { branches: ["main"], showRemotes: false, showTags: true, showStashes: true });
    // The whole choice is kept (and remembered), only the visible part is shown.
    const kept = { branches: ["main", "origin/main"], showRemotes: false, showTags: true, showStashes: true };
    expect(view.viewOptions).toEqual(kept);
    expect(JSON.parse(localStorage.getItem("gg.view.v1:/r")!)).toEqual(kept);
    // An event with the core's (visible) options does not overwrite it.
    core.handler?.(snapshot({ remoteBranches: ["origin/main"], viewOptions: { ...ALL, branches: ["main"], showRemotes: false } }));
    await settle();
    expect(view.viewOptions).toEqual(kept);

    await view.setViewOptions({ showRemotes: true });
    expect(ipc.api.setViewOptions).toHaveBeenLastCalledWith(7, { branches: ["main", "origin/main"], showRemotes: true, showTags: true, showStashes: true });
  });

  it("a selection of only hidden remote branches shows all branches", async () => {
    const s = snapshot({ remoteBranches: ["origin/main"], viewOptions: { ...ALL, branches: ["origin/main"] } });
    vi.mocked(ipc.api.getSnapshot).mockImplementation(async () => s);
    const view = repo.repoView(7, s);
    await settle();
    vi.mocked(ipc.api.setViewOptions).mockImplementation(async (_id, options) =>
      snapshot({ remoteBranches: ["origin/main"], viewOptions: options }),
    );
    await view.setViewOptions({ showRemotes: false });
    expect(ipc.api.setViewOptions).toHaveBeenLastCalledWith(7, { branches: null, showRemotes: false, showTags: true, showStashes: true });
    expect(view.viewOptions.branches).toEqual(["origin/main"]);
    expect(view.visibleOptions.branches).toBeNull();
  });

  it("a failed change goes back to the options the core has, and remembers those", async () => {
    const view = await openView(0);
    const notice = vi.fn();
    view.onNotice = notice;
    vi.mocked(ipc.api.setViewOptions).mockRejectedValue(
      new ipc.CoreError({ code: "git_failed", message: "boom", detail: null }),
    );
    await view.setViewOptions({ showTags: false, showStashes: true });
    expect(view.viewOptions).toEqual(ALL);
    expect(localStorage.getItem("gg.view.v1:/r")).toBeNull(); // the defaults are not stored
    expect(notice).toHaveBeenCalledWith("Could not change the view", "boom");
  });

  it("a notice carries the core's detail below its message", async () => {
    const view = await openView(0);
    const notice = vi.fn();
    view.onNotice = notice;
    vi.mocked(ipc.api.setViewOptions).mockRejectedValue(
      new ipc.CoreError({ code: "git_failed", message: "git failed", detail: "fatal: bad object" }),
    );
    await view.setViewOptions({ showTags: false, showStashes: true });
    expect(notice).toHaveBeenCalledWith("Could not change the view", "git failed\nfatal: bad object");
  });

  it("applies the answer to a change only if no event came meanwhile; else asks again", async () => {
    const view = await openView(0);
    let answer: (s: RepoSnapshot) => void = () => {};
    vi.mocked(ipc.api.setViewOptions).mockImplementation(() => new Promise((r) => (answer = r)));
    const done = view.setViewOptions({ showTags: false, showStashes: true });
    // An event (newer than the answer) arrives first: the fetch finished.
    const fetched = { running: false, lastError: "fatal: x", lastErrorDetail: "fatal: x\n", lastAtMs: 5 };
    const latest = snapshot({ viewOptions: { ...ALL, showTags: false }, fetch: fetched });
    core.handler?.(latest);
    await settle();
    vi.mocked(ipc.api.getSnapshot).mockClear().mockImplementation(async () => latest);
    answer(snapshot({ viewOptions: { ...ALL, showTags: false } })); // built before the event
    await done;
    await settle();
    expect(view.snapshot.fetch).toEqual(fetched);
    expect(ipc.api.getSnapshot).toHaveBeenCalled();
  });

  it("sends quick successive changes one at a time, the newest last", async () => {
    const view = await openView(0);
    const answers: (() => void)[] = [];
    vi.mocked(ipc.api.setViewOptions).mockImplementation(
      (_id, options) => new Promise((r) => answers.push(() => r(snapshot({ viewOptions: options })))),
    );
    void view.setViewOptions({ showTags: false, showStashes: true });
    void view.setViewOptions({ branches: ["main"] });
    const last = view.setViewOptions({ showRemotes: false });
    expect(ipc.api.setViewOptions).toHaveBeenCalledTimes(1);
    answers[0]();
    await vi.waitFor(() => expect(ipc.api.setViewOptions).toHaveBeenCalledTimes(2));
    expect(ipc.api.setViewOptions).toHaveBeenLastCalledWith(7, { branches: ["main"], showRemotes: false, showTags: false, showStashes: true });
    answers[1]();
    await last;
    expect(view.viewOptions).toEqual({ branches: ["main"], showRemotes: false, showTags: false, showStashes: true });
  });

  it("keeps the selected commit selected and at the same place on screen", async () => {
    const view = await openView(26 * 480);
    view.select("c490", 490); // 260 px below the top
    vi.mocked(ipc.api.setViewOptions).mockImplementation(async (_id, options) => {
      core.generation++;
      core.hashes = core.hashes.filter((h) => Number(h.slice(1)) % 2 === 0); // c490 → row 245
      return snapshot({ viewOptions: options });
    });
    await view.setViewOptions({ branches: ["main"] });
    await vi.waitFor(() => expect(view.snapshot.generation).toBe(2));
    expect(view.selectedHash).toBe("c490");
    expect(view.selectedIndex).toBe(245);
    expect(view.scrollTop).toBe(245 * 26 - 260);
  });

  it("brings a selected commit that was out of view into view", async () => {
    const view = await openView(26 * 100);
    view.select("c900", 900);
    vi.mocked(ipc.api.setViewOptions).mockImplementation(async (_id, options) => {
      core.generation++;
      core.hashes = core.hashes.filter((h) => Number(h.slice(1)) % 2 === 0); // c900 → row 450
      return snapshot({ viewOptions: options });
    });
    await view.setViewOptions({ showTags: false });
    await vi.waitFor(() => expect(view.snapshot.generation).toBe(2));
    expect(view.selectedIndex).toBe(450);
    expect(450 * 26).toBeGreaterThanOrEqual(view.scrollTop);
    expect(451 * 26).toBeLessThanOrEqual(view.scrollTop + view.viewportH);
  });

  it("re-applies remembered options on open and shows no other rows before them", async () => {
    const stored: ViewOptions = { branches: ["feature"], showRemotes: true, showTags: true, showStashes: true };
    localStorage.setItem("gg.view.v1:/r", JSON.stringify(stored));
    // The core loads the filtered rows as a new generation; until then it serves the old ones.
    vi.mocked(ipc.api.setViewOptions).mockImplementation(async () => snapshot({ viewOptions: stored, loading: true }));
    const view = repo.repoView(7, snapshot());
    const shown: (string | undefined)[] = [];
    view.onChange = () => shown.push(view.cache.get(0)?.hash);
    expect(view.viewOptions).toEqual(stored);
    await settle();
    await settle();
    expect(ipc.api.setViewOptions).toHaveBeenCalledWith(7, stored);
    expect(view.snapshot.totalRows).toBe(0);

    core.generation++;
    core.hashes = Array.from({ length: 40 }, (_, i) => `f${i}`);
    core.handler?.(snapshot({ viewOptions: stored }));
    await vi.waitFor(() => expect(view.snapshot.generation).toBe(2));
    await view.cache.ensure(0, 0);
    expect(view.cache.get(0)?.hash).toBe("f0");
    expect(shown.filter((h) => h !== undefined && !h.startsWith("f"))).toEqual([]);
  });

  it("asks nothing when the core already has the remembered options", async () => {
    localStorage.setItem("gg.view.v1:/r", JSON.stringify(ALL));
    const view = repo.repoView(7, snapshot());
    await settle();
    expect(ipc.api.setViewOptions).not.toHaveBeenCalled();
    expect(view.snapshot.totalRows).toBe(1000);
  });

  it("reads options remembered before stashes were shown as showing them", async () => {
    localStorage.setItem("gg.view.v1:/r", JSON.stringify({ branches: null, showRemotes: true, showTags: true }));
    expect(readOptions("/r")).toEqual(ALL);
    localStorage.setItem("gg.view.v1:/r", JSON.stringify({ ...ALL, showStashes: "no" }));
    expect(readOptions("/r")).toBeNull();
    expect(sameOptions(ALL, { ...ALL, showStashes: false })).toBe(false);
  });

  it("shows the rows anyway when the remembered options cannot be applied", async () => {
    localStorage.setItem("gg.view.v1:/r", JSON.stringify({ ...ALL, showTags: false }));
    vi.mocked(ipc.api.setViewOptions).mockRejectedValue(
      new ipc.CoreError({ code: "git_failed", message: "boom", detail: null }),
    );
    const view = repo.repoView(7, snapshot());
    const notice = vi.fn();
    view.onNotice = notice;
    await vi.waitFor(() => expect(view.snapshot.totalRows).toBe(1000));
    expect(notice).toHaveBeenCalled();
    // What the user chose for this repository is kept for the next time.
    expect(JSON.parse(localStorage.getItem("gg.view.v1:/r")!)).toEqual({ ...ALL, showTags: false });
    expect(view.viewOptions).toEqual(ALL);
  });
});

describe("RepoView search while loading", () => {
  it("looks the hits up again as rows arrive and once loading ends, one lookup at a time", async () => {
    vi.useFakeTimers();
    try {
      const wait = (ms = 250) => vi.advanceTimersByTimeAsync(ms);
      const loading = (totalRows: number) => snapshot({ loading: true, totalRows });
      vi.mocked(ipc.api.getSnapshot).mockImplementation(async () => loading(300));
      const view = repo.repoView(7, loading(300));
      await wait(0);
      const answers: ((hits: number[]) => void)[] = [];
      vi.mocked(ipc.api.searchCommits).mockImplementation(() => new Promise((r) => answers.push(r)));
      const first = view.search.run("fix");
      answers[0]([3]);
      await first;
      expect(view.search.hits).toEqual([3]);

      core.handler?.(loading(500)); // more rows: look again
      await wait();
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(2);
      core.handler?.(loading(700)); // while that lookup runs: one more afterwards, not one each
      core.handler?.(loading(800));
      await wait();
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(2);
      answers[1]([3, 400]);
      await wait();
      expect(view.search.hits).toEqual([3, 400]);
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(3);
      answers[2]([3, 400, 750]);
      await wait();

      core.handler?.(snapshot({ loading: false, totalRows: 1000 })); // loaded: a last lookup
      await wait(0);
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(4);
      answers[3]([3, 400, 750, 900]);
      await wait();
      expect(view.search.hits).toEqual([3, 400, 750, 900]);
      expect(view.selectedHash).toBe("c3"); // the selection stays

      core.handler?.(snapshot({ loading: false, totalRows: 1000 })); // nothing new: no lookup
      await wait();
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(4);
    } finally {
      vi.useRealTimers();
    }
  });

  it("spaces the lookups made while rows load 250 ms apart; the last one, once loaded, at once", async () => {
    vi.useFakeTimers();
    try {
      const loading = (totalRows: number) => snapshot({ loading: true, totalRows });
      vi.mocked(ipc.api.getSnapshot).mockImplementation(async () => loading(300));
      const view = repo.repoView(7, loading(300));
      await vi.advanceTimersByTimeAsync(0);
      vi.mocked(ipc.api.searchCommits).mockResolvedValue([3]);
      await view.search.run("fix");
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(1);

      core.handler?.(loading(500));
      await vi.advanceTimersByTimeAsync(100);
      core.handler?.(loading(600));
      await vi.advanceTimersByTimeAsync(149);
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(2);

      core.handler?.(loading(700));
      await vi.advanceTimersByTimeAsync(10);
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(2);
      core.handler?.(snapshot({ loading: false, totalRows: 1000 }));
      await vi.advanceTimersByTimeAsync(0);
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(3);
      await vi.advanceTimersByTimeAsync(500);
      expect(ipc.api.searchCommits).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the same hit set while only load progress changes", async () => {
    const view = await openView(0);
    vi.mocked(ipc.api.searchCommits).mockResolvedValue([3, 10]);
    await view.search.run("fix");
    const set = view.search.hitSet;
    expect([...set]).toEqual([3, 10]);
    await view.applySnapshot(snapshot({ totalRows: 1000 }));
    expect(view.search.hitSet).toBe(set);
  });
});

describe("RepoView perf hook", () => {
  it("tells a listener each new generation shown after the first full load", async () => {
    const view = await openView(0);
    const shown = vi.fn();
    view.onGenerationShown = shown;
    commitOnTop(1);
    await view.applySnapshot(snapshot());
    expect(shown).toHaveBeenCalledWith(2);
    await view.applySnapshot(snapshot({ totalRows: core.hashes.length }));
    expect(shown).toHaveBeenCalledTimes(1);
  });
});

describe("RepoView fetch and refresh", () => {
  it("refresh applies its answer only if no event came meanwhile; else asks again", async () => {
    const view = await openView(0);
    let answer: (s: RepoSnapshot) => void = () => {};
    vi.mocked(ipc.api.refresh).mockImplementation(() => new Promise((r) => (answer = r)));
    const done = view.refresh();
    const fetched = { running: false, lastError: "fatal: x", lastErrorDetail: "fatal: x\n", lastAtMs: 5 };
    const latest = snapshot({ fetch: fetched });
    core.handler?.(latest);
    await settle();
    vi.mocked(ipc.api.getSnapshot).mockClear().mockImplementation(async () => latest);
    answer(snapshot()); // built before the event
    await done;
    expect(view.snapshot.fetch).toEqual(fetched);
    expect(ipc.api.getSnapshot).toHaveBeenCalled();
  });

  it("fetch does nothing (and shows nothing) without remotes", async () => {
    vi.mocked(ipc.api.getSnapshot).mockImplementation(async () => snapshot({ remotes: [] }));
    const view = repo.repoView(7, snapshot({ remotes: [] }));
    await settle();
    await view.fetch();
    expect(view.fetching).toBe(false);
    expect(ipc.api.fetch).not.toHaveBeenCalled();
  });

  it("a fetch that cannot start is reported like a failed one", async () => {
    const view = await openView(0);
    const notice = vi.fn();
    view.onNotice = notice;
    vi.mocked(ipc.api.fetch).mockRejectedValue(new ipc.CoreError({ code: "io", message: "boom", detail: "x" }));
    await view.fetch();
    expect(view.fetching).toBe(false);
    expect(notice).toHaveBeenCalledWith("Fetch failed", "boom\nx");
  });

  it("refresh shows progress and applies the snapshot it returns", async () => {
    const view = await openView(0);
    commitOnTop(1);
    vi.mocked(ipc.api.refresh).mockImplementation(async () => snapshot());
    const done = view.refresh();
    expect(view.refreshing).toBe(true);
    await done;
    await vi.waitFor(() => expect(view.snapshot.generation).toBe(2));
    await vi.waitFor(() => expect(view.refreshing).toBe(false), { timeout: 2000 });
  });

  it("fetch shows progress at once and reports the failure of the fetch it started", async () => {
    const view = await openView(0);
    const notice = vi.fn();
    view.onNotice = notice;
    const done = view.fetch();
    expect(view.fetching).toBe(true);
    await done;
    expect(ipc.api.fetch).toHaveBeenCalledWith(7);
    core.handler?.(snapshot({ fetch: { running: true, lastError: null, lastErrorDetail: null, lastAtMs: null } }));
    await settle();
    expect(view.fetching).toBe(true);
    core.handler?.(
      snapshot({
        fetch: {
          running: false,
          lastError: "fatal: no such remote",
          lastErrorDetail: "From /o\n   a..b  main -> origin/main\nfatal: no such remote\n",
          lastAtMs: Date.now() + 1,
        },
      }),
    );
    await settle();
    expect(view.fetching).toBe(false);
    expect(notice).toHaveBeenCalledWith(
      "Fetch failed: fatal: no such remote",
      "From /o\n   a..b  main -> origin/main\nfatal: no such remote\n",
    );
  });

  it("does not report a failed background fetch", async () => {
    const view = await openView(0);
    const notice = vi.fn();
    view.onNotice = notice;
    core.handler?.(snapshot({ fetch: { running: false, lastError: "fatal: offline", lastErrorDetail: null, lastAtMs: Date.now() } }));
    await settle();
    expect(notice).not.toHaveBeenCalled();
    expect(view.snapshot.fetch.lastError).toBe("fatal: offline");
  });
});

describe("RepoView.recheck", () => {
  it("looks at the repository again at most once a second", async () => {
    vi.useFakeTimers({ toFake: ["performance"] });
    try {
      vi.mocked(ipc.api.refresh).mockImplementation(async () => snapshot());
      const view = await openView(0);
      view.recheck();
      view.recheck();
      await settle();
      expect(ipc.api.refresh).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(999);
      view.recheck();
      expect(ipc.api.refresh).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(2);
      view.recheck();
      await settle();
      expect(ipc.api.refresh).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
