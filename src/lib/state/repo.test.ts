import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CommitRow, RepoSnapshot } from "../types";

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
      getRows: vi.fn(async (_repoId: number, generation: number, start: number, count: number) => {
        if (generation !== core.generation) {
          throw new actual.CoreError({ code: "stale_generation", message: "stale", detail: null });
        }
        const rows = core.hashes.slice(start, start + count).map((hash, i) => row(start + i, hash));
        return { generation, start, rows };
      }),
      findRow: vi.fn(async (_repoId: number, hash: string) => {
        const i = core.hashes.indexOf(hash);
        return i < 0 ? null : i;
      }),
      perfMark: vi.fn(async () => {}),
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
    remotes: [],
    worktrees: [],
    status: { changed: 0, untracked: 0, conflicted: 0 },
    viewOptions: { branches: null, showRemotes: true, showTags: true },
    fetch: { running: false, lastError: null, lastAtMs: null },
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

beforeEach(async () => {
  vi.resetModules();
  core.generation = 1;
  core.hashes = Array.from({ length: 1000 }, (_, i) => `c${i}`);
  ipc = await import("../ipc");
  repo = await import("./repo.svelte");
  vi.mocked(ipc.api.getSnapshot).mockReset().mockImplementation(async () => snapshot());
  vi.mocked(ipc.api.getRows).mockClear();
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

  it("gives up after a few failed attempts and keeps the old rows", async () => {
    const view = await openView(26 * 50);
    const cacheBefore = view.cache;
    commitOnTop(1);
    vi.mocked(ipc.api.getRows).mockImplementation(async () => {
      throw new ipc.CoreError({ code: "stale_generation", message: "stale", detail: null });
    });
    await view.applySnapshot(snapshot());
    await new Promise((r) => setTimeout(r, 20));
    expect(view.cache).toBe(cacheBefore);
    expect(view.snapshot.generation).toBe(1);
    expect(vi.mocked(ipc.api.getSnapshot).mock.calls.length).toBeLessThan(8);
  });
});
