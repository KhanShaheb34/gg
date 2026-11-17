import { describe, expect, it } from "vitest";
import { confirmationIsRed, isRed } from "./dangers";
import type { DangerKind } from "./types";

describe("danger severity", () => {
  it("ignored files and pruning stale entries are ordinary; everything else is red", () => {
    expect(isRed("remove_worktree_ignored_files")).toBe(false);
    expect(isRed("prune_worktrees")).toBe(false);
    const red: DangerKind[] = [
      "force_push",
      "hard_reset",
      "delete_unmerged_branch",
      "delete_remote_branch",
      "drop_stash",
      "abort",
      "skip_discards",
      "remove_worktree_with_work",
      "delete_unmerged_branch_with_worktree",
      "prune_worktrees_loses_commits",
    ];
    for (const kind of red) expect(isRed(kind), kind).toBe(true);
  });

  it("a round is red when any of its dangers is", () => {
    expect(confirmationIsRed([{ kind: "remove_worktree_ignored_files" }])).toBe(false);
    expect(confirmationIsRed([{ kind: "prune_worktrees" }])).toBe(false);
    expect(confirmationIsRed([{ kind: "remove_worktree_ignored_files" }, { kind: "delete_unmerged_branch_with_worktree" }])).toBe(
      true,
    );
    expect(confirmationIsRed([{ kind: "remove_worktree_with_work" }])).toBe(true);
  });
});
