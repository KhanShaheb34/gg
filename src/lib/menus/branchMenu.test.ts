import { describe, expect, it } from "vitest";
import { branchMenu } from "./branchMenu";
import { DETACHED, disabled, find, ids, inProgress, label, row, RUNNING, snap, UNBORN } from "./menuTestData";

const LOCAL_IDS = ["checkout", "-", "rename", "delete", "-", "merge", "rebase", "-", "push", "pull", "-", "copy.name"];
const REMOTE_IDS = ["checkout", "fetch_into_local", "-", "merge", "-", "delete", "-", "copy.name"];

describe("branchMenu, local branch", () => {
  it("another local branch: everything of the spec, merge and rebase naming both branches", () => {
    const items = branchMenu(label({ upstream: "origin/feature/x" }), row(), snap());
    expect(ids(items)).toEqual(LOCAL_IDS);
    expect(disabled(items)).toEqual([]);
    expect(find(items, "merge").label).toBe("Merge feature/x into main…");
    expect(find(items, "rebase").label).toBe("Rebase main onto feature/x…");
    expect(find(items, "delete").danger).toBe(true);
  });

  it("the current branch: no checkout, delete or merge into itself", () => {
    const items = branchMenu(label({ name: "main", current: true, upstream: "origin/main" }), row(), snap());
    expect(ids(items)).toEqual(LOCAL_IDS);
    expect(disabled(items)).toEqual(["checkout", "delete", "merge", "rebase"]);
    expect(find(items, "checkout").disabledReason).toBe("Already checked out");
    expect(find(items, "delete").disabledReason).toBe("The current branch cannot be deleted");
    expect(find(items, "merge").disabledReason).toBe("A branch cannot be merged into itself");
    expect(find(items, "rebase").disabledReason).toBe("A branch cannot be rebased onto itself");
    // Not "Merge main into main…": the items say what they do in general.
    expect(find(items, "merge").label).toBe("Merge into the current branch…");
    expect(find(items, "rebase").label).toBe("Rebase the current branch onto this…");
  });

  it("a branch checked out in another worktree cannot be checked out, deleted or pulled here", () => {
    const items = branchMenu(label({ worktreePath: "/r.worktrees/x", upstream: "origin/feature/x" }), row(), snap());
    // Pulling a branch that is not checked out here fetches into it, which git refuses there.
    expect(disabled(items)).toEqual(["checkout", "delete", "pull"]);
    expect(find(items, "checkout").disabledReason).toBe("Checked out in the worktree at /r.worktrees/x");
    expect(find(items, "pull").disabledReason).toBe("Checked out in the worktree at /r.worktrees/x: pull it there");
  });

  it("a branch behind its upstream says how many commits a pull brings", () => {
    const items = branchMenu(label({ upstream: "origin/feature/x", behind: 3 }), row(), snap());
    expect(find(items, "pull").label).toBe("Pull 3 commits");
    expect(find(items, "push").label).toBe("Push…");
    expect(disabled(items)).toEqual([]);
  });

  it("a branch ahead of its upstream says how many commits a push sends", () => {
    const items = branchMenu(label({ upstream: "origin/feature/x", ahead: 1 }), row(), snap());
    expect(find(items, "push").label).toBe("Push 1 commit…");
    expect(find(items, "pull").label).toBe("Pull");
  });

  it("a branch with no upstream cannot be pulled", () => {
    const items = branchMenu(label(), row(), snap());
    expect(disabled(items)).toEqual(["pull"]);
    expect(find(items, "pull").disabledReason).toBe("feature/x has no upstream branch to pull from");
  });

  it("a repository without remotes cannot push", () => {
    const items = branchMenu(label(), row(), snap({ remotes: [], remoteBranches: [] }));
    expect(find(items, "push").disabledReason).toBe("This repository has no remote to push to");
  });

  it("with HEAD detached, nothing can go into the current branch; pushing a branch still can", () => {
    const items = branchMenu(label({ upstream: "origin/feature/x" }), row(), snap(DETACHED));
    expect(disabled(items)).toEqual(["merge", "rebase"]);
    expect(find(items, "merge").label).toBe("Merge feature/x into the current branch…");
    expect(find(items, "merge").disabledReason).toBe("HEAD is detached: there is no current branch");
    expect(find(items, "rebase").disabledReason).toBe("HEAD is detached: there is no current branch");
  });

  it("in a repository with no commit yet, nothing can go into the current branch", () => {
    const items = branchMenu(label({ upstream: "origin/feature/x" }), row(), snap(UNBORN));
    expect(find(items, "merge").disabledReason).toBe("The current branch has no commits yet");
  });

  it("while a rebase is in progress, disables exactly what the core refuses", () => {
    const other = branchMenu(label({ upstream: "origin/feature/x" }), row(), snap(inProgress("rebase")));
    expect(disabled(other)).toEqual(["checkout", "merge", "rebase"]);
    expect(find(other, "checkout").disabledReason).toBe("A rebase is in progress");

    // Pulling the current branch is refused; pulling another only fetches into it.
    const current = branchMenu(
      label({ name: "main", current: true, upstream: "origin/main" }),
      row(),
      snap(inProgress("rebase")),
    );
    expect(find(current, "pull").disabledReason).toBe("A rebase is in progress");
    expect(find(other, "pull").disabledReason).toBeUndefined();
  });

  it("keeps every item enabled while an operation runs", () => {
    expect(disabled(branchMenu(label({ upstream: "origin/feature/x" }), row(), snap(RUNNING)))).toEqual([]);
  });
});

describe("branchMenu, remote branch", () => {
  const remote = label({ kind: "remote", name: "origin/feature/x" });

  it("lists the remote branch actions of the spec", () => {
    const items = branchMenu(remote, row(), snap());
    expect(ids(items)).toEqual(REMOTE_IDS);
    expect(disabled(items)).toEqual([]);
    expect(find(items, "checkout").label).toBe("Check out as a local branch…");
    expect(find(items, "merge").label).toBe("Merge origin/feature/x into main…");
    expect(find(items, "delete").label).toBe("Delete from origin…");
    expect(find(items, "delete").danger).toBe(true);
  });

  it("names the remote whose name is the longest prefix", () => {
    const items = branchMenu(
      label({ kind: "remote", name: "team/a/fix" }),
      row(),
      snap({ remotes: ["team", "team/a"] }),
    );
    expect(find(items, "delete").label).toBe("Delete from team/a…");
  });

  it("with HEAD detached cannot merge, and while a merge is in progress cannot check out or merge", () => {
    expect(disabled(branchMenu(remote, row(), snap(DETACHED)))).toEqual(["merge"]);
    expect(disabled(branchMenu(remote, row(), snap(inProgress("merge"))))).toEqual(["checkout", "merge"]);
  });
});
