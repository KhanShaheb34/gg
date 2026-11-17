import { beforeEach, describe, expect, it, vi } from "vitest";
import { snap } from "./menuTestData";
import type { CommitRow, RefLabel } from "../types";

vi.mock("../state/ops.svelte", () => ({ runOp: vi.fn(async () => ({})) }));

describe("pick", () => {
  beforeEach(() => vi.resetModules());

  it("says in a full sentence, the name shown, when no remote has a remote branch", async () => {
    const { pick } = await import("./dispatch");
    const { app } = await import("../state/app.svelte");
    const label = { kind: "remote", name: "gone/x‮" } as RefLabel;
    pick("delete", { row: { kind: "commit" } as CommitRow, label }, snap({ remotes: ["origin"] }));
    expect(app.toasts.map((t) => t.message)).toEqual(["No remote of this repository has gone/x‹U+202E›."]);
  });

  it("drops, pops and applies a stash by its commit, in the worktree and on the branch the menu was built from", async () => {
    const { pick } = await import("./dispatch");
    const { runOp } = await import("../state/ops.svelte");
    const row = { kind: "stash", hash: "a".repeat(40), subject: "On main: wip" } as CommitRow;
    const shown = snap();
    pick("stash.drop", { row, label: null }, shown);
    pick("stash.pop", { row, label: null }, shown);
    pick("stash.apply", { row, label: null }, shown);
    expect(vi.mocked(runOp).mock.calls.map((c) => c[1])).toEqual([
      { op: "stash_drop", stash: "a".repeat(40), expectWorktree: "/r" },
      { op: "stash_pop", stash: "a".repeat(40), expectHead: shown.head, expectWorktree: "/r" },
      { op: "stash_apply", stash: "a".repeat(40), expectHead: shown.head, expectWorktree: "/r" },
    ]);
    expect(shown.head.branch).not.toBeNull();
  });
});
