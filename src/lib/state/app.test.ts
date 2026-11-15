import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppState, CoreErrorDto, OpenedRepo } from "../types";

// The core is mocked; CoreError stays the real class so `instanceof` checks behave.
vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn(() => () => {}),
    onFolderDrop: vi.fn(() => () => {}),
    api: {
      getAppState: vi.fn(),
      saveSession: vi.fn(),
      updateSettings: vi.fn(),
      removeRecent: vi.fn(),
      resetSettings: vi.fn(),
      openRepo: vi.fn(),
      closeRepo: vi.fn(),
      pickFolder: vi.fn(),
      // Each tab's graph view asks for its snapshot; never answered here.
      getSnapshot: vi.fn(() => new Promise(() => {})),
      refresh: vi.fn(() => new Promise(() => {})),
      setWindowFocused: vi.fn(async () => {}),
      takeOpenedPaths: vi.fn(async () => []),
      perfMark: vi.fn(async () => {}),
    },
  };
});

type Ipc = typeof import("../ipc");
type State = typeof import("./app.svelte");

let ipc: Ipc;
let s: State;

function appState(overrides: Partial<AppState> = {}): AppState {
  return {
    recents: [],
    session: { tabs: [], active: null },
    settings: { theme: "system", fetchIntervalSecs: 300, editorCommand: null },
    launchPath: null,
    homeDir: null,
    settingsProblem: null,
    gitProblem: null,
    ...overrides,
  };
}

function snapshot(repoId: number, rootPath: string): OpenedRepo {
  return { repoId, rootPath, activeWorktree: rootPath, name: rootPath.split("/").pop() ?? rootPath, openedWorktree: null } as OpenedRepo;
}

function coreError(code: CoreErrorDto["code"], message: string) {
  return new ipc.CoreError({ code, message, detail: null });
}

/** The core: each path below is a repository with a fixed id; anything else is not. */
const REPOS: Record<string, number> = { "/r/a": 1, "/r/b": 2, "/r/c": 3 };

function fakeOpenRepo(path: string): Promise<OpenedRepo> {
  const root = path.replace(/\/+$/, "");
  const id = REPOS[root];
  if (id === undefined) return Promise.reject(coreError("not_a_repo", "This folder is not a git repository."));
  return Promise.resolve(snapshot(id, root));
}

beforeEach(async () => {
  vi.resetModules();
  ipc = await import("../ipc");
  s = await import("./app.svelte");
  vi.mocked(ipc.api.getAppState).mockReset().mockResolvedValue(appState());
  vi.mocked(ipc.api.saveSession).mockReset().mockResolvedValue(undefined);
  vi.mocked(ipc.api.closeRepo).mockReset().mockResolvedValue(undefined);
  vi.mocked(ipc.api.openRepo).mockReset().mockImplementation(fakeOpenRepo);
  vi.mocked(ipc.api.updateSettings)
    .mockReset()
    .mockImplementation(async (patch) => ({ theme: "system", fetchIntervalSecs: 300, editorCommand: null, ...patch }));
  vi.mocked(ipc.api.removeRecent).mockReset().mockResolvedValue([]);
  vi.mocked(ipc.api.pickFolder).mockReset().mockResolvedValue(null);
  vi.mocked(ipc.api.takeOpenedPaths).mockReset().mockResolvedValue([]);
});

describe("openPath", () => {
  it("adds a tab for the repository and activates it", async () => {
    await s.openPath("/r/a");
    expect(s.app.tabs).toEqual([{ repoId: 1, path: "/r/a", name: "a" }]);
    expect(s.app.activeRepoId).toBe(1);
  });

  it("activates the existing tab when the repository is already open", async () => {
    await s.openPath("/r/a");
    await s.openPath("/r/b");
    await s.openPath("/r/a/");
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 2]);
    expect(s.app.activeRepoId).toBe(1);
  });

  it("says which folder the tab works in when the folder opened is another worktree of it (R42)", async () => {
    await s.openPath("/r/a");
    expect(s.app.toasts).toEqual([]);
    vi.mocked(ipc.api.openRepo).mockResolvedValueOnce({ ...snapshot(1, "/r/a"), openedWorktree: "/r/a-hotfix" });
    await s.openPath("/r/a-hotfix");
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1]);
    expect(s.app.activeRepoId).toBe(1);
    expect(s.app.toasts).toEqual([
      expect.objectContaining({
        kind: "info",
        persistent: true,
        message: "/r/a-hotfix is another worktree of a, which is open already: its tab works in /r/a.",
      }),
    ]);
  });

  it("shows the core's message as an error toast and adds no tab", async () => {
    await s.openPath("/not/a/repo");
    expect(s.app.tabs).toEqual([]);
    expect(s.app.activeRepoId).toBeNull();
    expect(s.app.toasts).toMatchObject([{ kind: "error", message: "This folder is not a git repository." }]);
  });

  it("refreshes recents from the core after opening", async () => {
    const recents = [{ path: "/r/a", name: "a", lastOpenedMs: 1 }];
    vi.mocked(ipc.api.getAppState).mockResolvedValue(appState({ recents }));
    await s.openPath("/r/a");
    expect(s.app.recents).toEqual(recents);
  });

  it("marks git as missing instead of toasting when the core cannot find git", async () => {
    vi.mocked(ipc.api.openRepo).mockRejectedValue(coreError("git_not_found", "git was not found on PATH"));
    await s.openPath("/r/a");
    expect(s.app.gitMissing).toBe(true);
    expect(s.app.toasts).toEqual([]);
  });
});

describe("openViaPicker", () => {
  it("opens the picked folder", async () => {
    vi.mocked(ipc.api.pickFolder).mockResolvedValue("/r/b");
    await s.openViaPicker();
    expect(s.app.activeRepoId).toBe(2);
  });

  it("does nothing when the picker is cancelled", async () => {
    await s.openViaPicker();
    expect(ipc.api.openRepo).not.toHaveBeenCalled();
  });
});

describe("closeTab", () => {
  beforeEach(async () => {
    await s.openPath("/r/a");
    await s.openPath("/r/b");
    await s.openPath("/r/c");
  });

  it("activates the right-hand neighbour of a closed active tab, or the left one at the end", async () => {
    s.activate(2);
    await s.closeTab(2);
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 3]);
    expect(s.app.activeRepoId).toBe(3);
    await s.closeTab(3);
    expect(s.app.activeRepoId).toBe(1);
    await s.closeTab(1);
    expect(s.app.activeRepoId).toBeNull();
    expect(ipc.api.closeRepo).toHaveBeenCalledTimes(3);
  });

  it("closeActiveTab pressed again while the active tab is still closing closes the next one", async () => {
    let release: () => void = () => {};
    vi.mocked(ipc.api.closeRepo).mockImplementationOnce(() => new Promise((r) => (release = () => r(undefined))));
    // The last tab is active: closing it makes its left neighbour active.
    const first = s.closeActiveTab();
    const second = s.closeActiveTab();
    release();
    await Promise.all([first, second]);
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1]);
    expect(s.app.activeRepoId).toBe(1);
  });

  it("keeps the active tab when another one closes", async () => {
    await s.closeTab(1);
    expect(s.app.activeRepoId).toBe(3);
  });

  it("keeps a tab whose repository is busy, and says why plainly", async () => {
    const busy = "An operation is running in b: close it once it has finished.";
    vi.mocked(ipc.api.closeRepo).mockRejectedValueOnce(coreError("busy", busy));
    s.activate(2);
    const saves = vi.mocked(ipc.api.saveSession).mock.calls.length;
    await s.closeTab(2);
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 2, 3]);
    expect(s.app.activeRepoId).toBe(2);
    expect(s.app.toasts).toEqual([expect.objectContaining({ kind: "info", message: busy })]);
    expect(ipc.api.saveSession).toHaveBeenCalledTimes(saves);
  });
});

describe("session", () => {
  it("is saved with the tab paths after every tab change", async () => {
    const save = vi.mocked(ipc.api.saveSession);
    await s.openPath("/r/a");
    expect(save).toHaveBeenLastCalledWith(["/r/a"], "/r/a");
    await s.openPath("/r/b");
    expect(save).toHaveBeenLastCalledWith(["/r/a", "/r/b"], "/r/b");
    s.activate(1);
    expect(save).toHaveBeenLastCalledWith(["/r/a", "/r/b"], "/r/a");
    await s.closeTab(1);
    expect(save).toHaveBeenLastCalledWith(["/r/b"], "/r/b");
    await s.closeTab(2);
    expect(save).toHaveBeenLastCalledWith([], null);
    expect(save).toHaveBeenCalledTimes(5);
  });
});

describe("tab navigation", () => {
  beforeEach(async () => {
    await s.openPath("/r/a");
    await s.openPath("/r/b");
    await s.openPath("/r/c");
  });

  it("cycles through tabs in both directions, wrapping around", () => {
    s.cycleTab(1);
    expect(s.app.activeRepoId).toBe(1);
    s.cycleTab(-1);
    expect(s.app.activeRepoId).toBe(3);
  });

  it("jumps to a tab by position, ignoring positions past the end", () => {
    s.activateIndex(1);
    expect(s.app.activeRepoId).toBe(2);
    s.activateIndex(7);
    expect(s.app.activeRepoId).toBe(2);
  });

  it("index -1 is the last tab", () => {
    s.activateIndex(0);
    s.activateIndex(-1);
    expect(s.app.activeRepoId).toBe(3);
  });
});

describe("init", () => {
  it("restores the session's tabs and active tab, skipping ones that fail with a toast", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a", "/gone", "/r/b"], active: "/r/a" } }),
    );
    await s.init();
    expect(s.app.tabs.map((t) => t.path)).toEqual(["/r/a", "/r/b"]);
    expect(s.app.activeRepoId).toBe(1);
    expect(s.app.toasts).toHaveLength(1);
    expect(s.app.toasts[0].message).toContain("/gone");
    expect(ipc.api.saveSession).toHaveBeenLastCalledWith(["/r/a", "/r/b"], "/r/a");
    expect(s.app.ready).toBe(true);
  });

  it("says once, plainly, that the git found is too old (M3)", async () => {
    const said = "Git 2.29 or newer is needed; found 2.25.1. Some actions and the background fetch fail with it: install a newer git, then restart the app.";
    vi.mocked(ipc.api.getAppState).mockResolvedValue(appState({ gitProblem: said }));
    await s.init();
    expect(s.app.toasts).toEqual([expect.objectContaining({ kind: "error", message: said, persistent: true })]);
  });

  it("is ready, and shows each tab in the saved order, as its repository opens", async () => {
    const answers = new Map<string, (s: OpenedRepo) => void>();
    vi.mocked(ipc.api.openRepo).mockImplementation((path) => new Promise((r) => answers.set(path, r)));
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a", "/r/b", "/r/c"], active: "/r/b" } }),
    );
    const done = s.init();
    await vi.waitFor(() => expect(answers.size).toBe(3));
    expect(s.app.ready).toBe(true);
    expect(s.app.restoring).toBe(true);
    answers.get("/r/c")!(snapshot(3, "/r/c"));
    await vi.waitFor(() => expect(s.app.tabs.map((t) => t.repoId)).toEqual([3]));
    // The saved active tab is not there yet: nothing else is put in front meanwhile.
    expect(s.app.activeRepoId).toBeNull();
    answers.get("/r/a")!(snapshot(1, "/r/a"));
    await vi.waitFor(() => expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 3]));
    answers.get("/r/b")!(snapshot(2, "/r/b"));
    await done;
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 2, 3]);
    expect(s.app.activeRepoId).toBe(2);
    expect(s.app.restoring).toBe(false);
  });

  it("shows the first tab when the saved active one cannot be reopened", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a", "/gone", "/r/b"], active: "/gone" } }),
    );
    await s.init();
    expect(s.app.activeRepoId).toBe(1);
    expect(s.app.restoring).toBe(false);
  });

  it("a tab the user picks while others are still opening stays active", async () => {
    let answerB: (s: OpenedRepo) => void = () => {};
    vi.mocked(ipc.api.openRepo).mockImplementation((path) =>
      path === "/r/b" ? new Promise((r) => (answerB = r)) : fakeOpenRepo(path),
    );
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a", "/r/b"], active: "/r/b" } }),
    );
    const done = s.init();
    await vi.waitFor(() => expect(s.app.tabs.map((t) => t.repoId)).toEqual([1]));
    s.activate(1);
    answerB(snapshot(2, "/r/b"));
    await done;
    expect(s.app.activeRepoId).toBe(1);
  });

  it("a save while tabs are still reopening keeps the ones not open yet at their places", async () => {
    let answerB: (s: OpenedRepo) => void = () => {};
    vi.mocked(ipc.api.openRepo).mockImplementation((path) =>
      path === "/r/b" ? new Promise((r) => (answerB = r)) : fakeOpenRepo(path),
    );
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a", "/r/b"], active: "/r/b" } }),
    );
    const done = s.init();
    await vi.waitFor(() => expect(s.app.tabs.map((t) => t.repoId)).toEqual([1]));
    await s.openPath("/r/c");
    expect(ipc.api.saveSession).toHaveBeenLastCalledWith(["/r/a", "/r/b", "/r/c"], "/r/c");
    s.activate(1);
    expect(ipc.api.saveSession).toHaveBeenLastCalledWith(["/r/a", "/r/b", "/r/c"], "/r/a");
    answerB(snapshot(2, "/r/b"));
    await done;
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 2, 3]);
    expect(ipc.api.saveSession).toHaveBeenLastCalledWith(["/r/a", "/r/b", "/r/c"], "/r/a");
  });

  it("a launch path that is also a session tab keeps its place in the session", async () => {
    // The session's own open of /r/b answers last, after the launch path's.
    const waiting: ((s: OpenedRepo) => void)[] = [];
    let bCalls = 0;
    vi.mocked(ipc.api.openRepo).mockImplementation((path) => {
      if (path === "/r/c" || (path === "/r/b" && bCalls++ > 0)) return fakeOpenRepo(path);
      return new Promise((r) => waiting.push(() => r(snapshot(REPOS[path], path))));
    });
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/b", "/r/a"], active: "/r/a" }, launchPath: "/r/b" }),
    );
    const done = s.init();
    await vi.waitFor(() => expect(s.app.tabs.map((t) => t.repoId)).toEqual([2]));
    // A save while /r/a and the session's own /r/b are still opening: /r/b once.
    await s.openPath("/r/c");
    expect(ipc.api.saveSession).toHaveBeenLastCalledWith(["/r/b", "/r/a", "/r/c"], "/r/c");
    for (const answer of waiting) answer({} as OpenedRepo);
    await done;
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([2, 1, 3]);
    expect(ipc.api.saveSession).toHaveBeenLastCalledWith(["/r/b", "/r/a", "/r/c"], "/r/c");
  });

  it("finds out at start that git is missing, before anything is opened", async () => {
    vi.mocked(ipc.api.getSnapshot).mockImplementation((repoId: number) =>
      repoId === 0 ? Promise.reject(coreError("git_not_found", "git was not found")) : new Promise(() => {}),
    );
    await s.init();
    expect(s.app.gitMissing).toBe(true);
    expect(s.app.toasts).toEqual([]);
  });

  it("with git there, the start-up check shows nothing", async () => {
    vi.mocked(ipc.api.getSnapshot).mockImplementation((repoId: number) =>
      repoId === 0 ? Promise.reject(coreError("unknown_repo", "no such repository")) : new Promise(() => {}),
    );
    await s.init();
    expect(s.app.gitMissing).toBe(false);
    expect(s.app.toasts).toEqual([]);
  });

  it("opens the launch path after the session and activates it", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a"], active: "/r/a" }, launchPath: "/r/c" }),
    );
    await s.init();
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 3]);
    expect(s.app.activeRepoId).toBe(3);
  });

  it("takes recents and settings from the stored state", async () => {
    const recents = [{ path: "/r/b", name: "b", lastOpenedMs: 5 }];
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ recents, settings: { theme: "dark", fetchIntervalSecs: 60, editorCommand: null } }),
    );
    await s.init();
    expect(s.app.recents).toEqual(recents);
    expect(s.app.settings).toEqual({ theme: "dark", fetchIntervalSecs: 60, editorCommand: null });
  });

  it("shows an error toast and stays usable when the core cannot be reached", async () => {
    vi.mocked(ipc.api.getAppState).mockRejectedValue(coreError("io", "Could not reach the dev server"));
    await s.init();
    expect(s.app.ready).toBe(true);
    expect(s.app.toasts).toMatchObject([{ kind: "error", message: "Could not reach the dev server" }]);
  });

  it("toasts repo-error events prefixed with the tab's name", async () => {
    await s.init();
    await s.openPath("/r/b");
    const onEvent = vi.mocked(ipc.onEvent);
    const call = onEvent.mock.calls.find(([name]) => name === "repo-error");
    expect(call).toBeDefined();
    const handler = call![1] as (p: { repoId: number; error: CoreErrorDto }) => void;
    handler({ repoId: 2, error: { code: "git_failed", message: "fetch failed", detail: "boom" } });
    expect(s.app.toasts).toMatchObject([{ kind: "error", message: "b: fetch failed", detail: "boom" }]);
  });
});

describe("folders the system asks to open", () => {
  it("are opened after the session, the last one shown", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a"], active: "/r/a" } }),
    );
    vi.mocked(ipc.api.takeOpenedPaths).mockResolvedValueOnce(["/r/b", "/r/c"]);
    await s.init();
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([1, 2, 3]);
    expect(s.app.activeRepoId).toBe(3);
  });

  it("are opened when the core says more have come, while the app runs", async () => {
    let announce: () => void = () => {};
    vi.mocked(ipc.onEvent).mockImplementation((name, handler) => {
      if (name === "paths-opened") announce = () => (handler as (p: unknown) => void)({});
      return () => {};
    });
    await s.init();
    vi.mocked(ipc.api.takeOpenedPaths).mockResolvedValueOnce(["/r/b"]);
    announce();
    await vi.waitFor(() => expect(s.app.activeRepoId).toBe(2));
    expect(s.app.tabs.map((t) => t.repoId)).toEqual([2]);
  });
});

describe("window focus", () => {
  it("refreshes only the active repository; the others when they are shown", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(
      appState({ session: { tabs: ["/r/a", "/r/b", "/r/c"], active: "/r/a" } }),
    );
    await s.init();
    // Each refresh ends at once (its outcome is not what this is about).
    vi.mocked(ipc.api.refresh).mockClear().mockRejectedValue(coreError("io", "x"));
    s.windowFocusChanged(true);
    expect(ipc.api.setWindowFocused).toHaveBeenCalledWith(true);
    expect(vi.mocked(ipc.api.refresh).mock.calls).toEqual([[1]]);
    await vi.waitFor(() => expect(ipc.api.refresh).toHaveBeenCalledTimes(1));
    await new Promise((r) => setTimeout(r, 0));
    s.windowFocusChanged(false);
    s.windowFocusChanged(true);
    s.activate(2);
    expect(vi.mocked(ipc.api.refresh).mock.calls.map((c) => c[0])).toEqual([1, 1, 2]);
    // Shown once since the focus came back: not refreshed again.
    s.activate(1);
    s.activate(2);
    expect(vi.mocked(ipc.api.refresh).mock.calls.map((c) => c[0])).toEqual([1, 1, 2]);
    expect(ipc.api.setWindowFocused).toHaveBeenLastCalledWith(true);
  });
});

describe("repo-error before the tab exists", () => {
  it("is shown with the repository's name once its tab is added", async () => {
    await s.init();
    const call = vi.mocked(ipc.onEvent).mock.calls.find(([name]) => name === "repo-error");
    const handler = call![1] as (p: { repoId: number; error: CoreErrorDto }) => void;
    // The core delivers errors held for a repository it opened at start while the UI's
    // open_repo for it is still being answered.
    vi.mocked(ipc.api.openRepo).mockImplementation(async (path) => {
      handler({ repoId: 3, error: { code: "git_failed", message: "Could not read the history", detail: "fatal: x" } });
      return fakeOpenRepo(path);
    });
    await s.openPath("/r/c");
    expect(s.app.toasts).toMatchObject([
      { kind: "error", message: "c: Could not read the history", detail: "fatal: x" },
    ]);
  });
});

describe("settings that cannot be saved", () => {
  const problem = "/c/state.json could not be read (Permission denied (os error 13))";

  it("are reported once, however many changes follow", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(appState({ settingsProblem: problem }));
    await s.init();
    for (const path of ["/r/a", "/r/b", "/r/c"]) await s.openPath(path);
    for (let i = 0; i < 10; i++) s.cycleTab(1);
    await s.setTheme("dark");
    expect(s.app.toasts).toHaveLength(1);
    expect(s.app.toasts[0]).toMatchObject({ kind: "error", persistent: true, detail: problem });
    expect(s.app.toasts[0].actions?.map((a) => a.label)).toEqual(["Reset settings"]);
  });

  it("Reset settings moves the file aside through the core, then the notice goes", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(appState({ settingsProblem: problem }));
    vi.mocked(ipc.api.resetSettings).mockResolvedValue(undefined);
    await s.init();
    await s.app.toasts[0].actions![0].run();
    expect(ipc.api.resetSettings).toHaveBeenCalledTimes(1);
    expect(s.app.toasts.map((t) => t.message)).toEqual(["Settings reset: saving again."]);
  });

  it("a reset that fails keeps the notice and says why", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue(appState({ settingsProblem: problem }));
    vi.mocked(ipc.api.resetSettings).mockRejectedValue(coreError("io", "Could not move /c/state.json aside"));
    await s.init();
    await s.app.toasts[0].actions![0].run();
    expect(s.app.toasts).toHaveLength(2);
    expect(s.app.toasts[0].persistent).toBe(true);
    expect(s.app.toasts[1]).toMatchObject({ kind: "error", message: "Could not reset settings: Could not move /c/state.json aside" });
  });
});

describe("recents", () => {
  it("removing a recent takes the list the core returns", async () => {
    const rest = [{ path: "/r/b", name: "b", lastOpenedMs: 2 }];
    vi.mocked(ipc.api.removeRecent).mockResolvedValue(rest);
    await s.removeRecent("/r/a");
    expect(ipc.api.removeRecent).toHaveBeenCalledWith("/r/a");
    expect(s.app.recents).toEqual(rest);
  });
});

describe("theme", () => {
  it("setTheme updates the setting and persists it", async () => {
    await s.setTheme("dark");
    expect(s.app.settings.theme).toBe("dark");
    expect(ipc.api.updateSettings).toHaveBeenCalledWith({ theme: "dark" });
  });

  it("keeps the last theme chosen when an earlier choice is answered after it", async () => {
    const answers: (() => void)[] = [];
    vi.mocked(ipc.api.updateSettings).mockImplementation(
      (patch) =>
        new Promise((resolve) => answers.push(() => resolve({ theme: "system", fetchIntervalSecs: 300, editorCommand: null, ...patch }))),
    );
    const light = s.setTheme("light");
    const dark = s.setTheme("dark");
    answers[1]();
    await dark;
    answers[0]();
    await light;
    expect(s.app.settings.theme).toBe("dark");
  });

  it("cycleTheme goes system, light, dark and back", async () => {
    const seen: string[] = [];
    for (let i = 0; i < 3; i++) {
      await s.cycleTheme();
      seen.push(s.app.settings.theme);
    }
    expect(seen).toEqual(["light", "dark", "system"]);
  });
});

describe("toasts", () => {
  it("are added with unique ids and dismissed by id", () => {
    s.toast("info", "one");
    s.toast("error", "two", "why");
    const [a, b] = s.app.toasts;
    expect(a.id).not.toBe(b.id);
    expect(b).toMatchObject({ kind: "error", message: "two", detail: "why" });
    s.dismissToast(a.id);
    expect(s.app.toasts.map((t) => t.message)).toEqual(["two"]);
  });
});
