import { describe, expect, it } from "vitest";
import { disabled, find, ids, pullRequest, RUNNING, snap, worktree } from "./menuTestData";
import { worktreeMenu, worktreePanelMenu } from "./worktreeMenu";

/** A worktree with changes to commit. */
const DIRTY = { status: { changed: 2, untracked: 0, conflicted: 0, conflictedPaths: [] } };

const IDS = ["make_active", "commit", "-", "push", "pr.create", "-", "open.editor", "open.terminal", "open.reveal", "copy.path", "-", "delete"];

describe("worktreeMenu", () => {
  it("another worktree on a branch: make active, commit, push its branch, open it outside, copy the path, delete", () => {
    const items = worktreeMenu(worktree(DIRTY), snap());
    expect(ids(items)).toEqual(IDS);
    expect(disabled(items)).toEqual([]);
    expect(find(items, "push").label).toBe("Push feature/x…");
    expect(find(items, "delete").danger).toBe(true);
  });

  it("its branch's pull request: open it when there is one, else create one; none for a detached HEAD", () => {
    const withPr = worktreeMenu(worktree({ pr: pullRequest({ number: 9 }) }), snap());
    expect(find(withPr, "pr.open").label).toBe("Open pull request #9");
    expect(ids(withPr)).not.toContain("pr.create");
    expect(find(worktreeMenu(worktree(), snap()), "pr.create").label).toBe("Create pull request…");
    const detached = worktreeMenu(worktree({ branch: null }), snap());
    expect(find(detached, "pr.create").disabledReason).toBe("HEAD is detached there: there is no branch to propose");
    const signedOut = snap({ prs: { status: "unavailable", reason: "signed_out", loadedAtMs: null, partial: false } });
    // Task 10 review minor 8: the dialog decides (and says) whether it pushes first.
    expect(find(worktreeMenu(worktree(), signedOut), "pr.create").label).toBe("Create pull request…");
    const pushed = worktree({ upstream: { name: "origin/feature/x", ahead: 0, behind: 0 } });
    expect(find(worktreeMenu(pushed, signedOut), "pr.create").label).toBe("Create pull request…");
    const notGithub = snap({ prs: { status: "unavailable", reason: "not_github", loadedAtMs: null, partial: false } });
    expect(find(worktreeMenu(worktree(), notGithub), "pr.create").disabledReason).toBe("This remote is not on GitHub");
  });

  it("commits a worktree with changes; one without has nothing to commit", () => {
    const items = worktreeMenu(worktree({ status: { changed: 0, untracked: 0, conflicted: 0, conflictedPaths: [] } }), snap());
    expect(find(items, "commit").disabledReason).toBe("Nothing to commit: no file has changed");
    const changed = worktreeMenu(worktree({ status: { changed: 0, untracked: 1, conflicted: 0, conflictedPaths: [] } }), snap());
    expect(find(changed, "commit").disabledReason).toBeUndefined();
    // Not read yet: offered (the panel reads the files).
    expect(find(worktreeMenu(worktree({ status: null }), snap()), "commit").disabledReason).toBeUndefined();
  });

  it("opens the worktree in the editor, a terminal and the file manager", () => {
    const items = worktreeMenu(worktree(), snap());
    expect(find(items, "open.editor").label).toBe("Open in editor");
    expect(find(items, "open.terminal").label).toBe("Open in terminal");
    expect(find(items, "open.reveal").label).toMatch(/^(Reveal in Finder|Show in File Explorer|Open containing folder)$/);
  });

  it("the active worktree cannot be made active again", () => {
    const items = worktreeMenu(worktree({ ...DIRTY, active: true }), snap());
    expect(disabled(items)).toEqual(["make_active"]);
    expect(find(items, "make_active").disabledReason).toBe("Already the active worktree");
  });

  it("a worktree whose folder is missing can be neither made active nor deleted", () => {
    const items = worktreeMenu(worktree({ prunable: true }), snap());
    expect(disabled(items)).toEqual(["make_active", "commit", "open.editor", "open.terminal", "open.reveal", "delete"]);
    expect(find(items, "make_active").disabledReason).toBe("Its folder is missing");
    expect(find(items, "delete").disabledReason).toBe("Its folder is missing: prune stale entries instead");
  });

  it("the main worktree cannot be deleted", () => {
    const items = worktreeMenu(worktree({ isMain: true, path: "/r", branch: "main" }), snap());
    expect(find(items, "delete").disabledReason).toBe("The main worktree cannot be deleted");
  });

  it("a locked worktree shows its lock, with the reason, and cannot be deleted", () => {
    const items = worktreeMenu(worktree({ locked: true, lockReason: "on a USB drive" }), snap());
    expect(ids(items)).toEqual([
      "make_active",
      "commit",
      "-",
      "push",
      "pr.create",
      "-",
      "locked",
      "open.editor",
      "open.terminal",
      "open.reveal",
      "copy.path",
      "-",
      "delete",
    ]);
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
