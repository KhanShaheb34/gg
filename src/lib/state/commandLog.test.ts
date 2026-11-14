import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CommandRecord } from "../types";

// A fake core: the log it keeps, and the calls it was asked.
const core = vi.hoisted(() => ({
  records: [] as CommandRecord[],
  calls: [] as { sinceId: number; repoId?: number; writesOnly?: boolean }[],
  repoChanged: null as ((payload: { repoId: number }) => void) | null,
}));

vi.mock("../ipc", () => ({
  api: {
    commandLog: vi.fn(async (sinceId: number, options: { repoId?: number; writesOnly?: boolean } = {}) => {
      core.calls.push({ sinceId, ...options });
      return core.records.filter(
        (r) =>
          r.id > sinceId &&
          (options.repoId === undefined || r.repoId === options.repoId) &&
          (!options.writesOnly || r.kind === "write"),
      );
    }),
  },
  onEvent: vi.fn((name: string, handler: (payload: { repoId: number }) => void) => {
    if (name === "repo-changed") core.repoChanged = handler;
    return () => {
      if (core.repoChanged === handler) core.repoChanged = null;
    };
  }),
}));

import {
  CommandLogStore,
  LOG_REFRESH_MS,
  MAX_KEPT,
  logPrefs,
  readLogPrefs,
  saveLogPrefs,
} from "./commandLog.svelte";

let nextId = 1;
function rec(overrides: Partial<CommandRecord> = {}): CommandRecord {
  return {
    id: nextId++,
    kind: "write",
    cwd: "/r",
    args: ["status"],
    startedAtMs: 1_700_000_000_000,
    durationMs: 5,
    exitCode: 0,
    stdout: "",
    stderr: "",
    repoId: 1,
    opId: null,
    opTitle: null,
    ...overrides,
  };
}

beforeEach(() => {
  nextId = 1;
  core.records = [];
  core.calls = [];
  core.repoChanged = null;
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("CommandLogStore", () => {
  it("fetches only what is new since the last id it has", async () => {
    const store = new CommandLogStore();
    core.records.push(rec(), rec());
    store.show(1, true);
    await store.fetchNow();
    expect(core.calls).toEqual([{ sinceId: 0, repoId: 1, writesOnly: true }]);
    expect(store.count).toBe(2);

    core.records.push(rec());
    await store.fetchNow();
    expect(core.calls[1]).toEqual({ sinceId: 2, repoId: 1, writesOnly: true });
    expect(store.count).toBe(3);
    expect(store.lastId).toBe(3);
  });

  it("groups an operation's commands under its title, whenever they arrive", async () => {
    const store = new CommandLogStore();
    store.show(1, false);
    core.records.push(
      rec({ opId: 7, opTitle: "Merge feature/x into main", args: ["merge", "feature/x"] }),
      rec({ kind: "read", args: ["status"] }),
      rec({ opId: 7, opTitle: "Merge feature/x into main", args: ["commit"] }),
    );
    await store.fetchNow();
    core.records.push(rec({ opId: 7, opTitle: "Merge feature/x into main", args: ["add", "a.txt"] }));
    await store.fetchNow();

    expect(store.items.map((i) => i.kind)).toEqual(["op", "cmd"]);
    const op = store.items[0];
    expect(op.kind === "op" && op.title).toBe("Merge feature/x into main");
    expect(op.kind === "op" && op.records.map((r) => r.args[0])).toEqual(["merge", "commit", "add"]);
  });

  it("takes an operation's title from a later record when the first had none", async () => {
    const store = new CommandLogStore();
    store.show(1, false);
    core.records.push(rec({ kind: "read", opId: 3, opTitle: null }));
    await store.fetchNow();
    core.records.push(rec({ opId: 3, opTitle: "Push main to origin" }));
    await store.fetchNow();
    const op = store.items[0];
    expect(op.kind === "op" && op.title).toBe("Push main to origin");
  });

  it("keeps at most MAX_KEPT commands, dropping the oldest entries whole", async () => {
    const store = new CommandLogStore();
    store.show(1, false);
    core.records.push(rec({ opId: 1, opTitle: "First" }), rec({ opId: 1, opTitle: "First" }));
    for (let i = 0; i < MAX_KEPT; i++) core.records.push(rec({ kind: "read" }));
    await store.fetchNow();
    expect(store.count).toBeLessThanOrEqual(MAX_KEPT);
    expect(store.items[0].kind).toBe("cmd"); // the operation went, both its commands
    expect(store.items.at(-1)?.kind === "cmd" && store.items.at(-1)?.key).toBe(`c${MAX_KEPT + 2}`);
    expect(store.find(1)).toBeNull();
  });

  it("finds the entry holding a command id (Show log)", async () => {
    const store = new CommandLogStore();
    store.show(1, true);
    core.records.push(rec(), rec({ opId: 4, opTitle: "Check out x" }), rec({ opId: 4, opTitle: "Check out x" }));
    await store.fetchNow();
    expect(store.find(3)).toMatchObject({ kind: "op", opId: 4 });
    expect(store.find(2)).toMatchObject({ kind: "op", opId: 4 });
    expect(store.find(99)).toBeNull();
  });

  it("starts over for another repository or filter", async () => {
    const store = new CommandLogStore();
    core.records.push(rec(), rec({ repoId: 2 }), rec({ kind: "read" }));
    store.show(1, true);
    await store.fetchNow();
    expect(store.count).toBe(1);
    store.show(1, false);
    expect(store.count).toBe(0);
    await store.fetchNow();
    expect(store.count).toBe(2);
    store.show(2, false);
    await store.fetchNow();
    expect(store.count).toBe(1);
    expect(core.calls.at(-1)).toEqual({ sinceId: 0, repoId: 2, writesOnly: false });
  });

  it("listens only while started, and asks at most once a second", async () => {
    const store = new CommandLogStore();
    store.show(1, true);
    expect(core.repoChanged).toBeNull();
    const stop = store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(core.calls).toHaveLength(1); // on starting
    expect(core.repoChanged).not.toBeNull();

    core.repoChanged!({ repoId: 2 }); // another repository: nothing
    core.repoChanged!({ repoId: 1 });
    core.repoChanged!({ repoId: 1 });
    core.repoChanged!({ repoId: 1 });
    await vi.advanceTimersByTimeAsync(LOG_REFRESH_MS - 10);
    expect(core.calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(20);
    expect(core.calls).toHaveLength(2);

    stop();
    expect(core.repoChanged).toBeNull();
    core.records.push(rec());
    await vi.advanceTimersByTimeAsync(5 * LOG_REFRESH_MS);
    expect(core.calls).toHaveLength(2);
  });

  it("drops an answer that comes back after it started over", async () => {
    const store = new CommandLogStore();
    core.records.push(rec({ repoId: 1 }), rec({ repoId: 2 }));
    store.show(1, true);
    const late = store.fetchNow();
    store.show(2, true);
    await late;
    await store.fetchNow();
    expect(store.items.map((i) => (i.kind === "cmd" ? i.record.repoId : null))).toEqual([2]);
  });
});

describe("panel preferences", () => {
  it("are read back as saved, with defaults for anything odd", () => {
    const items = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => items.get(k) ?? null,
      setItem: (k: string, v: string) => void items.set(k, String(v)),
      removeItem: (k: string) => void items.delete(k),
    });
    expect(readLogPrefs()).toEqual({ open: false, height: null, reads: false });
    logPrefs.open = true;
    logPrefs.height = 210;
    logPrefs.reads = true;
    saveLogPrefs();
    expect(readLogPrefs()).toEqual({ open: true, height: 210, reads: true });
    localStorage.setItem("gg.log.v1", '{"open":"yes","height":-4}');
    expect(readLogPrefs()).toEqual({ open: false, height: null, reads: false });
    vi.unstubAllGlobals();
  });
});
