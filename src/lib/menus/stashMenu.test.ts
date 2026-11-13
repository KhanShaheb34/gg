import { describe, expect, it } from "vitest";
import { disabled, find, ids, inProgress, label, row, RUNNING, snap, UNBORN } from "./menuTestData";
import { stashMenu, uncommittedMenu } from "./stashMenu";

const stashRow = row({ kind: "stash", subject: "On main: draft", labels: [label({ kind: "stash", name: "stash@{0}" })] });

describe("stashMenu", () => {
  it("lists apply, pop, create branch and drop", () => {
    const items = stashMenu(stashRow, snap());
    expect(ids(items)).toEqual(["stash.apply", "stash.pop", "-", "stash.branch", "-", "stash.drop"]);
    expect(disabled(items)).toEqual([]);
    expect(find(items, "stash.drop").danger).toBe(true);
  });

  it("while a cherry-pick is in progress, can still drop but not apply, pop or branch", () => {
    const items = stashMenu(stashRow, snap(inProgress("cherry-pick")));
    expect(disabled(items)).toEqual(["stash.apply", "stash.pop", "stash.branch"]);
    expect(find(items, "stash.pop").disabledReason).toBe("A cherry-pick is in progress");
  });

  it("stays enabled while an operation runs", () => {
    expect(disabled(stashMenu(stashRow, snap(RUNNING)))).toEqual([]);
  });
});

describe("uncommittedMenu", () => {
  it("offers to stash the changes", () => {
    const items = uncommittedMenu(snap({ status: { changed: 2, untracked: 1, conflicted: 0 } }));
    expect(ids(items)).toEqual(["stash.save"]);
    expect(find(items, "stash.save").label).toBe("Stash changes…");
    expect(disabled(items)).toEqual([]);
  });

  it("stays enabled while a merge is in progress, unless files are still conflicted", () => {
    const resolved = snap({ ...inProgress("merge"), status: { changed: 1, untracked: 0, conflicted: 0 } });
    expect(disabled(uncommittedMenu(resolved))).toEqual([]);
    const conflicted = snap({ ...inProgress("merge"), status: { changed: 1, untracked: 0, conflicted: 1 } });
    expect(find(uncommittedMenu(conflicted), "stash.save").disabledReason).toBe(
      "Conflicted files cannot be stashed: resolve them first",
    );
  });

  it("in a repository with no commit yet there is nothing to stash onto", () => {
    expect(find(uncommittedMenu(snap(UNBORN)), "stash.save").disabledReason).toBe(
      "Stashing needs a first commit",
    );
  });
});
