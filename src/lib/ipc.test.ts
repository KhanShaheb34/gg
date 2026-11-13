import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, CoreError, onEvent, onFolderDrop } from "./ipc";

const DEV_URL = "http://127.0.0.1:1421";

function respond(body: unknown) {
  return vi.fn(async () => new Response(JSON.stringify(body), { status: 200 }));
}

describe("HTTP transport", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("posts {cmd, args} to /invoke and returns ok", async () => {
    const page = { generation: 3, start: 10, rows: [] };
    const fetchMock = respond({ ok: page });
    vi.stubGlobal("fetch", fetchMock);

    await expect(api.getRows(7, 3, 10, 50)).resolves.toEqual(page);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe(`${DEV_URL}/invoke`);
    expect(init.method).toBe("POST");
    expect(init.headers).toEqual({ "content-type": "application/json" });
    expect(JSON.parse(init.body as string)).toEqual({
      cmd: "get_rows",
      args: { repoId: 7, generation: 3, start: 10, count: 50 },
    });
  });

  it("rejects with CoreError on {err}", async () => {
    vi.stubGlobal("fetch", respond({ err: { code: "not_a_repo", message: "m", detail: null } }));

    const error = await api.openRepo("/nope").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("not_a_repo");
    expect((error as CoreError).message).toBe("m");
    expect((error as CoreError).detail).toBeNull();
  });

  it("turns an unreachable server into an io CoreError", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      }),
    );

    const error = await api.getAppState().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("io");
    expect((error as CoreError).detail).toContain("Failed to fetch");
  });

  it("returns undefined for commands that return null", async () => {
    const fetchMock = respond({ ok: null });
    vi.stubGlobal("fetch", fetchMock);

    await expect(api.saveSession(["/a"], "/a")).resolves.toBeUndefined();
    await expect(api.setWindowFocused(true)).resolves.toBeUndefined();
    const bodies = fetchMock.mock.calls.map((c) => JSON.parse((c as unknown as [string, RequestInit])[1].body as string));
    expect(bodies).toEqual([
      { cmd: "save_session", args: { tabs: ["/a"], active: "/a" } },
      { cmd: "set_window_focused", args: { focused: true } },
    ]);
  });

  it("names every command's arguments as the core expects", async () => {
    const fetchMock = respond({ ok: null });
    vi.stubGlobal("fetch", fetchMock);
    const options = { branches: null, showRemotes: true, showTags: false };

    await api.updateSettings({ theme: "dark" });
    await api.removeRecent("/r");
    await api.closeRepo(1);
    await api.getSnapshot(1);
    await api.findRow(1, "abc");
    await api.searchCommits(1, "fix");
    await api.commitDetails(1, "abc");
    await api.setViewOptions(1, options);
    await api.fetch(1);
    await api.refresh(1);
    await api.perfMark("first-rows", 12.5);
    await api.commandLog(4);

    const bodies = fetchMock.mock.calls.map((c) => JSON.parse((c as unknown as [string, RequestInit])[1].body as string));
    expect(bodies).toEqual([
      { cmd: "update_settings", args: { patch: { theme: "dark" } } },
      { cmd: "remove_recent", args: { path: "/r" } },
      { cmd: "close_repo", args: { repoId: 1 } },
      { cmd: "get_snapshot", args: { repoId: 1 } },
      { cmd: "find_row", args: { repoId: 1, hash: "abc" } },
      { cmd: "search_commits", args: { repoId: 1, query: "fix" } },
      { cmd: "commit_details", args: { repoId: 1, hash: "abc" } },
      { cmd: "set_view_options", args: { repoId: 1, options } },
      { cmd: "fetch", args: { repoId: 1 } },
      { cmd: "refresh", args: { repoId: 1 } },
      { cmd: "perf_mark", args: { name: "first-rows", ms: 12.5 } },
      { cmd: "command_log", args: { sinceId: 4 } },
    ]);
  });
});

/** Minimal EventSource stand-in: records instances and lets tests push messages. */
class FakeEventSource {
  static instances: FakeEventSource[] = [];
  static readonly CLOSED = 2;
  readyState = 1;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) {
    FakeEventSource.instances.push(this);
  }
  close() {
    this.readyState = FakeEventSource.CLOSED;
  }
  push(name: string, payload: unknown) {
    this.onmessage?.({ data: JSON.stringify({ name, payload }) });
  }
}

describe("browser events", () => {
  beforeEach(() => {
    FakeEventSource.instances = [];
    vi.stubGlobal("EventSource", FakeEventSource);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shares one EventSource and routes events by name", () => {
    const changed: unknown[] = [];
    const errors: unknown[] = [];
    const offChanged = onEvent("repo-changed", (p) => changed.push(p));
    const offError = onEvent("repo-error", (p) => errors.push(p));

    expect(FakeEventSource.instances).toHaveLength(1);
    const source = FakeEventSource.instances[0];
    expect(source.url).toBe(`${DEV_URL}/events`);
    source.push("repo-changed", { repoId: 1 });
    source.push("repo-error", { repoId: 1, error: { code: "io", message: "x", detail: null } });
    expect(changed).toEqual([{ repoId: 1 }]);
    expect(errors).toHaveLength(1);

    offChanged();
    source.push("repo-changed", { repoId: 2 });
    expect(changed).toHaveLength(1);

    // The last handler gone, the connection closes; a new handler opens a fresh one.
    offError();
    expect(source.readyState).toBe(FakeEventSource.CLOSED);
    const off = onEvent("repo-changed", (p) => changed.push(p));
    expect(FakeEventSource.instances).toHaveLength(2);
    off();
  });

  it("reconnects when the connection is closed for good", () => {
    vi.useFakeTimers();
    try {
      const got: unknown[] = [];
      const off = onEvent("repo-changed", (p) => got.push(p));
      const first = FakeEventSource.instances[0];
      first.close();
      first.onerror?.();
      vi.advanceTimersByTime(5_000);
      expect(FakeEventSource.instances).toHaveLength(2);
      FakeEventSource.instances[1].push("repo-changed", { repoId: 3 });
      expect(got).toEqual([{ repoId: 3 }]);
      off();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("pickFolder in the browser", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("asks with window.prompt", async () => {
    const prompt = vi.fn(() => "  /some/repo  ");
    vi.stubGlobal("window", { prompt });
    await expect(api.pickFolder()).resolves.toBe("/some/repo");
    expect(prompt).toHaveBeenCalledWith("Repository path");
  });

  it("is null when cancelled or empty", async () => {
    vi.stubGlobal("window", { prompt: vi.fn(() => null) });
    await expect(api.pickFolder()).resolves.toBeNull();
    vi.stubGlobal("window", { prompt: vi.fn(() => "   ") });
    await expect(api.pickFolder()).resolves.toBeNull();
  });
});

describe("dev server URL", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("takes window.__GG_DEV_URL__ over the build-time default", async () => {
    const fetchMock = respond({ ok: null });
    vi.stubGlobal("fetch", fetchMock);
    vi.stubGlobal("window", { __GG_DEV_URL__: "http://127.0.0.1:5555" });
    await api.setWindowFocused(false);
    expect((fetchMock.mock.calls[0] as unknown as [string])[0]).toBe("http://127.0.0.1:5555/invoke");
  });

  it("falls back to the default without an override", async () => {
    const fetchMock = respond({ ok: null });
    vi.stubGlobal("fetch", fetchMock);
    vi.stubGlobal("window", {});
    await api.setWindowFocused(false);
    expect((fetchMock.mock.calls[0] as unknown as [string])[0]).toBe(`${DEV_URL}/invoke`);
  });
});

describe("onFolderDrop in the browser", () => {
  it("is inert: never calls the handler and returns a stop function", () => {
    const handler = vi.fn();
    const stop = onFolderDrop(handler);
    expect(typeof stop).toBe("function");
    stop();
    expect(handler).not.toHaveBeenCalled();
  });
});
