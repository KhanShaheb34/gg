import { beforeEach, describe, expect, it, vi } from "vitest";
import { pullRequest, snap, worktree } from "../menus/menuTestData";
import type { BranchFiles, RepoSnapshot } from "../types";

// Each branch_files request, answered when the test says.
const asked = vi.hoisted(() => [] as { path: string; resolve: (d: unknown) => void; reject: (e: unknown) => void }[]);
vi.mock("../ipc", () => ({
  api: {
    branchFiles: vi.fn((_repoId: number, path: string) => new Promise((resolve, reject) => asked.push({ path, resolve, reject }))),
  },
}));

import { BranchFold, branchFilesKey, branchFilesOf, forgetBranchFiles, loadBranchFiles, remoteTipsSignature } from "./branchFiles.svelte";

const WT = "/r.worktrees/feature-x";
const tip = (hash: string) => ({ hash, time: 0, author: "Ada", subject: "s" });

/** A snapshot with the feature/x worktree at `head`, tracking origin/feature/x, origin's HEAD main. */
function shot(o: Partial<RepoSnapshot> = {}, head = "a".repeat(40)): RepoSnapshot {
  return snap({
    worktrees: [worktree({ path: WT, head, upstream: { name: "origin/feature/x", remote: "origin", branch: "feature/x", ahead: 1, behind: 0 } })],
    remoteBranches: ["origin/main", "origin/develop", "origin/feature/x"],
    remoteBranchTips: [tip("1".repeat(40)), tip("2".repeat(40)), tip(head)],
    remoteHeads: { origin: "main" },
    ...o,
  });
}

const answer = (base: string, files = 1): BranchFiles => ({
  branch: "feature/x",
  base,
  baseCommit: "1".repeat(40),
  mergeBase: "1".repeat(40),
  head: "a".repeat(40),
  files: Array.from({ length: files }, (_, i) => ({ path: `f${i}.txt`, oldPath: null, status: "A" as const, additions: 1, deletions: 0 })),
  total: files,
});

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("what the list depends on (the key it is asked for again by)", () => {
  it("moves with HEAD, the branch, its upstream, its pull request's base, the remotes' defaults and the base's commit", () => {
    const k = branchFilesKey(shot(), WT);
    expect(branchFilesKey(shot({}, "b".repeat(40)), WT)).not.toBe(k);
    expect(branchFilesKey(shot({ worktrees: [worktree({ path: WT, head: "a".repeat(40), branch: "other" })] }), WT)).not.toBe(k);
    expect(branchFilesKey(shot({ worktrees: [worktree({ path: WT, head: "a".repeat(40), upstream: null })] }), WT)).not.toBe(k);
    expect(branchFilesKey(shot({ branchPrs: { "feature/x": pullRequest({ base: "develop" }) } }), WT)).not.toBe(k);
    expect(branchFilesKey(shot({ remoteHeads: { origin: "develop" } }), WT)).not.toBe(k);
    expect(branchFilesKey(shot({ remotes: ["origin", "upstream"] }), WT)).not.toBe(k);
    // The default branch's commit moved (a fetch); any other remote branch too (the base may be
    // the nearest by history), one coming or going too.
    const moved = shot();
    moved.remoteBranchTips = [tip("9".repeat(40)), tip("2".repeat(40)), tip("a".repeat(40))];
    expect(branchFilesKey(moved, WT)).not.toBe(k);
    const other = shot();
    other.remoteBranchTips = [tip("1".repeat(40)), tip("8".repeat(40)), tip("a".repeat(40))];
    expect(branchFilesKey(other, WT)).not.toBe(k);
    const added = shot({
      remoteBranches: ["origin/main", "origin/develop", "origin/feature/x", "origin/staging"],
      remoteBranchTips: [tip("1".repeat(40)), tip("2".repeat(40)), tip("a".repeat(40)), tip("3".repeat(40))],
    });
    expect(branchFilesKey(added, WT)).not.toBe(k);
  });

  it("moves when a better base comes into reach: the pull request's base fetched where only the local branch was (review)", () => {
    const pr = { branchPrs: { "feature/x": pullRequest({ base: "release" }) } };
    const local = shot({ ...pr, branches: ["main", "feature/x", "release"], branchTips: [tip("7".repeat(40)), tip("a".repeat(40)), tip("5".repeat(40))] });
    const fetched = shot({
      ...pr,
      branches: ["main", "feature/x", "release"],
      branchTips: [tip("7".repeat(40)), tip("a".repeat(40)), tip("5".repeat(40))],
      remoteBranches: ["origin/main", "origin/develop", "origin/feature/x", "origin/release"],
      remoteBranchTips: [tip("1".repeat(40)), tip("2".repeat(40)), tip("a".repeat(40)), tip("5".repeat(40))],
    });
    expect(branchFilesKey(fetched, WT)).not.toBe(branchFilesKey(local, WT));
    // The local one moving counts too, while it is a candidate.
    const movedLocal = shot({ ...pr, branches: ["main", "feature/x", "release"], branchTips: [tip("7".repeat(40)), tip("a".repeat(40)), tip("6".repeat(40))] });
    expect(branchFilesKey(movedLocal, WT)).not.toBe(branchFilesKey(local, WT));
  });

  it("moves when the base moves after an answer of none (review): the branch at the base's commit, then the base going back", () => {
    // feature/x and origin/main at one commit: the core says none.
    const same = shot({ remoteBranchTips: [tip("a".repeat(40)), tip("2".repeat(40)), tip("a".repeat(40))] });
    const back = shot({ remoteBranchTips: [tip("0".repeat(40)), tip("2".repeat(40)), tip("a".repeat(40))] });
    expect(branchFilesKey(back, WT)).not.toBe(branchFilesKey(same, WT));
    // The base's ref going away (origin's HEAD forgotten, then set again) too.
    expect(branchFilesKey(shot({ remoteHeads: {} }), WT)).not.toBe(branchFilesKey(same, WT));
  });

  it("does not move with what the list does not depend on: a new snapshot, files written, an operation", () => {
    const k = branchFilesKey(shot(), WT);
    expect(branchFilesKey(shot({ seq: 99, generation: 7, status: { changed: 3, untracked: 1, conflicted: 0, conflictedPaths: [] } }), WT)).toBe(k);
    expect(branchFilesKey(shot({ op: { id: 1, title: "Fetch", startedAtMs: 1, queued: 0 } }), WT)).toBe(k);
  });

  it("is empty for a worktree the snapshot does not list; a branch named like an object's members has no pull request it does not have", () => {
    expect(branchFilesKey(shot(), "/elsewhere")).toBe("");
    const s = shot({ worktrees: [worktree({ path: WT, branch: "constructor" })] });
    expect(JSON.parse(branchFilesKey(s, WT))[4]).toBe(null);
    // A remote named like an object's members has no default branch it does not have.
    expect(() => branchFilesKey(shot({ remotes: ["constructor"], remoteHeads: {} }), WT)).not.toThrow();
    expect(JSON.parse(branchFilesKey(shot({ remotes: ["constructor"], remoteHeads: {} }), WT))[7]).toEqual([]);
  });
});

describe("the remote branches' signature", () => {
  it("is the same for the same branches and commits, and moves with any of them", () => {
    const a = shot();
    expect(remoteTipsSignature(a)).toBe(remoteTipsSignature(shot()));
    const renamed = shot({ remoteBranches: ["origin/main", "origin/devel", "origin/feature/x"] });
    expect(remoteTipsSignature(renamed)).not.toBe(remoteTipsSignature(a));
    // A name and a commit cannot trade places.
    const swapped = shot({ remoteBranches: ["origin/main", "origin/develop", "origin/feature/x"], remoteBranchTips: [tip("1".repeat(40)), tip("a".repeat(40)), tip("2".repeat(40))] });
    expect(remoteTipsSignature(swapped)).not.toBe(remoteTipsSignature(a));
    expect(remoteTipsSignature(shot({ remoteBranches: [], remoteBranchTips: [] }))).toBe("0:811c9dc5");
  });
});

describe("asking for the list", () => {
  beforeEach(() => {
    asked.length = 0;
    forgetBranchFiles("/r");
  });

  it("takes an answer only while it is the latest asked for", async () => {
    const older = loadBranchFiles(1, "/r", WT, "k1");
    const newer = loadBranchFiles(1, "/r", WT, "k2");
    expect(branchFilesOf("/r", WT)?.loading).toBe(true);
    asked[1].resolve(answer("origin/develop", 2));
    await newer;
    asked[0].resolve(answer("origin/main", 1));
    await older;
    const e = branchFilesOf("/r", WT)!;
    expect(e.data?.base).toBe("origin/develop");
    expect(e.data?.files).toHaveLength(2);
    expect(e.key).toBe("k2");
    expect(e.loading).toBe(false);
  });

  it("an answer of none, then a list once something moved", async () => {
    const none = loadBranchFiles(1, "/r", WT, "k1");
    asked[0].resolve(null);
    await none;
    expect(branchFilesOf("/r", WT)).toMatchObject({ data: null, error: null, key: "k1" });
    const some = loadBranchFiles(1, "/r", WT, "k2");
    asked[1].resolve(answer("origin/main", 1));
    await some;
    expect(branchFilesOf("/r", WT)).toMatchObject({ key: "k2", error: null });
    expect(branchFilesOf("/r", WT)?.data?.files).toHaveLength(1);
  });

  it("a failure keeps what was shown, says why in one line, and leaves its key unanswered so it is asked again (review)", async () => {
    const first = loadBranchFiles(1, "/r", WT, "k1");
    asked[0].resolve(answer("origin/main"));
    await first;
    const failed = loadBranchFiles(1, "/r", WT, "k2");
    asked[1].reject(new Error("git failed\nmore"));
    await failed;
    let e = branchFilesOf("/r", WT)!;
    expect(e.data?.base).toBe("origin/main");
    expect(e.error).toBe("git failed");
    expect(e.key).toBe(null);
    // Asked again for the same key (Retry): answered, the error gone.
    const again = loadBranchFiles(1, "/r", WT, "k2");
    asked[2].resolve(answer("origin/main", 2));
    await again;
    e = branchFilesOf("/r", WT)!;
    expect(e).toMatchObject({ key: "k2", error: null });
    expect(e.data?.files).toHaveLength(2);
    // A failure with nothing shown before: no data, the reason kept.
    forgetBranchFiles("/r");
    const lone = loadBranchFiles(1, "/r", WT, "k3");
    asked[3].reject({ code: "git_failed", message: "boom" });
    await lone;
    expect(branchFilesOf("/r", WT)).toMatchObject({ data: null, error: "boom", key: null, loading: false });
  });

  it("each worktree has its own", async () => {
    const a = loadBranchFiles(1, "/r", WT, "k");
    const b = loadBranchFiles(1, "/r", "/r", "k");
    asked[1].resolve(answer("origin/main", 3));
    asked[0].resolve(answer("origin/main", 1));
    await Promise.all([a, b, settle()]);
    expect(branchFilesOf("/r", WT)?.data?.files).toHaveLength(1);
    expect(branchFilesOf("/r", "/r")?.data?.files).toHaveLength(3);
  });
});

describe("whether Committed on branch is open", () => {
  it("is closed by default with uncommitted changes and open without", () => {
    const fold = new BranchFold();
    expect(fold.isOpen(WT, true)).toBe(false);
    expect(fold.isOpen(WT, false)).toBe(true);
  });

  it("follows the user's click while the worktree's changes stay as they were", () => {
    const fold = new BranchFold();
    fold.see(WT, true);
    fold.toggle(WT, true);
    expect(fold.isOpen(WT, true)).toBe(true);
    fold.see(WT, true);
    expect(fold.isOpen(WT, true)).toBe(true);
    fold.toggle(WT, true);
    expect(fold.isOpen(WT, true)).toBe(false);
  });

  it("goes back to the default when the worktree goes from changes to none, or back", () => {
    const fold = new BranchFold();
    // Clean, closed by the user; changes come: closed (the default then); all committed: open again.
    fold.see(WT, false);
    fold.toggle(WT, false);
    expect(fold.isOpen(WT, false)).toBe(false);
    fold.see(WT, true);
    expect(fold.isOpen(WT, true)).toBe(false);
    fold.see(WT, false);
    expect(fold.isOpen(WT, false)).toBe(true);
    // With changes, opened by the user; committed: open (the default); a file written: closed.
    fold.see(WT, true);
    fold.toggle(WT, true);
    expect(fold.isOpen(WT, true)).toBe(true);
    fold.see(WT, false);
    expect(fold.isOpen(WT, false)).toBe(true);
    fold.see(WT, true);
    expect(fold.isOpen(WT, true)).toBe(false);
  });

  it("goes back to the default when another worktree shows", () => {
    const fold = new BranchFold();
    fold.see(WT, false);
    fold.toggle(WT, false);
    fold.see("/r", false);
    expect(fold.isOpen("/r", false)).toBe(true);
    fold.see(WT, false);
    expect(fold.isOpen(WT, false)).toBe(true);
  });
});
