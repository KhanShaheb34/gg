import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CliState } from "../types";

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn(() => () => {}),
    api: { getAppState: vi.fn(), installCli: vi.fn(), uninstallCli: vi.fn() },
  };
});

type Ipc = typeof import("../ipc");

const before: CliState = { installed: null, shadowedBy: "an alias for git log", candidates: ["/h/.local/bin", "/h/bin"] };
const after: CliState = { ...before, installed: "/h/bin/gg" };

describe("the gg command's state", () => {
  let ipc: Ipc;
  let app: typeof import("./app.svelte").app;
  let cli: typeof import("./cli.svelte");
  let overlays: typeof import("./overlays.svelte");

  beforeEach(async () => {
    vi.resetModules();
    ipc = await import("../ipc");
    app = (await import("./app.svelte")).app;
    cli = await import("./cli.svelte");
    overlays = await import("./overlays.svelte");
    vi.mocked(ipc.api.getAppState).mockReset();
    vi.mocked(ipc.api.installCli).mockReset();
    vi.mocked(ipc.api.uninstallCli).mockReset();
  });

  it("is read afresh, and kept as it was when that fails", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValueOnce({ cli: before } as never);
    await cli.refreshCli();
    expect(app.cli).toEqual(before);
    vi.mocked(ipc.api.getAppState).mockRejectedValueOnce(new Error("gone"));
    await cli.refreshCli();
    expect(app.cli).toEqual(before);
  });

  it("follows an install and a removal", async () => {
    vi.mocked(ipc.api.installCli).mockResolvedValueOnce(after);
    expect(await cli.installCli("/h/bin")).toEqual(after);
    expect(vi.mocked(ipc.api.installCli).mock.calls).toEqual([["/h/bin"]]);
    expect(app.cli).toEqual(after);
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({ ...before, removed: ["/h/bin/gg"] });
    expect(await cli.uninstallCli()).toEqual(["/h/bin/gg"]);
    expect(app.cli).toEqual(before);
  });

  it("leaves the state alone when the core refuses", async () => {
    app.cli = before;
    const refusal = new ipc.CoreError({ code: "bad_request", message: "There is already a gg", detail: null });
    vi.mocked(ipc.api.installCli).mockRejectedValueOnce(refusal);
    await expect(cli.installCli("/h/bin")).rejects.toBe(refusal);
    expect(app.cli).toEqual(before);
  });

  it("removes the command from the welcome screen and says so, or why not", async () => {
    app.cli = after;
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({ ...before, removed: ["/h/bin/gg", "/h/.local/bin/gg"] });
    await cli.removeCli();
    expect(app.cli).toEqual(before);
    expect(app.toasts.map((t) => [t.kind, t.message])).toEqual([
      ["info", "Removed the gg command: /h/bin/gg, /h/.local/bin/gg."],
    ]);
    // Removed meanwhile by hand: nothing went, and only that is said.
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({ ...before, removed: [] });
    await cli.removeCli();
    expect(app.toasts.at(-1)?.message).toBe("Nothing to remove: no gg command written by gg was found on your PATH.");
    app.cli = after;
    const refusal = new ipc.CoreError({ code: "io", message: "Could not remove /h/bin/gg", detail: "denied" });
    vi.mocked(ipc.api.uninstallCli).mockRejectedValueOnce(refusal);
    await cli.removeCli();
    expect(app.cli).toEqual(after);
    expect(app.toasts.at(-1)).toMatchObject({ kind: "error", message: "Could not remove the gg command: Could not remove /h/bin/gg" });
  });

  it("opens one dialog, with the state read when it opens", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue({ cli: after } as never);
    await Promise.all([cli.openCliDialog(), cli.openCliDialog()]);
    expect(overlays.overlays.dialogs).toHaveLength(1);
    expect(app.cli).toEqual(after);
    await cli.openCliDialog();
    expect(overlays.overlays.dialogs).toHaveLength(1);
  });
});
