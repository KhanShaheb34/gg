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

import { BranchFold, branchFilesKey, branchFilesOf, forgetBranchFiles, loadBranchFiles } from "./branchFiles.svelte";

const WT = "/r.worktrees/feature-x";
const tip = (hash: string) => ({ hash, time: 0, author: "Ada", subject: "s" });

/** A snapshot with the feature/x worktree at `head`, tracking origin/feature/x, origin's HEAD main. */
function shot(o: Partial<RepoSnapshot> = {}, head = "a".repeat(40)): RepoSnapshot {
  return snap({
    worktrees: [worktree({ path: WT, head, upstream: { name: "origin/feature/x", ahead: 1, behind: 0 } })],
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
    const k = branchFilesKey(shot(), WT, "origin/main");
    expect(branchFilesKey(shot({}, "b".repeat(40)), WT, "origin/main")).not.toBe(k);
    expect(branchFilesKey(shot({ worktrees: [worktree({ path: WT, head: "a".repeat(40), branch: "other" })] }), WT, "origin/main")).not.toBe(k);
    expect(branchFilesKey(shot({ worktrees: [worktree({ path: WT, head: "a".repeat(40), upstream: null })] }), WT, "origin/main")).not.toBe(k);
    expect(branchFilesKey(shot({ branchPrs: { "feature/x": pullRequest({ base: "develop" }) } }), WT, "origin/main")).not.toBe(k);
    expect(branchFilesKey(shot({ remoteHeads: { origin: "develop" } }), WT, "origin/main")).not.toBe(k);
    expect(branchFilesKey(shot({ remotes: ["origin", "upstream"] }), WT, "origin/main")).not.toBe(k);
    // The base's commit moved (a fetch); another branch's did not matter.
    const moved = shot();
    moved.remoteBranchTips = [tip("9".repeat(40)), tip("2".repeat(40)), tip("a".repeat(40))];
    expect(branchFilesKey(moved, WT, "origin/main")).not.toBe(k);
    const other = shot();
    other.remoteBranchTips = [tip("1".repeat(40)), tip("8".repeat(40)), tip("a".repeat(40))];
    expect(branchFilesKey(other, WT, "origin/main")).toBe(k);
    // A local base's commit too.
    expect(branchFilesKey(shot(), WT, "main")).not.toBe(branchFilesKey(shot({ branchTips: [tip("7".repeat(40)), tip("c".repeat(40))] }), WT, "main"));
  });

  it("does not move with what the list does not depend on: a new snapshot, files written, an operation", () => {
    const k = branchFilesKey(shot(), WT, "origin/main");
    expect(branchFilesKey(shot({ seq: 99, generation: 7, status: { changed: 3, untracked: 1, conflicted: 0, conflictedPaths: [] } }), WT, "origin/main")).toBe(k);
    expect(branchFilesKey(shot({ op: { id: 1, title: "Fetch", startedAtMs: 1, queued: 0 } }), WT, "origin/main")).toBe(k);
  });

  it("is empty for a worktree the snapshot does not list; a branch named like an object's members has no pull request it does not have", () => {
    expect(branchFilesKey(shot(), "/elsewhere", null)).toBe("");
    const s = shot({ worktrees: [worktree({ path: WT, branch: "constructor" })] });
    expect(JSON.parse(branchFilesKey(s, WT, null))[4]).toBe(null);
  });
});

describe("asking for the list", () => {
  beforeEach(() => {
    asked.length = 0;
    forgetBranchFiles("/r");
  });

  it("takes an answer only while it is the latest asked for, and is then taken as asked for its base's key", async () => {
    const keyOf = (base: string | null) => `key:${base}`;
    const older = loadBranchFiles(1, "/r", WT, "k1", keyOf);
    const newer = loadBranchFiles(1, "/r", WT, "k2", keyOf);
    expect(branchFilesOf("/r", WT)?.loading).toBe(true);
    asked[1].resolve(answer("origin/develop", 2));
    await newer;
    asked[0].resolve(answer("origin/main", 1));
    await older;
    const e = branchFilesOf("/r", WT)!;
    expect(e.data?.base).toBe("origin/develop");
    expect(e.data?.files).toHaveLength(2);
    expect(e.key).toBe("key:origin/develop");
    expect(e.loading).toBe(false);
  });

  it("no list is null; a failure keeps what was shown and says why in one line", async () => {
    const keyOf = (base: string | null) => `key:${base}`;
    const first = loadBranchFiles(1, "/r", WT, "k1", keyOf);
    asked[0].resolve(answer("origin/main"));
    await first;
    const failed = loadBranchFiles(1, "/r", WT, "k2", keyOf);
    asked[1].reject(new Error("git failed\nmore"));
    await failed;
    let e = branchFilesOf("/r", WT)!;
    expect(e.data?.base).toBe("origin/main");
    expect(e.error).toBe("git failed");
    // What it was asked for stays: the next ask is for something that moved.
    expect(e.key).toBe("k2");
    const none = loadBranchFiles(1, "/r", WT, "k3", keyOf);
    asked[2].resolve(null);
    await none;
    e = branchFilesOf("/r", WT)!;
    expect(e.data).toBe(null);
    expect(e.error).toBe(null);
    expect(e.key).toBe("key:null");
  });

  it("each worktree has its own", async () => {
    const keyOf = () => "k";
    const a = loadBranchFiles(1, "/r", WT, "k", keyOf);
    const b = loadBranchFiles(1, "/r", "/r", "k", keyOf);
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
