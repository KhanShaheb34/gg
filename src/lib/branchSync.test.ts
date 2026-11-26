import { describe, expect, it } from "vitest";
import { branchSync } from "./branchSync";
import { DETACHED, HEAD_HASH, inProgress, RUNNING, snap, UNBORN, worktree } from "./menus/menuTestData";
import type { RepoSnapshot } from "./types";

/** A snapshot whose active worktree (/r) has `main` checked out with `upstream`. */
function on(upstream: { name: string; ahead: number; behind: number } | null, overrides: Partial<RepoSnapshot> = {}): RepoSnapshot {
  return snap({
    worktrees: [
      worktree({ path: "/r", branch: "main", head: HEAD_HASH, isMain: true, active: true, upstream }),
      worktree({ upstream: { name: "origin/feature/x", ahead: 3, behind: 4 } }),
    ],
    ...overrides,
  });
}

describe("branchSync", () => {
  it("behind: a pull of the current branch, as its menu's Pull sends it", () => {
    const s = branchSync(on({ name: "origin/main", ahead: 0, behind: 9 }))!;
    expect(s.branch).toBe("main");
    expect(s.upstream).toBe("origin/main");
    expect(s.push).toBeNull();
    expect(s.pull).toEqual({
      count: 9,
      title: "Pull 9 commits from origin/main",
      request: { op: "pull", branch: "main", mode: "default", expectHead: { branch: "main", hash: HEAD_HASH }, expectWorktree: "/r" },
      disabledReason: undefined,
    });
    expect(branchSync(on({ name: "origin/main", ahead: 0, behind: 1 }))!.pull!.title).toBe("Pull 1 commit from origin/main");
  });

  it("ahead: a push to its upstream of the same name", () => {
    const s = branchSync(on({ name: "origin/main", ahead: 2, behind: 0 }))!;
    expect(s.pull).toBeNull();
    expect(s.push).toEqual({
      count: 2,
      title: "Push 2 commits to origin/main",
      note: null,
      request: { op: "push", branch: "main", remote: "origin", to: null, force: false },
      disabledReason: undefined,
    });
  });

  it("diverged: both, the push saying git will refuse it until the pull", () => {
    const s = branchSync(on({ name: "origin/main", ahead: 2, behind: 3 }))!;
    expect(s.pull!.count).toBe(3);
    expect(s.push!.count).toBe(2);
    expect(s.push!.request).not.toBeNull();
    expect(s.push!.note).toBe("origin/main has 3 commits that main does not: git refuses the push until you pull them, or force push.");
  });

  it("an upstream of another name: the Push dialog, which names where the push goes", () => {
    const s = branchSync(on({ name: "origin/trunk", ahead: 1, behind: 0 }))!;
    expect(s.push!.request).toBeNull();
    expect(s.push!.title).toBe("Push 1 commit…");
    expect(s.push!.note).toBe("Its upstream, origin/trunk, has another name: choose where the push goes.");
  });

  it("the remote is the longest remote name the upstream starts with", () => {
    const s = branchSync(on({ name: "up/stream/main", ahead: 1, behind: 0 }, { remotes: ["up", "up/stream"] }))!;
    expect(s.push!.request).toEqual({ op: "push", branch: "main", remote: "up/stream", to: null, force: false });
  });

  it("an upstream that is a local branch: a pull, but nothing to push to", () => {
    expect(branchSync(on({ name: "develop", ahead: 2, behind: 0 }))).toBeNull();
    const s = branchSync(on({ name: "develop", ahead: 2, behind: 1 }))!;
    expect(s.push).toBeNull();
    expect(s.pull!.title).toBe("Pull 1 commit from develop");
  });

  it("nothing: level, no upstream, detached, unborn, or the worktrees not read yet", () => {
    expect(branchSync(on({ name: "origin/main", ahead: 0, behind: 0 }))).toBeNull();
    expect(branchSync(on(null))).toBeNull();
    expect(branchSync(on({ name: "origin/main", ahead: 1, behind: 1 }, DETACHED))).toBeNull();
    expect(branchSync(on({ name: "origin/main", ahead: 1, behind: 1 }, UNBORN))).toBeNull();
    expect(branchSync(snap())).toBeNull();
  });

  it("the active worktree's branch, not another worktree's", () => {
    const s = snap({
      activeWorktree: "/r.worktrees/feature-x",
      head: { branch: "feature/x", hash: HEAD_HASH },
      worktrees: [
        worktree({ path: "/r", branch: "main", upstream: { name: "origin/main", ahead: 0, behind: 5 } }),
        worktree({ active: true, upstream: { name: "origin/feature/x", ahead: 3, behind: 4 } }),
      ],
    });
    const sync = branchSync(s)!;
    expect(sync.branch).toBe("feature/x");
    expect(sync.pull!.count).toBe(4);
    expect(sync.pull!.request).toMatchObject({ branch: "feature/x", expectWorktree: "/r.worktrees/feature-x" });
    // HEAD read before the worktree list caught up with a switch: nothing, until they agree.
    expect(branchSync({ ...s, head: { branch: "other", hash: HEAD_HASH } })).toBeNull();
  });

  it("waits while an action runs; a pull also while git is stopped in a merge", () => {
    const running = branchSync(on({ name: "origin/main", ahead: 1, behind: 1 }, RUNNING))!;
    expect(running.pull!.disabledReason).toBe("Wait for Push main to origin to end");
    expect(running.push!.disabledReason).toBe("Wait for Push main to origin to end");
    const merging = branchSync(on({ name: "origin/main", ahead: 1, behind: 1 }, inProgress("merge")))!;
    expect(merging.pull!.disabledReason).toBe("A merge is in progress");
    expect(merging.push!.disabledReason).toBeUndefined();
  });
});
