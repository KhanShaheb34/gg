import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CommitDetails, CommitRow } from "../types";

// A fake core whose answers the test hands out, in any order.
const core = vi.hoisted(() => ({
  pending: [] as { hash: string; resolve: (d: unknown) => void; reject: (e: unknown) => void }[],
}));

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    api: {
      commitDetails: vi.fn(
        (_repoId: number, hash: string) =>
          new Promise((resolve, reject) => core.pending.push({ hash, resolve, reject })),
      ),
    },
  };
});

import { api, CoreError } from "../ipc";
import { CommitDetailsLoader, DETAILS_CACHE_SIZE, DETAILS_DEBOUNCE_MS } from "./details.svelte";

const commitDetails = vi.mocked(api.commitDetails);

/** A full 40-digit hash made from `n`. */
const H = (n: number) => n.toString(16).padStart(40, "0");

function details(hash: string): CommitDetails {
  return {
    hash,
    parents: [],
    author: "Ada",
    authorEmail: "ada@x",
    authorTime: 0,
    committer: "Ada",
    committerEmail: "ada@x",
    commitTime: 0,
    message: `message of ${hash}`,
    files: [],
  };
}

function row(hash: string): CommitRow {
  return {
    index: 0,
    kind: "commit",
    hash,
    parents: [],
    author: "Ada",
    email: "ada@x",
    time: 0,
    subject: `subject of ${hash}`,
    lane: 0,
    color: 0,
    edges: [],
    labels: [],
    isHead: false,
    isMerge: false,
    detachedWorktrees: [],
  };
}

/** Answers the oldest unanswered request for `hash`. */
async function answer(hash: string, result: CommitDetails | Error = details(hash)) {
  const i = core.pending.findIndex((p) => p.hash === hash);
  if (i < 0) throw new Error(`no request for ${hash}`);
  const [p] = core.pending.splice(i, 1);
  if (result instanceof Error) p.reject(result);
  else p.resolve(result);
  await vi.advanceTimersByTimeAsync(0);
}

const requested = () => commitDetails.mock.calls.map((c) => c[1]);

let loader: CommitDetailsLoader;

beforeEach(() => {
  vi.useFakeTimers();
  core.pending = [];
  commitDetails.mockClear();
  loader = new CommitDetailsLoader(3);
});

afterEach(() => {
  loader.dispose();
  vi.useRealTimers();
});

describe("CommitDetailsLoader", () => {
  it("asks the core at once when nothing was asked for just before, and shows the answer", async () => {
    loader.show(H(1), row(H(1)));
    expect(requested()).toEqual([H(1)]);
    expect(commitDetails.mock.calls[0][0]).toBe(3);
    expect(loader.hash).toBe(H(1));
    expect(loader.row?.subject).toBe(`subject of ${H(1)}`);
    expect(loader.details).toBeNull();
    expect(loader.loading).toBe(true);

    await answer(H(1));
    expect(loader.details?.hash).toBe(H(1));
    expect(loader.loading).toBe(false);
  });

  it("never shows the answer for an earlier selection, whichever order the answers come in", async () => {
    loader.show(H(1));
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    loader.show(H(2));
    expect(requested()).toEqual([H(1), H(2)]);

    await answer(H(2));
    expect(loader.details?.hash).toBe(H(2));
    await answer(H(1));
    expect(loader.details?.hash).toBe(H(2));
    expect(loader.hash).toBe(H(2));

    // The late answer was kept, though: going back shows it without asking again.
    loader.show(H(1));
    expect(loader.details?.hash).toBe(H(1));
    expect(requested()).toEqual([H(1), H(2)]);
  });

  it("while the selection moves quickly, asks only for where it stops", async () => {
    loader.show(H(1));
    for (let n = 2; n <= 6; n++) {
      await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS / 3);
      loader.show(H(n), row(H(n)));
      // The row's own information is there at once.
      expect(loader.row?.hash).toBe(H(n));
      expect(loader.details).toBeNull();
    }
    expect(requested()).toEqual([H(1)]);
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS);
    expect(requested()).toEqual([H(1), H(6)]);

    await answer(H(6));
    await answer(H(1));
    expect(loader.details?.hash).toBe(H(6));
  });

  it("shows a remembered commit at once, without asking", async () => {
    loader.show(H(1));
    await answer(H(1));
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    loader.show(H(2));
    await answer(H(2));
    loader.show(H(1));
    expect(loader.details?.hash).toBe(H(1));
    expect(loader.loading).toBe(false);
    expect(requested()).toEqual([H(1), H(2)]);
  });

  it(`remembers the ${DETAILS_CACHE_SIZE} most recently shown commits`, async () => {
    for (let n = 1; n <= DETAILS_CACHE_SIZE + 1; n++) {
      await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
      loader.show(H(n));
      await answer(H(n));
      if (n === DETAILS_CACHE_SIZE) {
        // Seeing commit 2 again makes commit 1 the least recently shown.
        await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
        loader.show(H(2));
      }
    }
    commitDetails.mockClear();
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    loader.show(H(2));
    expect(requested()).toEqual([]);
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    loader.show(H(1));
    expect(requested()).toEqual([H(1)]);
  });

  it("does not ask twice for a commit already asked for", async () => {
    loader.show(H(1));
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    loader.show(H(2));
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    loader.show(H(1));
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    expect(requested()).toEqual([H(1), H(2)]);
    await answer(H(1));
    expect(loader.details?.hash).toBe(H(1));
  });

  it("asks nothing for the uncommitted row or no selection", async () => {
    loader.show(H(1));
    await answer(H(1));
    loader.show("UNCOMMITTED");
    loader.show(null);
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS * 2);
    expect(requested()).toEqual([H(1)]);
    expect(loader.details).toBeNull();
    expect(loader.loading).toBe(false);
    expect(loader.hash).toBeNull();
  });

  it("keeps the row it was given while the same commit stays selected", () => {
    loader.show(H(1), row(H(1)));
    loader.show(H(1));
    expect(loader.row?.hash).toBe(H(1));
    loader.show(H(1), row(H(2))); // not this commit's row: ignored
    expect(loader.row?.hash).toBe(H(1));
  });

  it("reports a failure for the selection only, and tries again when asked", async () => {
    loader.show(H(1));
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    loader.show(H(2));
    await answer(H(1), new CoreError({ code: "git_failed", message: "boom", detail: null }));
    expect(loader.error).toBeNull();
    await answer(H(2), new CoreError({ code: "git_failed", message: "bad object", detail: null }));
    expect(loader.error).toBe("bad object");
    expect(loader.loading).toBe(false);

    loader.retry();
    expect(loader.error).toBeNull();
    expect(loader.loading).toBe(true);
    await vi.advanceTimersByTimeAsync(DETAILS_DEBOUNCE_MS + 1);
    await answer(H(2));
    expect(loader.details?.hash).toBe(H(2));
  });

  it("drops answers after it is disposed of", async () => {
    loader.show(H(1));
    loader.dispose();
    await answer(H(1));
    expect(loader.details).toBeNull();
  });
});
