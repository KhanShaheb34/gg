import { describe, expect, it } from "vitest";
import { disabled, find, ids, RUNNING, snap, worktree } from "./menuTestData";
import { worktreeMenu, worktreePanelMenu } from "./worktreeMenu";

const IDS = ["make_active", "-", "push", "-", "copy.path", "-", "delete"];

describe("worktreeMenu", () => {
  it("another worktree on a branch: make active, push its branch, copy the path, delete", () => {
    const items = worktreeMenu(worktree(), snap());
    expect(ids(items)).toEqual(IDS);
    expect(disabled(items)).toEqual([]);
    expect(find(items, "push").label).toBe("Push feature/x…");
    expect(find(items, "delete").danger).toBe(true);
  });

  it("leaves out what is not built yet: no commit, pull request, terminal, reveal or editor items", () => {
    const all = ids(worktreeMenu(worktree(), snap()));
    for (const id of ["commit", "pr.create", "pr.open", "open.editor", "open.terminal", "open.reveal"]) {
      expect(all).not.toContain(id);
    }
  });

  it("the active worktree cannot be made active again", () => {
    const items = worktreeMenu(worktree({ active: true }), snap());
    expect(disabled(items)).toEqual(["make_active"]);
    expect(find(items, "make_active").disabledReason).toBe("Already the active worktree");
  });

  it("a worktree whose folder is missing can be neither made active nor deleted", () => {
    const items = worktreeMenu(worktree({ prunable: true }), snap());
    expect(disabled(items)).toEqual(["make_active", "delete"]);
    expect(find(items, "make_active").disabledReason).toBe("Its folder is missing");
    expect(find(items, "delete").disabledReason).toBe("Its folder is missing: prune stale entries instead");
  });

  it("the main worktree cannot be deleted", () => {
    const items = worktreeMenu(worktree({ isMain: true, path: "/r", branch: "main" }), snap());
    expect(find(items, "delete").disabledReason).toBe("The main worktree cannot be deleted");
  });

  it("a locked worktree shows its lock, with the reason, and cannot be deleted", () => {
    const items = worktreeMenu(worktree({ locked: true, lockReason: "on a USB drive" }), snap());
    expect(ids(items)).toEqual(["make_active", "-", "push", "-", "locked", "copy.path", "-", "delete"]);
    expect(find(items, "locked").label).toBe("Locked: on a USB drive");
    expect(find(items, "locked").disabledReason).toMatch(/git worktree unlock/);
    expect(find(items, "delete").disabledReason).toBe("It is locked (on a USB drive): unlock it first");
    const bare = worktreeMenu(worktree({ locked: true }), snap());
    expect(find(bare, "locked").label).toBe("Locked");
    expect(find(bare, "delete").disabledReason).toBe("It is locked: unlock it first");
  });

  it("a detached worktree has no branch to push", () => {
    const items = worktreeMenu(worktree({ branch: null }), snap());
    expect(find(items, "push").label).toBe("Push…");
    expect(find(items, "push").disabledReason).toBe("HEAD is detached there: there is no branch to push");
  });

  it("without a remote nothing can be pushed", () => {
    const items = worktreeMenu(worktree(), snap({ remotes: [] }));
    expect(find(items, "push").disabledReason).toBe("This repository has no remote to push to");
  });

  it("no switch while an operation runs", () => {
    const items = worktreeMenu(worktree(), snap(RUNNING));
    expect(find(items, "make_active").disabledReason).toBe("An operation is running in this repository");
  });

  it("a long branch name is shortened in the push item", () => {
    const long = `feature/${"x".repeat(80)}`;
    expect(find(worktreeMenu(worktree({ branch: long }), snap()), "push").label).toMatch(/^Push feature\/x+…x+…$/);
  });
});

describe("worktreePanelMenu", () => {
  it("adds a worktree, and prunes only when a worktree's folder is missing", () => {
    const items = worktreePanelMenu(snap({ worktrees: [worktree()] }));
    expect(ids(items)).toEqual(["add", "-", "prune"]);
    expect(find(items, "prune").disabledReason).toBe("No worktree's folder is missing");
    expect(disabled(worktreePanelMenu(snap({ worktrees: [worktree({ prunable: true })] })))).toEqual([]);
  });

  it("a repository without commits has nothing to check out in a worktree", () => {
    const items = worktreePanelMenu(snap({ head: { branch: "main", hash: null } }));
    expect(find(items, "add").disabledReason).toBe("The repository has no commits yet");
  });
});
