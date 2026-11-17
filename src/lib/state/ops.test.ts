import { beforeEach, describe, expect, it, vi } from "vitest";
import { snap } from "../menus/menuTestData";
import type { Danger, OpHint, OpOutcome, OpRequest, RepoSnapshot } from "../types";

vi.mock("../ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc")>();
  return {
    CoreError: actual.CoreError,
    onEvent: vi.fn(() => () => {}),
    api: {
      runOp: vi.fn(),
      saveSession: vi.fn(async () => {}),
      setActiveWorktree: vi.fn(),
      openExternal: vi.fn(async () => ({ note: null })),
    },
  };
});

// The repository's view: a stand-in that records what it is given.
const view = vi.hoisted(() => ({
  snapshot: null as unknown,
  applySnapshot: vi.fn(),
  fetch: vi.fn(async () => {}),
  loadedLabel: (() => undefined) as (name: string) => unknown,
}));
vi.mock("./repo.svelte", () => ({
  findRepoView: vi.fn((repoId: number) => (repoId === 1 ? view : undefined)),
  repoView: vi.fn(() => view),
  disposeRepoView: vi.fn(),
}));

// Dialogs: the test plays the user, through the props the dialog was opened with.
vi.mock("./overlays.svelte", () => ({ openDialog: vi.fn(), closeDialog: vi.fn(), showDialog: vi.fn() }));
vi.mock("../components/dialogs/ConfirmDialog.svelte", () => ({ default: { name: "ConfirmDialog" } }));
vi.mock("../components/dialogs/PullDialog.svelte", () => ({ default: { name: "PullDialog" } }));

type Ipc = typeof import("../ipc");
type ConfirmProps = {
  title: string;
  dangers: Danger[];
  repository: string | null;
  again: boolean;
  onConfirm: () => void;
  onCancel: () => void;
};

let ipc: Ipc;
let ops: typeof import("./ops.svelte");
let app: typeof import("./app.svelte");
let overlays: typeof import("./overlays.svelte");

function outcome(overrides: Partial<OpOutcome> = {}): OpOutcome {
  return {
    opId: 9,
    status: "done",
    title: "Merge feature/x into main",
    message: "Merged feature/x into main.",
    detail: null,
    logFrom: 0,
    logTo: 0,
    dangers: [],
    hints: [],
    created: null,
    committed: null,
    field: null,
    snapshot: snap({ seq: 5 }),
    ...overrides,
  };
}

function danger(overrides: Partial<Danger> = {}): Danger {
  return { kind: "hard_reset", message: "2 changed files will be lost.", detail: "a.txt\nb.txt", token: null, ...overrides };
}

const HEAD = { branch: "main", hash: "c".repeat(40) };
const RESET: OpRequest = { op: "reset", hash: "a".repeat(40), mode: "hard", expectHead: HEAD, expectWorktree: "/r" };
const settle = () => new Promise((r) => setTimeout(r, 0));

/** The props of the dialog opened last (the confirmation). */
function lastDialog(): ConfirmProps {
  const calls = vi.mocked(overlays.openDialog).mock.calls;
  expect(calls.length).toBeGreaterThan(0);
  return calls[calls.length - 1][1] as ConfirmProps;
}

/** Waits for the confirmation to be shown (it is asked for after the core's answer). */
async function confirmation(count: number): Promise<ConfirmProps> {
  await vi.waitFor(() => expect(overlays.openDialog).toHaveBeenCalledTimes(count));
  return lastDialog();
}

beforeEach(async () => {
  vi.resetModules();
  ipc = await import("../ipc");
  overlays = await import("./overlays.svelte");
  app = await import("./app.svelte");
  ops = await import("./ops.svelte");
  vi.mocked(ipc.api.runOp).mockReset();
  let ids = 40;
  vi.mocked(overlays.openDialog).mockReset().mockImplementation(() => ++ids);
  vi.mocked(overlays.closeDialog).mockReset();
  view.applySnapshot.mockReset();
  view.fetch.mockReset();
  view.snapshot = snap();
});

describe("runOp", () => {
  it("sends the request with nothing confirmed, applies the snapshot and shows the result", async () => {
    const done = outcome();
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(done);
    expect(await ops.runOp(1, RESET)).toBe(done);
    expect(ipc.api.runOp).toHaveBeenCalledWith(1, RESET, []);
    expect(view.applySnapshot).toHaveBeenCalledWith(done.snapshot);
    expect(overlays.openDialog).not.toHaveBeenCalled();
    expect(app.app.toasts).toEqual([
      expect.objectContaining({ kind: "info", message: "Merged feature/x into main.", actions: [] }),
    ]);
  });

  it("asks before a dangerous request and, once confirmed, sends it again with those dangers", async () => {
    const dangers = [danger()];
    vi.mocked(ipc.api.runOp)
      .mockResolvedValueOnce(outcome({ status: "needs_confirmation", title: "Reset main to aaaaaaaa", dangers }))
      .mockResolvedValueOnce(outcome({ message: "Reset main to aaaaaaaa." }));
    const run = ops.runOp(1, RESET);
    const dialog = await confirmation(1);
    expect(dialog.title).toBe("Reset main to aaaaaaaa");
    expect(dialog.dangers).toEqual(dangers);
    expect(app.app.toasts).toEqual([]);

    dialog.onConfirm();
    const result = await run;
    expect(result.status).toBe("done");
    expect(overlays.closeDialog).toHaveBeenCalledWith(41);
    expect(vi.mocked(ipc.api.runOp).mock.calls).toEqual([
      [1, RESET, []],
      [1, RESET, [{ kind: "hard_reset", token: null }]],
    ]);
    expect(app.app.toasts.map((t) => t.message)).toEqual(["Reset main to aaaaaaaa."]);
  });

  it("counts only the first answer, and closes only its own confirmation", async () => {
    vi.mocked(ipc.api.runOp)
      .mockResolvedValueOnce(outcome({ status: "needs_confirmation", dangers: [danger()] }))
      .mockResolvedValueOnce(outcome());
    const run = ops.runOp(1, RESET);
    const dialog = await confirmation(1);
    dialog.onConfirm();
    dialog.onConfirm(); // a double click
    dialog.onCancel();
    await run;
    await settle();
    expect(ipc.api.runOp).toHaveBeenCalledTimes(2);
    expect(vi.mocked(overlays.closeDialog).mock.calls).toEqual([[41]]);
  });

  it("names the repository when the operation is not on the active tab's", async () => {
    vi.mocked(ipc.api.runOp).mockResolvedValue(outcome({ status: "needs_confirmation", dangers: [danger()] }));
    view.snapshot = snap({ name: "payments-api" });
    app.app.activeRepoId = 2;
    void ops.runOp(1, RESET);
    const elsewhere = await confirmation(1);
    expect(elsewhere.repository).toBe("payments-api");
    elsewhere.onCancel();

    app.app.activeRepoId = 1;
    void ops.runOp(1, RESET);
    const here = await confirmation(2);
    expect(here.repository).toBeNull();
    here.onCancel();
  });

  it("sends nothing more when the confirmation is cancelled, and says nothing", async () => {
    const asked = outcome({ status: "needs_confirmation", dangers: [danger()] });
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(asked);
    const run = ops.runOp(1, RESET);
    (await confirmation(1)).onCancel();
    expect(await run).toBe(asked);
    await settle();
    expect(ipc.api.runOp).toHaveBeenCalledTimes(1);
    expect(overlays.closeDialog).toHaveBeenCalled();
    expect(app.app.toasts).toEqual([]);
  });

  it("confirms a force push with the remote commit it was shown, and asks again if the remote moved", async () => {
    const push: OpRequest = { op: "push", branch: "main", remote: "origin", to: null, force: true };
    const force = (token: string, message: string) =>
      outcome({ status: "needs_confirmation", dangers: [danger({ kind: "force_push", token, message })] });
    vi.mocked(ipc.api.runOp)
      .mockResolvedValueOnce(force("1111", "origin/main has 1 commit main does not."))
      .mockResolvedValueOnce(force("2222", "origin/main has 3 commits main does not."))
      .mockResolvedValueOnce(outcome({ message: "Force-pushed main to origin." }));
    const run = ops.runOp(1, push);
    (await confirmation(1)).onConfirm();
    const again = await confirmation(2);
    expect(again.dangers[0].message).toBe("origin/main has 3 commits main does not.");
    // Asked again: the confirmation says what it is about changed (R39).
    expect(lastDialog().again).toBe(true);
    again.onConfirm();
    await run;
    expect(vi.mocked(ipc.api.runOp).mock.calls).toEqual([
      [1, push, []],
      [1, push, [{ kind: "force_push", token: "1111" }]],
      [1, push, [{ kind: "force_push", token: "2222" }]],
    ]);
  });

  it("confirms every danger with the token it was shown with, the request unchanged (R39)", async () => {
    const drop: OpRequest = { op: "stash_drop", stash: "d".repeat(40), expectWorktree: "/r" };
    vi.mocked(ipc.api.runOp)
      .mockResolvedValueOnce(outcome({ status: "needs_confirmation", dangers: [danger({ kind: "drop_stash", token: "d".repeat(40) })] }))
      .mockResolvedValueOnce(outcome());
    const run = ops.runOp(1, drop);
    const first = await confirmation(1);
    expect(lastDialog().again).toBe(false);
    first.onConfirm();
    await run;
    expect(vi.mocked(ipc.api.runOp).mock.calls[1]).toEqual([1, drop, [{ kind: "drop_stash", token: "d".repeat(40) }]]);
  });

  it("an operation that stopped on conflicts is marked so for the log (shown amber, not failed)", async () => {
    const { stoppedOps } = await import("./commandLog.svelte");
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ opId: 77, status: "stopped" }));
    await ops.runOp(1, RESET);
    expect(stoppedOps.ids.has(77)).toBe(true);
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ opId: 78, status: "failed" }));
    await ops.runOp(1, RESET);
    expect(stoppedOps.ids.has(78)).toBe(false);
  });

  it("a request refused because HEAD moved since it was asked for shows the core's plain line", async () => {
    const message = "Nothing was done: it was asked for on main, but feature is checked out now. Look at it again, and ask again if it still applies.";
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ status: "failed", message }));
    await ops.runOp(1, RESET);
    expect(app.app.toasts).toEqual([expect.objectContaining({ kind: "error", message })]);
  });

  it("shows a failure with git's text folded below and a link to its log entries", async () => {
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(
      outcome({
        status: "failed",
        message: "Your local changes would be overwritten.",
        detail: "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt",
        logFrom: 41,
        logTo: 43,
      }),
    );
    await ops.runOp(1, RESET);
    const [notice] = app.app.toasts;
    expect(notice).toMatchObject({
      kind: "error",
      message: "Your local changes would be overwritten.",
      detail: "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt",
      foldDetail: true,
      persistent: true,
    });
    expect(notice.link?.label).toBe("Show log");
    expect(notice.link?.keepOpen).toBe(true);
    await notice.link!.run();
    expect(app.app.logRequest).toEqual({ repoId: 1, from: 41 });
  });

  it("shows no notice when git stops on conflicts (the banner shows it), but applies the snapshot", async () => {
    const stopped = outcome({ status: "stopped", message: "Merge stopped on conflicts.", detail: "CONFLICT…" });
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(stopped);
    await ops.runOp(1, RESET);
    expect(app.app.toasts).toEqual([]);
    expect(view.applySnapshot).toHaveBeenCalledWith(stopped.snapshot);
  });

  it("reports an error from the core and rejects with it", async () => {
    vi.mocked(ipc.api.runOp).mockRejectedValueOnce(
      new ipc.CoreError({ code: "bad_name", message: "“a..b” is not a valid branch name", detail: null }),
    );
    await expect(ops.runOp(1, RESET)).rejects.toThrow("not a valid branch name");
    expect(app.app.toasts).toEqual([
      expect.objectContaining({ kind: "error", message: "“a..b” is not a valid branch name" }),
    ]);
  });

  it("leaves the errors its caller shows itself unreported (a dialog shows a bad name beside its field)", async () => {
    const bad = new ipc.CoreError({ code: "bad_name", message: "A branch named “x” already exists.", detail: null });
    vi.mocked(ipc.api.runOp).mockRejectedValueOnce(bad);
    const create: OpRequest = { op: "create_branch", name: "x", at: "a".repeat(40), checkout: false, expectWorktree: "/r" };
    await expect(ops.runOp(1, create, { ownErrors: ["bad_name"] })).rejects.toBe(bad);
    expect(app.app.toasts).toEqual([]);

    // Others are still reported.
    vi.mocked(ipc.api.runOp).mockRejectedValueOnce(new ipc.CoreError({ code: "io", message: "Disk full", detail: null }));
    await expect(ops.runOp(1, create, { ownErrors: ["bad_name"] })).rejects.toThrow("Disk full");
    expect(app.app.toasts).toEqual([expect.objectContaining({ kind: "error", message: "Disk full" })]);
  });

  it("leaves the whole outcome to a caller that says how it went (ownOutcome): no notice, earlier ones still go", async () => {
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ opId: 3, hints: [{ kind: "fetch" }] }));
    await ops.runOp(1, RESET);
    expect(app.app.toasts).toHaveLength(1);
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ opId: 4, status: "failed", message: "gh failed." }));
    const out = await ops.runOp(1, RESET, { ownOutcome: true });
    expect(out.message).toBe("gh failed.");
    expect(app.app.toasts).toEqual([]);
  });

  it("leaves a refusal in the request's turn to a caller that shows it, and reports git's failures", async () => {
    const add: OpRequest = { op: "worktree_add", path: "../wt", branch: { existing: "feature/x" } };
    const refused = outcome({ status: "failed", message: "Nothing was created: there is a worktree at /wt already." });
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(refused);
    expect(await ops.runOp(1, add, { ownRefusals: true })).toBe(refused);
    expect(app.app.toasts).toEqual([]);
    expect(view.applySnapshot).toHaveBeenCalledWith(refused.snapshot);

    // git ran and failed (a hook): reported as always.
    const failed = outcome({ status: "failed", message: "git could not create it.", detail: "hook said no", logFrom: 4, logTo: 4 });
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(failed);
    await ops.runOp(1, add, { ownRefusals: true });
    expect(app.app.toasts).toEqual([expect.objectContaining({ kind: "error", message: "git could not create it." })]);

    // Without asking, a refusal is reported.
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(refused);
    await ops.runOp(1, add);
    expect(app.app.toasts).toHaveLength(2);
  });
});

describe("hint buttons", () => {
  it("a notice with buttons stays until it is dismissed or an operation after its own runs there", async () => {
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(
      outcome({ opId: 9, hints: [{ kind: "push", branch: "main", remote: "origin" }] }),
    );
    await ops.runOp(1, RESET);
    expect(app.app.toasts[0]).toMatchObject({ persistent: true, until: { repoId: 1, opId: 9 } });
    // Without buttons it goes by itself, as before.
    app.app.toasts.length = 0;
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ opId: 10 }));
    await ops.runOp(1, RESET);
    expect(app.app.toasts[0].persistent).toBeFalsy();
    expect(app.app.toasts[0].until).toBeUndefined();
  });

  it("goes when the outcome of a later operation on that repository arrives (review item 4)", async () => {
    const push = { kind: "push", branch: "main", remote: "origin" } as const;
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ opId: 9, hints: [push] }));
    await ops.runOp(1, RESET);
    expect(app.app.toasts).toHaveLength(1);
    // Its snapshot never showed the later one running (it ended at once).
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ opId: 11, message: "Reset main." }));
    await ops.runOp(1, RESET);
    expect(app.app.toasts.map((t) => t.message)).toEqual(["Reset main."]);
  });

  it("names the branch on its buttons", async () => {
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(
      outcome({ hints: [{ kind: "push", branch: "feature/\u202ex", remote: "origin" }] }),
    );
    await ops.runOp(1, RESET);
    expect(app.app.toasts[0].actions?.map((a) => a.label)).toEqual(["Push feature/‹U+202E›x to origin/feature/‹U+202E›x"]);
  });

  async function buttonsFor(hints: OpHint[], status: OpOutcome["status"] = "done") {
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(outcome({ status, hints }));
    await ops.runOp(1, RESET);
    vi.mocked(ipc.api.runOp).mockReset().mockResolvedValue(outcome({ message: "Followed up." }));
    return app.app.toasts[0].actions ?? [];
  }

  it("a notice's follow-up runs only where its operation ran: the request's expectWorktree, not the snapshot's", async () => {
    // Another worktree was made active while the operation ran: its snapshot shows that one.
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(
      outcome({ hints: [{ kind: "pull", branch: "main" }], snapshot: snap({ seq: 5, activeWorktree: "/r/other" }) }),
    );
    await ops.runOp(1, { ...RESET, expectWorktree: "/r/wt" });
    vi.mocked(ipc.api.runOp).mockReset().mockResolvedValue(outcome({ message: "Followed up." }));
    const [pull] = app.app.toasts[0].actions ?? [];
    await pull.run();
    const sent = vi.mocked(ipc.api.runOp).mock.calls[0][1];
    expect(sent).toMatchObject({ op: "pull", expectWorktree: "/r/wt" });
  });

  it("make_active: makes the worktree holding the branch active, and shows it", async () => {
    const after = snap({ seq: 9, activeWorktree: "/r/feature" });
    vi.mocked(ipc.api.setActiveWorktree).mockResolvedValueOnce(after);
    const [make] = await buttonsFor([{ kind: "make_active", worktree: "/r/feature" }], "failed");
    expect(make.label).toBe("Make /r/feature active");
    await make.run();
    await Promise.resolve();
    expect(ipc.api.setActiveWorktree).toHaveBeenCalledWith(1, "/r/feature");
    expect(ipc.api.runOp).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(view.applySnapshot).toHaveBeenCalledWith(after));
  });

  it("remove_index_lock: offers to remove the stale lock, in the worktree it is in, through the core's red confirmation (M5)", async () => {
    const [remove] = await buttonsFor([{ kind: "remove_index_lock", path: "/r/.git/index.lock", worktree: "/r/wt" }], "failed");
    expect(remove.label).toBe("Remove the lock file…");
    expect(remove.danger).toBe(true);
    await remove.run();
    expect(ipc.api.runOp).toHaveBeenLastCalledWith(1, { op: "remove_index_lock", expectWorktree: "/r/wt" }, []);
  });

  it("open_url: opens the page the core named (a pull request, a compare page) in the browser", async () => {
    const url = "https://github.com/octo/cat/pull/42";
    const [open] = await buttonsFor([{ kind: "open_url", url, label: "Open pull request" }]);
    expect(open.label).toBe("Open pull request");
    await open.run();
    expect(ipc.api.openExternal).toHaveBeenCalledWith(1, { target: "url", url });
    expect(ipc.api.runOp).not.toHaveBeenCalled();
  });

  it("push: pushes the branch the core named", async () => {
    const [push] = await buttonsFor([{ kind: "push", branch: "main", remote: "origin" }]);
    expect(push.label).toBe("Push main to origin/main");
    await push.run();
    expect(ipc.api.runOp).toHaveBeenCalledWith(
      1,
      { op: "push", branch: "main", remote: "origin", to: null, force: false },
      [],
    );
  });

  it("a rejected push: fetch, pull, and a separate force push that goes through the confirmation", async () => {
    const buttons = await buttonsFor(
      [{ kind: "fetch" }, { kind: "pull", branch: "main" }, { kind: "force_push", branch: "main", remote: "origin", to: null }],
      "failed",
    );
    expect(buttons.map((b) => [b.label, !!b.danger])).toEqual([
      ["Fetch", false],
      ["Pull main", false],
      ["Force push main to origin/main…", true],
    ]);
    await buttons[0].run();
    expect(view.fetch).toHaveBeenCalled();
    await buttons[1].run();
    expect(ipc.api.runOp).toHaveBeenLastCalledWith(
      1,
      { op: "pull", branch: "main", mode: "default", expectHead: (view.snapshot as RepoSnapshot).head, expectWorktree: "/r" },
      [],
    );
    await buttons[2].run();
    expect(ipc.api.runOp).toHaveBeenLastCalledWith(
      1,
      { op: "push", branch: "main", remote: "origin", to: null, force: true },
      [],
    );
  });

  it("a rejected push to the upstream of another name offers the force push there, named (R38)", async () => {
    const [force] = await buttonsFor([{ kind: "force_push", branch: "feature", remote: "origin", to: "main" }], "failed");
    expect(force.label).toBe("Force push feature to origin/main…");
    await force.run();
    expect(ipc.api.runOp).toHaveBeenLastCalledWith(
      1,
      { op: "push", branch: "feature", remote: "origin", to: "main", force: true },
      [],
    );
  });

  it("skip: a continue that came out empty offers to skip that commit, by its hash (R40)", async () => {
    const commit = "e".repeat(40);
    const [skip] = await buttonsFor([{ kind: "skip", commit }], "failed");
    expect(skip.label).toBe("Skip this commit");
    await skip.run();
    expect(ipc.api.runOp).toHaveBeenLastCalledWith(
      1,
      { op: "skip", expectHead: (view.snapshot as RepoSnapshot).head, commit, expectWorktree: "/r" },
      [],
    );
  });

  it("the Skip notice holds only while git is stopped on that commit (R40)", async () => {
    const commit = "e".repeat(40);
    const stoppedOn = (c: string | null) =>
      snap({
        inProgress: {
          kind: "cherry-pick",
          files: [],
          fileCount: 0,
          step: null,
          total: null,
          subject: null,
          incoming: null,
          rebasing: null,
          commit: c,
          empty: true,
        },
      });
    view.snapshot = stoppedOn(commit);
    await buttonsFor([{ kind: "skip", commit }], "failed");
    const notice = app.app.toasts[0];
    expect(notice.holds?.()).toBe(true);
    view.snapshot = stoppedOn("f".repeat(40));
    expect(notice.holds?.()).toBe(false);
    view.snapshot = snap();
    expect(notice.holds?.()).toBe(false);
    // Other notices with buttons have no such condition.
    app.app.toasts.length = 0;
    await buttonsFor([{ kind: "fetch" }], "failed");
    expect(app.app.toasts[0].holds).toBeUndefined();
  });

  it("choose_pull_mode: no notice; the dialog that asks merge or rebase opens", async () => {
    view.loadedLabel = vi.fn(() => ({ upstream: "origin/main" }));
    const message = "This branch and its upstream have diverged: choose whether to pull by merging or by rebasing.";
    vi.mocked(ipc.api.runOp).mockResolvedValueOnce(
      outcome({ status: "failed", message, hints: [{ kind: "choose_pull_mode", branch: "main" }] }),
    );
    await ops.runOp(1, { op: "pull", branch: "main", mode: "default", expectHead: HEAD, expectWorktree: "/r/wt" });
    expect(app.app.toasts).toEqual([]);
    expect(overlays.showDialog).toHaveBeenCalledWith(
      { name: "PullDialog" },
      { repoId: 1, branch: "main", upstream: "origin/main", message, head: (view.snapshot as RepoSnapshot).head, worktree: "/r/wt" },
    );
  });

  it("a follow-up that fails is reported, not thrown at the button", async () => {
    const [push] = await buttonsFor([{ kind: "push", branch: "main", remote: "origin" }]);
    vi.mocked(ipc.api.runOp).mockRejectedValueOnce(new ipc.CoreError({ code: "busy", message: "Busy", detail: null }));
    await expect(push.run()).resolves.toBeUndefined();
    expect(app.app.toasts.at(-1)).toMatchObject({ kind: "error", message: "Busy" });
  });
});

describe("ops.busy", () => {
  it("is the operation running on the repository, from its snapshot", () => {
    expect(ops.ops.busy(1)).toBeNull();
    const op = { id: 4, title: "Push main to origin", startedAtMs: 1, queued: 2 };
    view.snapshot = snap({ op });
    expect(ops.ops.busy(1)).toEqual(op);
    expect(ops.ops.busy(2)).toBeNull();
  });
});
