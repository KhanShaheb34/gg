import { afterEach, describe, expect, it, vi } from "vitest";
import {
  buildIndex,
  localNameOf,
  matching,
  needleOf,
  pickerIndex,
  pickerList,
  ROW_H,
  rowAt,
  visibleRange,
  type PickerBranch,
  type PickerList,
  type PickerRow,
} from "./branchPicker";
import { snap as baseSnap } from "./menus/menuTestData";
import type { BranchTip, RepoSnapshot, WorktreeInfo } from "./types";

const tip = (hash: string, time: number, subject = `Commit ${hash[0]}`, author = "Ada"): BranchTip => ({
  hash: hash.padEnd(40, hash[0]),
  time,
  author,
  subject,
});

function worktree(path: string, branch: string | null, active = false): WorktreeInfo {
  return {
    path,
    head: "a".repeat(40),
    branch,
    isMain: active,
    locked: false,
    prunable: false,
    active,
    status: null,
    upstream: null,
    unpushed: null,
    inProgress: null,
    lockReason: null,
    pr: null,
  };
}

/** A repository: main (current), feature/login (newest), staging, old; origin's three. */
function repo(overrides: Partial<RepoSnapshot> = {}): RepoSnapshot {
  return baseSnap({
    head: { branch: "main", hash: "m".padEnd(40, "m") },
    branches: ["feature/login", "main", "old", "staging"],
    branchTips: [tip("f", 400, "Validate login"), tip("m", 300, "Tidy imports"), tip("o", 100), tip("s", 200)],
    remoteBranches: ["origin/feature/login", "origin/main", "origin/staging"],
    remoteBranchTips: [tip("f", 400), tip("m", 300), tip("t", 500, "Hotfix two", "Grace")],
    remotes: ["origin"],
    worktrees: [worktree("/r", "main", true), worktree("/r.worktrees/staging", "staging")],
    ...overrides,
  });
}

/** Every row of `list`, as kind and name. */
function rows(list: PickerList): string[] {
  const out: string[] = [];
  for (let i = 0; i < list.length; i++) out.push(describeRow(rowAt(list, i)));
  return out;
}

function describeRow(row: PickerRow): string {
  switch (row.kind) {
    case "branch":
      return `${row.first ? "^" : ""}${row.branch.name}`;
    case "head":
      return "HEAD";
    case "create":
      return `create:${row.name}`;
    case "createFrom":
      return `from:${row.name}`;
  }
}

describe("the index", () => {
  it("lists each group newest commit first, ties by name, with what a row shows", () => {
    const index = buildIndex(repo());
    expect(index.local.map((b) => b.name)).toEqual(["feature/login", "main", "staging", "old"]);
    expect(index.remote.map((b) => b.name)).toEqual(["origin/staging", "origin/feature/login", "origin/main"]);
    const login = index.local[0];
    expect(login).toMatchObject({ remote: false, time: 400, author: "Ada", subject: "Validate login", key: "feature/login" });
    expect(login.hash).toBe("f".repeat(40));
    expect(index.remote[0]).toMatchObject({ author: "Grace", subject: "Hotfix two", remote: true, current: false, worktree: null });

    const tied = buildIndex(repo({ branches: ["b", "a", "c"], branchTips: [tip("b", 5), tip("a", 5), tip("c", 9)] }));
    expect(tied.local.map((b) => b.name)).toEqual(["c", "a", "b"]);
  });

  it("marks the current branch and those checked out in another worktree", () => {
    const index = buildIndex(repo());
    const by = (name: string) => index.byName.get(name)!;
    expect(by("main")).toMatchObject({ current: true, worktree: null });
    expect(by("staging")).toMatchObject({ current: false, worktree: "/r.worktrees/staging" });
    expect(by("old")).toMatchObject({ current: false, worktree: null });
    // A remote branch is never either.
    expect(index.remote.every((b) => !b.current && b.worktree === null)).toBe(true);
    // Detached: no branch is current.
    const detached = buildIndex(repo({ head: { branch: null, hash: "o".repeat(40) } }));
    expect(detached.local.some((b) => b.current)).toBe(false);
  });

  it("knows HEAD's commit and the branch it belongs to, if any", () => {
    expect(buildIndex(repo()).head).toMatchObject({ hash: "m".repeat(40), branch: { name: "main" } });
    const detached = buildIndex(repo({ head: { branch: null, hash: "o".repeat(40) } }));
    expect(detached.head).toMatchObject({ hash: "o".repeat(40), branch: { name: "old" } });
    const nowhere = buildIndex(repo({ head: { branch: null, hash: "z".repeat(40) } }));
    expect(nowhere.head).toEqual({ hash: "z".repeat(40), branch: null });
    expect(buildIndex(repo({ head: { branch: "main", hash: null } })).head).toBeNull();
  });

  it("is made once per set of branches: a snapshot that only changes something else reuses it", () => {
    const first = repo();
    const index = pickerIndex(first);
    // A new snapshot object (a status change): equal branches, new arrays.
    const again = { ...first, seq: 9, status: { ...first.status, changed: 3 }, branches: [...first.branches], branchTips: first.branchTips.map((t) => ({ ...t })) };
    expect(pickerIndex(again)).toBe(index);
    // A branch moved: a new index.
    const moved = { ...again, branchTips: again.branchTips.map((t, i) => (i === 0 ? { ...t, hash: "9".repeat(40) } : t)) };
    expect(pickerIndex(moved)).not.toBe(index);
    // HEAD moved to another branch, or a worktree changed branch: a new index.
    const switched = pickerIndex(moved);
    expect(pickerIndex({ ...moved, head: { ...moved.head, branch: "old" } })).not.toBe(switched);
    const wt = pickerIndex(moved);
    expect(pickerIndex({ ...moved, worktrees: [moved.worktrees[0]] })).not.toBe(wt);
  });
});

describe("matching", () => {
  it("is the branch filter's: the trimmed text, any case, anywhere in the name", () => {
    const index = buildIndex(repo());
    const names = (text: string) => matching(index.local, needleOf(text), text.trim()).map((b) => b.name);
    expect(names("LOG")).toEqual(["feature/login"]);
    expect(names("  in ")).toEqual(["feature/login", "main", "staging"]);
    expect(names("")).toEqual(["feature/login", "main", "staging", "old"]);
    expect(names("xyz")).toEqual([]);
    // Substring, not fuzzy: letters in order but apart do not match.
    expect(names("fl")).toEqual([]);
  });

  it("puts the branch named exactly as typed first, the rest newest first", () => {
    const index = buildIndex(repo({ branches: ["main-old", "main"], branchTips: [tip("a", 900), tip("b", 1)] }));
    expect(matching(index.local, "main", "main").map((b) => b.name)).toEqual(["main", "main-old"]);
    expect(matching(index.local, "mai", "mai").map((b) => b.name)).toEqual(["main-old", "main"]);
  });

  it("returns the index's own entries: nothing is copied", () => {
    const index = buildIndex(repo());
    expect(matching(index.local, "", "")).toBe(index.local);
    expect(matching(index.local, "log", "log")[0]).toBe(index.local[0]);
  });
});

describe("the rows of each step", () => {
  it("lists local, then remote branches, then the two ways to make one", () => {
    const index = buildIndex(repo());
    expect(rows(pickerList(index, { kind: "list" }, ""))).toEqual([
      "^feature/login",
      "main",
      "staging",
      "old",
      "^origin/staging",
      "origin/feature/login",
      "origin/main",
      "create:",
      "from:",
    ]);
    expect(rows(pickerList(index, { kind: "list" }, "log"))).toEqual(["^feature/login", "^origin/feature/login", "create:log", "from:log"]);
  });

  it("with no branch matching, the first row creates the branch typed", () => {
    const list = pickerList(buildIndex(repo()), { kind: "list" }, "  brand-new ");
    expect(rows(list)).toEqual(["create:brand-new", "from:brand-new"]);
    expect(rowAt(list, 0)).toEqual({ kind: "create", name: "brand-new", at: null });
  });

  it("offers a local branch named exactly as typed, not to make it again", () => {
    expect(rows(pickerList(buildIndex(repo()), { kind: "list" }, "main"))).toEqual(["^main", "^origin/main", "from:main"]);
    // A remote branch's name is no local branch's: it can be made.
    expect(rows(pickerList(buildIndex(repo()), { kind: "list" }, "origin/main"))).toEqual(["^origin/main", "create:origin/main", "from:origin/main"]);
  });

  it("from: HEAD first, then the branches, filtered alike; no rows to make one", () => {
    const index = buildIndex(repo());
    expect(rows(pickerList(index, { kind: "from", name: "x" }, ""))).toEqual([
      "HEAD",
      "^feature/login",
      "main",
      "staging",
      "old",
      "^origin/staging",
      "origin/feature/login",
      "origin/main",
    ]);
    expect(rows(pickerList(index, { kind: "from", name: null }, "sta"))).toEqual(["^staging", "^origin/staging"]);
    expect(rows(pickerList(index, { kind: "from", name: null }, "he"))).toEqual(["HEAD"]);
    expect(rows(pickerList(index, { kind: "from", name: null }, "mmmm"))).toEqual(["HEAD"]);
    // No commit yet: no HEAD row.
    expect(rows(pickerList(buildIndex(repo({ head: { branch: "main", hash: null } })), { kind: "from", name: null }, ""))[0]).toBe(
      "^feature/login",
    );
  });

  it("name: one row, making the branch typed where it was chosen", () => {
    const index = buildIndex(repo());
    const at = { kind: "branch" as const, branch: index.local[2] };
    const list = pickerList(index, { kind: "name", at }, " fix ");
    expect(rows(list)).toEqual(["create:fix"]);
    expect(rowAt(list, 0)).toEqual({ kind: "create", name: "fix", at });
  });
});

describe("the rows rendered", () => {
  it("are those in view and a few either side", () => {
    expect(visibleRange(10_000, 0, 10 * ROW_H)).toEqual([0, 14]);
    expect(visibleRange(10_000, 5000 * ROW_H, 10 * ROW_H)).toEqual([4996, 5014]);
    expect(visibleRange(10_000, 9995 * ROW_H, 10 * ROW_H)).toEqual([9991, 10_000]);
    expect(visibleRange(3, 0, 10 * ROW_H)).toEqual([0, 3]);
    expect(visibleRange(0, 0, 10 * ROW_H)).toEqual([0, 0]);
  });
});

it("a remote branch is checked out as its name on the remote", () => {
  const index = buildIndex(repo({ remotes: ["origin", "origin/fork"], remoteBranches: ["origin/fork/feature/x"], remoteBranchTips: [tip("a", 1)] }));
  expect(localNameOf(index.remote[0], ["origin", "origin/fork"])).toBe("feature/x");
  const plain = buildIndex(repo()).remote.find((b) => b.name === "origin/feature/login")!;
  expect(localNameOf(plain, ["origin"])).toBe("feature/login");
  expect(localNameOf(plain, [])).toBe("origin/feature/login");
});

// ---- Speed -------------------------------------------------------------------------------
// 10,000 local and 5,000 remote branches. What is asserted first is the work: the index is
// built once and reused, a keystroke lowercases only the text and copies no branch, and the
// rows exist only as asked for. The time bounds come after, far above what this takes on a
// developer's machine (measured: see docs/performance.md), so that a CI runner several times
// slower, under load, does not fail them; they catch a change that makes the work grow.

const LOCAL = 10_000;
const REMOTE = 5_000;

function many(): RepoSnapshot {
  const words = ["feature", "fix", "release", "chore", "spike", "docs"];
  const branches: string[] = [];
  const branchTips: BranchTip[] = [];
  for (let i = 0; i < LOCAL; i++) {
    branches.push(`${words[i % words.length]}/team-${i % 37}/topic-${String(i).padStart(5, "0")}`);
    branchTips.push(tip(i.toString(16).padStart(8, "0"), 1_700_000_000 + ((i * 7919) % 100_000), `Change number ${i}`, `Author ${i % 50}`));
  }
  const remoteBranches = branches.slice(0, REMOTE).map((b) => `origin/${b}`);
  const remoteBranchTips = branchTips.slice(0, REMOTE);
  const order = branches.map((_, i) => i).sort((a, b) => (branches[a] < branches[b] ? -1 : 1));
  return repo({
    head: { branch: branches[order[0]], hash: branchTips[order[0]].hash },
    branches: order.map((i) => branches[i]),
    branchTips: order.map((i) => branchTips[i]),
    remoteBranches: [...remoteBranches].sort(),
    remoteBranchTips,
    worktrees: [],
  });
}

function median(f: () => void, runs = 9): number {
  const times: number[] = [];
  for (let i = 0; i < runs; i++) {
    const t = performance.now();
    f();
    times.push(performance.now() - t);
  }
  return times.sort((a, b) => a - b)[Math.floor(runs / 2)];
}

/** What the picker does for a field holding `text`: its list, and the rows of the first screen. */
function screen(index: ReturnType<typeof buildIndex>, text: string): PickerRow[] {
  const list = pickerList(index, { kind: "list" }, text);
  const [first, last] = visibleRange(list.length, 0, 10 * ROW_H);
  const out: PickerRow[] = [];
  for (let i = first; i < last; i++) out.push(rowAt(list, i));
  return out;
}

describe("10,000 branches", () => {
  afterEach(() => vi.restoreAllMocks());

  it("opening builds the index once; a keystroke lowercases only the text and makes only the rows shown", () => {
    const s = many();
    const index = pickerIndex(s);
    expect(index.local).toHaveLength(LOCAL);
    expect(index.remote).toHaveLength(REMOTE);
    // A later snapshot of the same branches (a file saved) builds nothing.
    expect(pickerIndex({ ...s, seq: s.seq + 1, branches: [...s.branches] })).toBe(index);

    const lower = vi.spyOn(String.prototype, "toLowerCase");
    const rowsShown = screen(index, "Topic-0777");
    expect(lower).toHaveBeenCalledTimes(1);
    expect(rowsShown.length).toBeLessThanOrEqual(14);
    // Every match is one of the index's own entries: none is copied or re-made.
    const list = pickerList(index, { kind: "list" }, "topic-0777");
    expect(list.local).toHaveLength(10);
    for (const b of list.local) expect(index.local).toContain(b as PickerBranch);
    expect(list.remote).toHaveLength(0); // origin has the first 5,000 only
    expect(list.length).toBe(10 + 2);
  });

  it("builds the list on open well under 50 ms, and filters a keystroke well under 16 ms", () => {
    const s = many();
    const open = median(() => {
      const index = buildIndex(s);
      screen(index, "");
    });
    const index = buildIndex(s);
    const texts = ["f", "fe", "fea", "feat", "featu", "feature/team-1", "topic-0", "topic-07", "TOPIC-077", "zzz"];
    let k = 0;
    const keystroke = median(() => {
      screen(index, texts[k++ % texts.length]);
    }, 21);
    console.log(`branch picker, ${LOCAL} + ${REMOTE} branches: list built on open ${open.toFixed(2)} ms, a keystroke ${keystroke.toFixed(2)} ms (medians)`);
    // Targets: 50 ms and 16 ms; bounds at their size, many times what this takes here.
    expect(open).toBeLessThan(50);
    expect(keystroke).toBeLessThan(16);
  });
});
