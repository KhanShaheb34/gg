import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return { CoreError: actual.CoreError, onEvent: vi.fn(() => () => {}), api: { openInEditor: vi.fn() } };
});

type Ipc = typeof import("../ipc");

describe("openInEditor", () => {
  let api: Ipc["api"];
  let app: typeof import("./app.svelte").app;
  let openInEditor: typeof import("./editor").openInEditor;

  beforeEach(async () => {
    vi.resetModules();
    api = (await import("../ipc")).api;
    app = (await import("./app.svelte")).app;
    openInEditor = (await import("./editor")).openInEditor;
    vi.mocked(api.openInEditor).mockReset();
  });

  it("asks the core for the file, or the folder, and shows nothing when it opened that", async () => {
    vi.mocked(api.openInEditor).mockResolvedValue(null);
    await openInEditor(3, "src/a b.ts");
    await openInEditor(3, null);
    expect(vi.mocked(api.openInEditor).mock.calls).toEqual([
      [3, "src/a b.ts"],
      [3, null],
    ]);
    expect(app.toasts).toEqual([]);
  });

  it("shows the core's note (the folder opened instead) once", async () => {
    const note = "No editor command is set, so the worktree folder was opened instead of the file.";
    vi.mocked(api.openInEditor).mockResolvedValue(note);
    await openInEditor(3, "a.txt");
    await openInEditor(3, "b.txt");
    expect(app.toasts.map((t) => [t.kind, t.message])).toEqual([["info", note]]);
  });

  it("reports an error", async () => {
    const { CoreError } = await import("../ipc");
    vi.mocked(api.openInEditor).mockRejectedValue(
      new CoreError({ code: "io", message: "The program “x” was not found.", detail: null }),
    );
    await openInEditor(3, "a.txt");
    expect(app.toasts.map((t) => [t.kind, t.message])).toEqual([["error", "The program “x” was not found."]]);
  });
});
