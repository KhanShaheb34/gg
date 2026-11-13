import { describe, expect, it, vi } from "vitest";
import type { CommitRow, RowsPage } from "../types";
import { RowCache } from "./rowCache";

function row(index: number): CommitRow {
  return {
    index,
    kind: "commit",
    hash: `h${index}`,
    parents: [],
    author: "Ada",
    email: "ada@example.com",
    time: 0,
    subject: `Commit ${index}`,
    lane: 0,
    color: 0,
    edges: [],
    labels: [],
    isHead: false,
    isMerge: false,
    detachedWorktrees: [],
  };
}

/** A fake core: records requests; each resolves when the test says so. */
function fakeCore(totalRows: () => number) {
  const requests: { generation: number; start: number; count: number; resolve: () => void; reject: (e: unknown) => void }[] = [];
  const fetchPage = vi.fn(
    (generation: number, start: number, count: number) =>
      new Promise<RowsPage>((resolve, reject) => {
        requests.push({
          generation,
          start,
          count,
          resolve: () => {
            const end = Math.min(start + count, totalRows());
            const rows = [];
            for (let i = start; i < end; i++) rows.push(row(i));
            resolve({ generation, start, rows });
          },
          reject,
        });
      }),
  );
  /** Resolves every pending request and lets the cache handle the results. */
  const flush = async () => {
    for (const r of requests.splice(0)) r.resolve();
    await new Promise((r) => setTimeout(r, 0));
  };
  return { fetchPage, requests, flush };
}

describe("RowCache", () => {
  it("loads the pages covering the range plus one page either side, clipped at 0", async () => {
    const core = fakeCore(() => 5000);
    const cache = new RowCache(core.fetchPage);
    cache.reset(1, 5000);
    void cache.ensure(0, 50);
    expect(core.requests.map((r) => [r.generation, r.start, r.count])).toEqual([
      [1, 0, 200],
      [1, 200, 200],
    ]);
  });

  it("does not ask again for pages already in flight", () => {
    const core = fakeCore(() => 5000);
    const cache = new RowCache(core.fetchPage);
    cache.reset(1, 5000);
    void cache.ensure(0, 50);
    void cache.ensure(0, 50);
    expect(core.fetchPage).toHaveBeenCalledTimes(2);
  });

  it("serves rows once loaded and reports the update", async () => {
    const core = fakeCore(() => 5000);
    const cache = new RowCache(core.fetchPage);
    const onUpdate = vi.fn();
    cache.onUpdate = onUpdate;
    cache.reset(1, 5000);
    expect(cache.get(10)).toBeUndefined();
    const done = cache.ensure(0, 50);
    await core.flush();
    await done;
    expect(cache.get(10)?.subject).toBe("Commit 10");
    expect(cache.get(399)?.hash).toBe("h399");
    expect(cache.get(400)).toBeUndefined();
    expect(onUpdate).toHaveBeenCalled();
    // Loaded pages are not asked for again.
    void cache.ensure(0, 50);
    expect(core.fetchPage).toHaveBeenCalledTimes(2);
  });

  it("drops everything on a new generation and discards results for the old one", async () => {
    const core = fakeCore(() => 5000);
    const cache = new RowCache(core.fetchPage);
    cache.reset(1, 5000);
    await Promise.all([cache.ensure(0, 10), core.flush()]);
    expect(cache.get(0)).toBeDefined();

    void cache.ensure(1000, 1010); // in flight for generation 1
    cache.reset(2, 5000);
    expect(cache.get(0)).toBeUndefined();
    await core.flush();
    expect(cache.get(1000)).toBeUndefined();

    // The same pages are asked for again under the new generation.
    void cache.ensure(1000, 1010);
    expect(core.requests.every((r) => r.generation === 2)).toBe(true);
    expect(core.requests.length).toBeGreaterThan(0);
  });

  it("keeps its pages when only the row count grows, and completes a partial last page", async () => {
    let total = 250;
    const core = fakeCore(() => total);
    const cache = new RowCache(core.fetchPage);
    cache.reset(1, total);
    await Promise.all([cache.ensure(0, 249), core.flush()]);
    expect(cache.get(249)).toBeDefined();
    expect(core.fetchPage).toHaveBeenCalledTimes(2);

    total = 600;
    cache.reset(1, total);
    expect(cache.get(10)).toBeDefined();
    void cache.ensure(0, 249);
    // Page 0 is complete; page 1 (200..399) held only 50 rows and is asked for again; page 2 is new.
    expect(core.requests.map((r) => r.start)).toEqual([200, 400]);
  });

  it("evicts the least recently used page beyond maxPages", async () => {
    const core = fakeCore(() => 1000);
    const cache = new RowCache(core.fetchPage, 10, 2);
    cache.reset(1, 1000);
    await Promise.all([cache.ensure(0, 5), core.flush()]); // pages 0 and 1
    await Promise.all([cache.ensure(15, 15), core.flush()]); // touches 0 and 1, loads 2
    expect(cache.get(5)).toBeUndefined(); // page 0 was the least recently used
    expect(cache.get(15)).toBeDefined();
    expect(cache.get(25)).toBeDefined();
  });

  it("clips ranges past the last row", () => {
    const core = fakeCore(() => 300);
    const cache = new RowCache(core.fetchPage);
    cache.reset(1, 300);
    void cache.ensure(250, 5000);
    expect(core.requests.map((r) => r.start)).toEqual([0, 200]);
    cache.reset(2, 0);
    void cache.ensure(0, 10);
    expect(core.requests).toHaveLength(2);
  });

  it("forgets a page whose request failed, so it can be asked for again", async () => {
    const core = fakeCore(() => 5000);
    const cache = new RowCache(core.fetchPage);
    cache.onError = vi.fn();
    cache.reset(1, 5000);
    const done = cache.ensure(0, 10);
    for (const r of core.requests.splice(0)) r.reject(new Error("boom"));
    await done; // never rejects
    expect(cache.onError).toHaveBeenCalled();
    void cache.ensure(0, 10);
    expect(core.fetchPage).toHaveBeenCalledTimes(4);
  });

  it("tells whether every row of a range is loaded", async () => {
    const core = fakeCore(() => 5000);
    const cache = new RowCache(core.fetchPage);
    cache.reset(1, 5000);
    expect(cache.loaded(0, 10)).toBe(false);
    await Promise.all([cache.ensure(0, 10), core.flush()]);
    expect(cache.loaded(0, 399)).toBe(true);
    expect(cache.loaded(390, 410)).toBe(false);
    // Clipped to the rows there are.
    cache.reset(1, 300);
    expect(cache.loaded(250, 900)).toBe(true);
  });
});
