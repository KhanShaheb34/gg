import { beforeEach, describe, expect, it, vi } from "vitest";

const handlers = new Map<string, (p: unknown) => void>();
vi.mock("../ipc", () => ({
  onEvent: (name: string, handler: (p: unknown) => void) => {
    handlers.set(name, handler);
    return () => handlers.delete(name);
  },
}));

describe("the busy indicator's progress line", () => {
  beforeEach(() => {
    vi.resetModules();
    handlers.clear();
  });

  it("keeps git's latest line of the running operation, made one tidy visible line", async () => {
    const { watchOpProgress, progressLine } = await import("./opProgress.svelte");
    watchOpProgress();
    const emit = (p: object) => handlers.get("op-progress")!(p);
    expect(progressLine(1, 7)).toBeNull();
    emit({ repoId: 1, opId: 7, line: "Counting objects:  50% (5/10)" });
    emit({ repoId: 1, opId: 7, line: "  Writing objects:  45% (9/20),\t1.2 MiB | 3 MiB/s  " });
    emit({ repoId: 2, opId: 8, line: "elsewhere" });
    expect(progressLine(1, 7)).toBe("Writing objects: 45% (9/20), 1.2 MiB | 3 MiB/s");
    // Another operation's line is not this one's.
    expect(progressLine(1, 6)).toBeNull();
    expect(progressLine(2, 8)).toBe("elsewhere");
    // pr_create's own steps come the same way.
    emit({ repoId: 1, opId: 7, line: "Creating the pull request…" });
    expect(progressLine(1, 7)).toBe("Creating the pull request…");
    // Characters that would hide or rearrange text are shown.
    emit({ repoId: 1, opId: 7, line: "remote: a‮b" });
    expect(progressLine(1, 7)).toBe("remote: a‹U+202E›b");
  });

  it("is cleared when the operation ends", async () => {
    const { watchOpProgress, progressLine, clearProgress } = await import("./opProgress.svelte");
    watchOpProgress();
    handlers.get("op-progress")!({ repoId: 1, opId: 7, line: "Receiving objects: 10%" });
    clearProgress(1);
    expect(progressLine(1, 7)).toBeNull();
  });

  it("shows only once the operation has run for a second", async () => {
    const { PROGRESS_AFTER_MS, progressDue } = await import("./opProgress.svelte");
    expect(PROGRESS_AFTER_MS).toBe(1000);
    expect(progressDue(10_000, 10_999)).toBe(false);
    expect(progressDue(10_000, 11_000)).toBe(true);
  });
});
