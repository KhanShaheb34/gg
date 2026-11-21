import { beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdateStatus } from "../types";

const listeners = new Map<string, (payload: unknown) => void>();

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn((name: string, handler: (payload: unknown) => void) => {
      listeners.set(name, handler);
      return () => listeners.delete(name);
    }),
    api: {
      updateStatus: vi.fn(),
      startUpdateChecks: vi.fn(),
      checkForUpdates: vi.fn(),
      restartToUpdate: vi.fn(),
      openAppLink: vi.fn(),
    },
  };
});

const platform = vi.hoisted(() => ({ mac: false }));
vi.mock("../platform", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../platform")>();
  return { ...actual, isMac: () => platform.mac };
});

type Ipc = typeof import("../ipc");

describe("what Settings says of an update", () => {
  let u: typeof import("./updates.svelte");

  beforeEach(async () => {
    vi.resetModules();
    u = await import("./updates.svelte");
  });

  const at = new Date(2026, 10, 21, 14, 5, 9).getTime();
  const line = (s: UpdateStatus, automatic = true, now = at + 60_000) => u.statusLine(s, automatic, "standard", now);

  it("says each state in plain words", () => {
    expect(line({ state: "unavailable" })).toEqual({ text: "This build of gg does not update itself.", tone: "off" });
    expect(line({ state: "idle" })).toEqual({ text: "Not checked yet.", tone: "off" });
    expect(line({ state: "idle" }, false)).toEqual({ text: "Automatic checks are off.", tone: "off" });
    expect(line({ state: "checking" })).toEqual({ text: "Checking for updates…", tone: "busy" });
    expect(line({ state: "upToDate", checkedAtMs: at })).toEqual({ text: "gg is up to date. Checked at 14:05:09.", tone: "ok" });
    expect(line({ state: "downloading", version: "0.4.1", received: 5, total: 20 }).text).toBe("Downloading gg 0.4.1… 25%");
    expect(line({ state: "downloading", version: "0.4.1", received: 5, total: null }).text).toBe("Downloading gg 0.4.1…");
    expect(line({ state: "ready", version: "0.4.1" })).toEqual({ text: "gg 0.4.1 is ready. Restart to update.", tone: "ok" });
    expect(line({ state: "installing", version: "0.4.1" }).text).toBe("Installing gg 0.4.1…");
    expect(line({ state: "available", version: "0.4.1", reason: "r" })).toEqual({ text: "gg 0.4.1 is available.", tone: "warn" });
    expect(
      line({
        state: "failed",
        step: "check",
        version: null,
        message: "Could not check for updates. GitHub could not be reached.",
        detail: null,
        atMs: at,
      }),
    ).toEqual({ text: "Could not check for updates. GitHub could not be reached.", tone: "warn" });
  });

  it("says when it checked in the user's date format", () => {
    const yesterday = at - 26 * 3600_000;
    expect(u.checkedWhen(at, "standard", at + 1000)).toBe("at 14:05:09");
    expect(u.checkedWhen(at, "relative", at + 120_000)).toBe("2 min ago");
    expect(u.checkedWhen(yesterday, "standard", at)).toMatch(/^20 Nov 2026 \d\d:\d\d$/);
    expect(u.checkedWhen(yesterday, "iso", at)).toMatch(/^2026-11-20 \d\d:\d\d$/);
  });
});

describe("the update notice", () => {
  let ipc: Ipc;
  let app: typeof import("./app.svelte").app;
  let u: typeof import("./updates.svelte");

  beforeEach(async () => {
    vi.resetModules();
    listeners.clear();
    ipc = await import("../ipc");
    app = (await import("./app.svelte")).app;
    u = await import("./updates.svelte");
    vi.mocked(ipc.api.updateStatus).mockReset().mockResolvedValue({ state: "idle" });
    vi.mocked(ipc.api.startUpdateChecks).mockReset().mockResolvedValue({ state: "idle" });
    vi.mocked(ipc.api.restartToUpdate).mockReset();
    vi.mocked(ipc.api.openAppLink).mockReset().mockResolvedValue({ note: null });
    app.toasts = [];
  });

  const send = (s: UpdateStatus) => listeners.get("update-changed")!(s);

  it("says once per run that a version is ready, and offers the restart", async () => {
    const stop = u.initUpdates();
    send({ state: "checking" });
    send({ state: "downloading", version: "0.4.1", received: 0, total: null });
    expect(app.toasts).toEqual([]);
    send({ state: "ready", version: "0.4.1" });
    expect(app.toasts).toHaveLength(1);
    const [notice] = app.toasts;
    expect(notice.message).toBe("gg 0.4.1 is ready.");
    expect(notice.persistent).toBe(true);
    expect(notice.actions?.map((a) => a.label)).toEqual(["Restart to update"]);
    // Dismissed: not again this run for that version, though a later check says it again.
    app.toasts = [];
    send({ state: "ready", version: "0.4.1" });
    expect(app.toasts).toEqual([]);
    // A newer one is said again.
    send({ state: "ready", version: "0.4.2" });
    expect(app.toasts.map((t) => t.message)).toEqual(["gg 0.4.2 is ready."]);
    stop();
  });

  it("a failed check or download says nothing in a notice", () => {
    const stop = u.initUpdates();
    send({ state: "failed", step: "check", version: null, message: "Could not check for updates. GitHub could not be reached.", detail: "x", atMs: 1 });
    send({ state: "failed", step: "download", version: "0.4.1", message: "Could not download gg 0.4.1. The download was not found.", detail: null, atMs: 1 });
    expect(app.toasts).toEqual([]);
    expect(u.updates.status.state).toBe("failed");
    stop();
  });

  it("where gg cannot update itself, says a version is available and why, with the download page", async () => {
    const stop = u.initUpdates();
    send({ state: "available", version: "0.5.0", reason: "gg is running from its disk image, so it cannot update itself." });
    const [notice] = app.toasts;
    expect(notice.message).toBe("gg 0.5.0 is available.");
    expect(notice.note).toBe("gg is running from its disk image, so it cannot update itself.");
    expect(notice.actions?.map((a) => a.label)).toEqual(["Download page"]);
    await notice.actions![0].run();
    expect(ipc.api.openAppLink).toHaveBeenCalledWith("home");
    stop();
  });

  it("names Homebrew's command only on a Mac where Homebrew linked gg and gg cannot update itself", async () => {
    const stop = u.initUpdates();
    platform.mac = true;
    app.cli = { installed: "/opt/homebrew/bin/gg", link: "homebrew", shadowedBy: null, candidates: [], shadowConflict: null, turnOff: null, shellLine: null };
    send({ state: "available", version: "0.5.0", reason: "gg cannot update itself: it may not change /Applications." });
    expect(app.toasts[0].note).toBe(
      "gg cannot update itself: it may not change /Applications. Or update it with Homebrew: brew upgrade --cask gg",
    );
    expect(u.brewHint()).toBe("brew upgrade --cask gg");
    app.cli = { installed: null, link: null, shadowedBy: null, candidates: [], shadowConflict: null, turnOff: null, shellLine: null };
    expect(u.brewHint()).toBeNull();
    platform.mac = false;
    stop();
  });

  it("a restart refused while git works says why and keeps the notice", async () => {
    const stop = u.initUpdates();
    send({ state: "ready", version: "0.4.1" });
    vi.mocked(ipc.api.restartToUpdate).mockRejectedValue(
      new ipc.CoreError({ code: "busy", message: "gg cannot restart while git is working in app. Restart once it is done.", detail: null }),
    );
    await app.toasts[0].actions![0].run();
    expect(app.toasts.map((t) => [t.kind, t.message])).toEqual([
      ["info", "gg 0.4.1 is ready."],
      ["info", "gg cannot restart while git is working in app. Restart once it is done."],
    ]);
    stop();
  });

  it("a status read that answers after an event is older, and ignored", async () => {
    let answer!: (s: UpdateStatus) => void;
    vi.mocked(ipc.api.updateStatus).mockReturnValue(new Promise((r) => (answer = r)));
    const stop = u.initUpdates();
    send({ state: "ready", version: "0.4.1" });
    answer({ state: "idle" });
    await Promise.resolve();
    await Promise.resolve();
    expect(u.updates.status).toEqual({ state: "ready", version: "0.4.1" });
    expect(app.toasts).toHaveLength(1);
    stop();
  });

  it("starts the checks once, when the first rows are on screen", async () => {
    const stop = u.initUpdates();
    expect(ipc.api.startUpdateChecks).not.toHaveBeenCalled();
    const rows = await import("./firstRows");
    rows.firstRowsShown();
    rows.firstRowsShown();
    u.startUpdates();
    expect(ipc.api.startUpdateChecks).toHaveBeenCalledTimes(1);
    stop();
  });
});
