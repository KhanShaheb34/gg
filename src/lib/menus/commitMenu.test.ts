import { describe, expect, it } from "vitest";
import { commitMenu } from "./commitMenu";
import { DETACHED, disabled, find, HEAD_HASH, ids, inProgress, row, RUNNING, snap } from "./menuTestData";

describe("commitMenu", () => {
  it("lists the commit actions of the spec, in groups", () => {
    const items = commitMenu(row(), snap());
    expect(ids(items)).toEqual([
      "checkout",
      "-",
      "branch.create",
      "tag.create",
      "-",
      "cherry_pick",
      "revert",
      "reset",
      "-",
      "copy.hash",
      "copy.message",
    ]);
    expect(ids(find(items, "reset").submenu!)).toEqual(["reset.soft", "reset.mixed", "reset.hard"]);
    expect(disabled(items)).toEqual([]);
  });

  it("disables what the core refuses while files still have conflicts nothing in progress explains (R20')", () => {
    const conflicted = { status: { changed: 1, untracked: 0, conflicted: 1, conflictedPaths: ["a.txt"] } };
    const items = commitMenu(row(), snap(conflicted));
    expect(disabled(items)).toEqual(["checkout", "cherry_pick", "revert"]);
    expect(find(items, "revert").disabledReason).toBe("Files still have conflicts from an earlier step: resolve them first");
    // A reset is a way out: it stays.
    expect(find(items, "reset").disabledReason).toBeUndefined();
  });

  it("names the current branch in reset, and marks a hard reset as dangerous", () => {
    const items = commitMenu(row(), snap());
    expect(find(items, "reset").label).toBe("Reset main to here");
    expect(find(items, "reset.hard").danger).toBe(true);
    expect(find(items, "reset.soft").danger).toBeFalsy();
    expect(find(items, "reset.mixed").danger).toBeFalsy();
  });

  it("on a detached HEAD resets HEAD, and checkout of HEAD's own commit is disabled", () => {
    const elsewhere = commitMenu(row(), snap(DETACHED));
    expect(find(elsewhere, "reset").label).toBe("Reset HEAD to here");
    expect(disabled(elsewhere)).toEqual([]);

    const here = commitMenu(row({ hash: HEAD_HASH, isHead: true }), snap(DETACHED));
    expect(find(here, "checkout").disabledReason).toBe("HEAD is already detached at this commit");
  });

  it("cannot cherry-pick HEAD's own commit onto itself", () => {
    const items = commitMenu(row({ hash: HEAD_HASH, isHead: true }), snap());
    expect(disabled(items)).toEqual(["cherry_pick"]);
    expect(find(items, "cherry_pick").disabledReason).toBe("This commit is already HEAD");
  });

  it("offers cherry-pick and revert on a merge commit (the dialog asks for the parent)", () => {
    const items = commitMenu(row({ isMerge: true, parents: ["1".repeat(40), "2".repeat(40)] }), snap());
    expect(find(items, "cherry_pick").disabledReason).toBeUndefined();
    expect(find(items, "revert").disabledReason).toBeUndefined();
  });

  it.each(["merge", "rebase", "cherry-pick", "revert"] as const)(
    "while a %s is in progress, disables only what the core refuses",
    (kind) => {
      const items = commitMenu(row(), snap(inProgress(kind)));
      expect(disabled(items)).toEqual(["checkout", "cherry_pick", "revert"]);
      const article = "A";
      expect(find(items, "checkout").disabledReason).toBe(`${article} ${kind} is in progress`);
    },
  );

  it("keeps every item enabled while an operation runs (requests queue)", () => {
    expect(disabled(commitMenu(row(), snap(RUNNING)))).toEqual([]);
  });

  it("shows control characters and bidi marks in a branch name visibly", () => {
    const items = commitMenu(row(), snap({ head: { branch: "ma‮in", hash: HEAD_HASH } }));
    expect(find(items, "reset").label).toBe("Reset ma‹U+202E›in to here");
  });

  it("shortens a very long branch name in the middle", () => {
    const long = `feature/${"very-long-name-".repeat(6)}end`;
    const label = find(commitMenu(row(), snap({ head: { branch: long, hash: HEAD_HASH } })), "reset").label;
    expect(label.length).toBeLessThan(70);
    expect(label).toMatch(/^Reset feature\/very-long.*….*-end to here$/);
  });
});
