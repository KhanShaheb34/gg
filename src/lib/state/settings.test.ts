import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Settings } from "../types";

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn(() => () => {}),
    api: { updateSettings: vi.fn(), restoreDefaultSettings: vi.fn() },
  };
});

const DEFAULTS: Settings = {
  theme: "system",
  fetchIntervalSecs: 300,
  editorCommand: null,
  terminalCommand: null,
  worktreePattern: "../{repo}.worktrees/{branch}",
  dateFormat: "standard",
};

describe("saving a setting", () => {
  let ipc: typeof import("../ipc");
  let app: typeof import("./app.svelte").app;
  let s: typeof import("./settings.svelte");

  beforeEach(async () => {
    vi.resetModules();
    vi.useFakeTimers();
    ipc = await import("../ipc");
    app = (await import("./app.svelte")).app;
    s = await import("./settings.svelte");
    vi.mocked(ipc.api.updateSettings)
      .mockReset()
      .mockImplementation(async (patch) => ({ ...DEFAULTS, ...patch }));
  });
  afterEach(() => vi.useRealTimers());

  it("waits for a pause in typing, saves the last value once, says Saved, then nothing", async () => {
    const field = new s.SettingField("editorCommand", s.TEXT_DELAY_MS);
    field.set("c");
    field.set("co");
    field.set("code -w");
    expect(ipc.api.updateSettings).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(s.TEXT_DELAY_MS);
    expect(vi.mocked(ipc.api.updateSettings).mock.calls).toEqual([[{ editorCommand: "code -w" }]]);
    expect(field.status).toBe("saved");
    expect(app.settings.editorCommand).toBe("code -w");
    await vi.advanceTimersByTimeAsync(s.SAVED_FOR_MS);
    expect(field.status).toBe("idle");
  });

  it("shows the core's refusal and applies nothing; the next good value clears it", async () => {
    const { CoreError } = ipc;
    vi.mocked(ipc.api.updateSettings).mockRejectedValueOnce(
      new CoreError({ code: "bad_request", message: "Write {path} as a separate word.", detail: null }),
    );
    const field = new s.SettingField("terminalCommand", s.TEXT_DELAY_MS);
    field.set("wezterm --cwd={path}");
    expect(await field.flush()).toBe(false);
    expect(field.status).toBe("error");
    expect(field.error).toBe("Write {path} as a separate word.");
    expect(app.settings.terminalCommand ?? null).toBeNull();
    field.set("wezterm start --cwd {path}");
    expect(await field.flush()).toBe(true);
    expect(field.error).toBeNull();
    expect(app.settings.terminalCommand).toBe("wezterm start --cwd {path}");
  });

  it("a settings file that cannot be written is said once, for the whole sheet; a save that works clears it (Task 10 review minor 11)", async () => {
    const { CoreError } = ipc;
    const cannot = () =>
      new CoreError({ code: "io", message: "Could not save settings to /cfg", detail: "Permission denied (os error 13)" });
    vi.mocked(ipc.api.updateSettings).mockRejectedValueOnce(cannot()).mockRejectedValueOnce(cannot());
    const theme = new s.SettingField("theme", s.TEXT_DELAY_MS);
    const dates = new s.SettingField("dateFormat", s.TEXT_DELAY_MS);
    theme.set("dark");
    dates.set("iso");
    expect(await theme.flush()).toBe(false);
    expect(await dates.flush()).toBe(false);
    expect(s.saving.problem).toBe("Could not save settings to /cfg: Permission denied (os error 13)");
    // Said at the top of the sheet only: the fields carry no message of their own.
    expect([theme.error, dates.error]).toEqual([null, null]);
    expect([theme.status, dates.status]).toEqual(["error", "error"]);
    // A refusal of a value is the field's own, not the file's.
    vi.mocked(ipc.api.updateSettings).mockRejectedValueOnce(
      new CoreError({ code: "bad_request", message: "Write {path} as a separate word.", detail: null }),
    );
    const terminal = new s.SettingField("terminalCommand", s.TEXT_DELAY_MS);
    terminal.set("x --cwd={path}");
    await terminal.flush();
    expect(terminal.error).toBe("Write {path} as a separate word.");
    expect(s.saving.problem).toBe("Could not save settings to /cfg: Permission denied (os error 13)");
    dates.set("iso");
    expect(await dates.flush()).toBe(true);
    expect(s.saving.problem).toBeNull();
  });

  it("an answer to an earlier value never undoes a later one", async () => {
    const answers: (() => void)[] = [];
    vi.mocked(ipc.api.updateSettings).mockImplementation(
      (patch) => new Promise((resolve) => answers.push(() => resolve({ ...DEFAULTS, ...patch }))),
    );
    const field = new s.SettingField("dateFormat");
    field.set("iso");
    field.set("relative");
    answers[1]();
    await vi.advanceTimersByTimeAsync(0);
    answers[0]();
    await vi.advanceTimersByTimeAsync(0);
    expect(app.settings.dateFormat).toBe("relative");
  });

  it("Reset to defaults takes the core's settings and applies the theme", async () => {
    vi.mocked(ipc.api.restoreDefaultSettings).mockResolvedValueOnce(DEFAULTS);
    app.settings = { ...DEFAULTS, theme: "dark", editorCommand: "code" };
    await s.restoreDefaults();
    expect(app.settings).toEqual(DEFAULTS);
  });
});

describe("the command preview", () => {
  it("splits as the core does and puts the path where {path} is, else last", async () => {
    const { commandWords } = await import("./settings.svelte");
    expect(commandWords("code -w", "/r/a b")).toEqual(["code", "-w", "/r/a b"]);
    expect(commandWords("kitty --directory {path} --hold", "/r")).toEqual(["kitty", "--directory", "/r", "--hold"]);
    expect(commandWords("'/Applications/My Editor' -n", "/r")).toEqual(["/Applications/My Editor", "-n", "/r"]);
    expect(commandWords("code 'oops", "/r")).toBeNull();
    expect(commandWords("   ", "/r")).toEqual([]);
  });

  it("names fetch intervals plainly", async () => {
    const { intervalName, FETCH_CHOICES } = await import("./settings.svelte");
    expect(FETCH_CHOICES.map(intervalName)).toEqual(["Off", "1 min", "5 min", "15 min", "1 hour"]);
    expect(intervalName(45)).toBe("45 s");
    expect(intervalName(7200)).toBe("2 hours");
  });
});
