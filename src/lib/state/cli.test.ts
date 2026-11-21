import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CliState } from "../types";

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn(() => () => {}),
    api: {
      getAppState: vi.fn(),
      installCli: vi.fn(),
      uninstallCli: vi.fn(),
      turnOffShellGg: vi.fn(),
      removeShellLine: vi.fn(),
    },
  };
});

type Ipc = typeof import("../ipc");

const before: CliState = {
  installed: null,
  link: null,
  shadowedBy: "an alias for git log",
  candidates: ["/h/.local/bin", "/h/bin"],
  shadowConflict: "alias",
  turnOff: { conflict: "alias", file: "/h/.zshrc" },
  shellLine: null,
};
const after: CliState = { ...before, installed: "/h/bin/gg" };
/** No line taken out of a shell file. */
const noLine = { lineRemovedFrom: [], lineProblem: null };

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
    vi.mocked(ipc.api.turnOffShellGg).mockReset();
    vi.mocked(ipc.api.removeShellLine).mockReset();
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
    vi.mocked(ipc.api.installCli).mockResolvedValueOnce({ ...after, shellLineAdded: null });
    expect(await cli.installCli("/h/bin")).toEqual({ state: after, shellLineAdded: null });
    // The box left as it is: no line asked for.
    expect(vi.mocked(ipc.api.installCli).mock.calls).toEqual([["/h/bin", false]]);
    expect(app.cli).toEqual(after);
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({ ...before, removed: ["/h/bin/gg"], ...noLine });
    expect(await cli.uninstallCli()).toEqual({ removed: ["/h/bin/gg"], line: noLine });
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
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({ ...before, removed: ["/h/bin/gg", "/h/.local/bin/gg"], ...noLine });
    await cli.removeCli();
    expect(app.cli).toEqual(before);
    expect(app.toasts.map((t) => [t.kind, t.message])).toEqual([
      ["info", "Removed the gg command: /h/bin/gg, /h/.local/bin/gg."],
    ]);
    // Removed meanwhile by hand: nothing went, and only that is said.
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({ ...before, removed: [], ...noLine });
    await cli.removeCli();
    expect(app.toasts.at(-1)?.message).toBe("Nothing to remove: no gg command written by gg was found on your PATH.");
    app.cli = after;
    const refusal = new ipc.CoreError({ code: "io", message: "Could not remove /h/bin/gg", detail: "denied" });
    vi.mocked(ipc.api.uninstallCli).mockRejectedValueOnce(refusal);
    await cli.removeCli();
    expect(app.cli).toEqual(after);
    expect(app.toasts.at(-1)).toMatchObject({ kind: "error", message: "Could not remove the gg command: Could not remove /h/bin/gg" });
  });

  it("installs with the line when the box is ticked, and keeps the state without the outcome", async () => {
    const turnedOff: CliState = { ...after, shadowedBy: null, shadowConflict: null, turnOff: null, shellLine: "/h/.zshrc" };
    const shellLineAdded = { file: "/h/.zshrc", conflict: "alias" as const, added: true, problem: null, checked: true };
    vi.mocked(ipc.api.installCli).mockResolvedValueOnce({ ...turnedOff, shellLineAdded });
    expect(await cli.installCli("/h/bin", true)).toEqual({ state: turnedOff, shellLineAdded });
    expect(vi.mocked(ipc.api.installCli).mock.calls).toEqual([["/h/bin", true]]);
    expect(app.cli).toEqual(turnedOff);
  });

  it("says the line went with the command, or why it could not", async () => {
    app.cli = { ...after, shellLine: "/h/.zshrc" };
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({
      ...before,
      removed: ["/h/bin/gg"],
      lineRemovedFrom: ["/h/.zshrc"],
      lineProblem: null,
    });
    await cli.removeCli();
    expect(app.cli).toEqual(before);
    expect(app.toasts.map((t) => [t.kind, t.message, t.note])).toEqual([
      ["info", "Removed the gg command: /h/bin/gg.", "Took gg’s line out of /h/.zshrc; new terminal windows no longer run it."],
    ]);
    const problem = "Could not take gg's line out of /h/.zshrc: Permission denied (os error 13).";
    vi.mocked(ipc.api.uninstallCli).mockResolvedValueOnce({ ...before, removed: ["/h/bin/gg"], lineRemovedFrom: [], lineProblem: problem });
    await cli.removeCli();
    expect(app.toasts.at(-1)).toMatchObject({ kind: "error", message: "Removed the gg command: /h/bin/gg.", note: problem });
  });

  it("turns the shell's gg off from Settings and says whether the shell still has it", async () => {
    app.cli = before;
    const shellLineAdded = { file: "/h/.zshrc", conflict: "alias" as const, added: true, problem: null, checked: true };
    const turnedOff: CliState = { ...before, shadowedBy: null, shadowConflict: null, turnOff: null, shellLine: "/h/.zshrc" };
    vi.mocked(ipc.api.turnOffShellGg).mockResolvedValueOnce({ ...turnedOff, shellLineAdded });
    await cli.turnOffShellGg();
    expect(app.cli).toEqual(turnedOff);
    expect(app.toasts.at(-1)).toMatchObject({
      kind: "info",
      message: "Turned off your shell’s gg alias: added one line to the end of /h/.zshrc. It takes effect in new terminal windows.",
      persistent: false,
    });
    // Something after the line makes it again: said, and it stays until dismissed.
    vi.mocked(ipc.api.turnOffShellGg).mockResolvedValueOnce({ ...turnedOff, shadowedBy: "an alias for git log", shadowConflict: "alias", shellLineAdded });
    await cli.turnOffShellGg();
    expect(app.toasts.at(-1)).toMatchObject({
      kind: "info",
      message:
        "Added one line to the end of /h/.zshrc, but your shell still defines gg as an alias for git log: something that runs after that line defines it again.",
      persistent: true,
    });
    // Refused (nothing to turn off): the state as it was, and why.
    const state = app.cli;
    const refusal = new ipc.CoreError({ code: "bad_request", message: "Your shell has no gg alias or function that gg can turn off.", detail: null });
    vi.mocked(ipc.api.turnOffShellGg).mockRejectedValueOnce(refusal);
    await cli.turnOffShellGg();
    expect(app.cli).toBe(state);
    expect(app.toasts.at(-1)).toMatchObject({ kind: "error" });
  });

  it("takes the line out from Settings", async () => {
    app.cli = { ...before, shadowedBy: null, shadowConflict: null, turnOff: null, shellLine: "/h/.zshrc" };
    vi.mocked(ipc.api.removeShellLine).mockResolvedValueOnce({ ...before, lineRemovedFrom: ["/h/.zshrc"], lineProblem: null });
    await cli.removeShellLine();
    expect(app.cli).toEqual(before);
    expect(app.toasts.at(-1)).toMatchObject({ kind: "info", message: "Took gg’s line out of /h/.zshrc; new terminal windows no longer run it." });
    vi.mocked(ipc.api.removeShellLine).mockResolvedValueOnce({ ...before, ...noLine });
    await cli.removeShellLine();
    expect(app.toasts.at(-1)?.message).toBe("No line of gg’s was found in your shell’s start-up files.");
    const changed = "A line in ~/.zshrc is marked as added by gg but has been changed since; gg left it alone.";
    vi.mocked(ipc.api.removeShellLine).mockResolvedValueOnce({ ...before, lineRemovedFrom: [], lineProblem: changed });
    await cli.removeShellLine();
    expect(app.toasts.at(-1)).toMatchObject({ kind: "error", message: changed, persistent: true });
  });

  it("opens one dialog, with the state read when it opens", async () => {
    vi.mocked(ipc.api.getAppState).mockResolvedValue({ cli: after } as never);
    await Promise.all([cli.openCliDialog(), cli.openCliDialog()]);
    expect(overlays.overlays.dialogs).toHaveLength(1);
    expect(app.cli).toEqual(after);
    await cli.openCliDialog();
    expect(overlays.overlays.dialogs).toHaveLength(1);
  });

  it("opens no dialog for a command Homebrew linked, and says where it is and who looks after it", async () => {
    const homebrew: CliState = { ...before, installed: "/opt/homebrew/bin/gg", link: "homebrew", shadowedBy: null };
    vi.mocked(ipc.api.getAppState).mockResolvedValue({ cli: homebrew } as never);
    await cli.openCliDialog();
    expect(overlays.overlays.dialogs).toHaveLength(0);
    expect(app.toasts.map((t) => [t.kind, t.message, t.detail])).toEqual([
      [
        "info",
        "The gg command is already installed by Homebrew at /opt/homebrew/bin/gg.",
        "Homebrew made it, a link to the launcher inside gg.app, and removes it when you uninstall gg with Homebrew; gg leaves it alone.",
      ],
    ]);
    // Anyone else's link: the same, without naming who.
    vi.mocked(ipc.api.getAppState).mockResolvedValue({ cli: { ...homebrew, installed: "/usr/local/bin/gg", link: "other" } } as never);
    await cli.openCliDialog();
    expect(overlays.overlays.dialogs).toHaveLength(0);
    expect(app.toasts.at(-1)).toMatchObject({
      message: "The gg command is already installed at /usr/local/bin/gg, a link to gg’s launcher.",
      detail: "gg did not make this link and leaves it alone.",
    });
  });
});
