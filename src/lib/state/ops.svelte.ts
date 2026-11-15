// Operations from the UI: `runOp` sends one request to the core and sees it through: it asks
// the user to confirm the dangers the core finds, applies the repository's new state, and
// shows how it went. The running operation itself is part of the repository's snapshot.

import ConfirmDialog from "../components/dialogs/ConfirmDialog.svelte";
import PullDialog from "../components/dialogs/PullDialog.svelte";
import { api, CoreError } from "../ipc";
import { shown } from "../menus/common";
import type { Confirmed, CoreErrorDto, OpHint, OpOutcome, OpRequest, RepoSnapshot } from "../types";
import { app, dismissToast, notify, reportError, showLog, type ToastAction } from "./app.svelte";
import { markStopped } from "./commandLog.svelte";
import { closeDialog, openDialog, showDialog } from "./overlays.svelte";
import { findRepoView } from "./repo.svelte";

/**
 * Runs `request` on repository `repoId` (queued behind the operations before it, by the
 * core). When the core finds dangers in it, nothing has run yet: the user is shown them and,
 * on confirming, the request is sent again with them confirmed, each with the token of the
 * state it was shown (R39), as often as the core asks (it asks again, with the state as it
 * is then, if anything it was about changed meanwhile); cancelling ends it there. The
 * outcome's snapshot is applied to the repository's view. The notice: `done`, its message
 * with a button per hint; `failed`, its message with git's text folded below, the hints, and
 * a link to its command-log entries; `stopped` (git stopped on conflicts), none: the
 * conflict banner shows it. A pull that needs to be told whether to merge or rebase (hint
 * `choose_pull_mode`) shows that choice instead of a notice, and is sent again as chosen. An error from the core is reported, and the promise rejects
 * with it; errors with a code in `ownErrors` are not reported: the caller shows them (a
 * dialog shows a refused name beside its field).
 */
export async function runOp(
  repoId: number,
  request: OpRequest,
  { ownErrors = [] }: { ownErrors?: CoreErrorDto["code"][] } = {},
): Promise<OpOutcome> {
  let confirmed: Confirmed[] = [];
  for (;;) {
    let outcome: OpOutcome;
    try {
      outcome = await api.runOp(repoId, request, confirmed);
    } catch (e) {
      if (!(e instanceof CoreError && ownErrors.includes(e.code))) reportError(e);
      throw e;
    }
    findRepoView(repoId)?.applySnapshot(outcome.snapshot);
    if (outcome.status !== "needs_confirmation") {
      report(repoId, outcome);
      return outcome;
    }
    const again = confirmed.length > 0;
    if (!(await confirm(repoId, outcome, again))) return outcome;
    confirmed = outcome.dangers.map((d) => ({ kind: d.kind, token: d.token }));
  }
}

/** What runs in each repository, for the busy indicator. */
export const ops = {
  /** The operation running in repository `repoId` (with the count queued behind it), or null. */
  busy(repoId: number): RepoSnapshot["op"] {
    return findRepoView(repoId)?.snapshot.op ?? null;
  },
};

/**
 * Shows the dangers of `outcome` (on top of any dialog open) and resolves to whether the
 * user went ahead. Only the first answer counts, and it closes this confirmation only. For
 * a repository other than the active tab's, the confirmation names it. `again`: the user
 * confirmed before, and what it is about has changed since: the confirmation says so.
 */
function confirm(repoId: number, outcome: OpOutcome, again: boolean): Promise<boolean> {
  return new Promise((resolve) => {
    let ended = false;
    const end = (yes: boolean) => {
      if (ended) return;
      ended = true;
      closeDialog(id);
      resolve(yes);
    };
    const repository = repoId === app.activeRepoId ? null : (findRepoView(repoId)?.snapshot.name ?? null);
    const id = openDialog(ConfirmDialog, {
      title: outcome.title,
      dangers: outcome.dangers,
      repository,
      again,
      onConfirm: () => end(true),
      onCancel: () => end(false),
    });
  });
}

function report(repoId: number, outcome: OpOutcome): void {
  if (outcome.status === "stopped") markStopped(outcome.opId);
  // What earlier notices offered may no longer hold once a later operation ran there (R30):
  // its snapshot may never have shown it running.
  for (const t of app.toasts) {
    if (t.until && t.until.repoId === repoId && outcome.opId > t.until.opId) dismissToast(t.id);
  }
  const choose = outcome.hints.find((h) => h.kind === "choose_pull_mode");
  if (outcome.status === "failed" && choose) {
    const upstream = findRepoView(repoId)?.loadedLabel(choose.branch)?.upstream ?? null;
    showDialog(PullDialog, { repoId, branch: choose.branch, upstream, message: outcome.message, head: headOf(repoId) });
    return;
  }
  const actions = outcome.hints.flatMap((hint) => hintActions(repoId, hint));
  const detail = outcome.detail ?? undefined;
  // A notice that offers what to do next waits for the user, until the next operation there.
  const until = actions.length > 0 ? { repoId, opId: outcome.opId } : undefined;
  // A Skip applies while git is stopped on the commit it is for (R40).
  const skip = outcome.hints.find((h) => h.kind === "skip");
  const holds = skip ? () => findRepoView(repoId)?.snapshot.inProgress?.commit === skip.commit : undefined;
  if (outcome.status === "done") {
    notify({ kind: "info", message: outcome.message, detail, foldDetail: true, actions, persistent: !!until, until, holds });
  } else if (outcome.status === "failed") {
    notify({
      kind: "error",
      message: outcome.message,
      detail,
      foldDetail: true,
      persistent: true,
      until,
      holds,
      actions,
      // The notice stays: the log is read against it.
      link:
        outcome.logFrom > 0
          ? { label: "Show log", keepOpen: true, run: () => showLog(repoId, outcome.logFrom) }
          : undefined,
    });
  }
}

/** Runs a follow-up from a notice's button; its failure is reported by runOp. */
async function followUp(repoId: number, request: OpRequest): Promise<void> {
  try {
    await runOp(repoId, request);
  } catch {
    // Reported already.
  }
}

/** The buttons a hint puts on a notice. */
function hintActions(repoId: number, hint: OpHint): ToastAction[] {
  switch (hint.kind) {
    case "push": {
      const { branch, remote } = hint;
      const push: OpRequest = { op: "push", branch, remote, to: null, force: false };
      return [{ label: `Push ${shown(branch)} to ${shown(`${remote}/${branch}`)}`, run: () => followUp(repoId, push) }];
    }
    case "fetch":
      return [{ label: "Fetch", run: () => findRepoView(repoId)?.fetch() }];
    case "pull": {
      const { branch } = hint;
      // HEAD as shown when the button is pressed: the pull names its branch.
      const pull = (): OpRequest => ({ op: "pull", branch, mode: "default", expectHead: headOf(repoId) });
      return [{ label: `Pull ${shown(branch)}`, run: () => followUp(repoId, pull()) }];
    }
    case "force_push": {
      const { branch, remote, to } = hint;
      const push: OpRequest = { op: "push", branch, remote, to, force: true };
      const there = shown(`${remote}/${to ?? branch}`);
      return [{ label: `Force push ${shown(branch)} to ${there}…`, danger: true, run: () => followUp(repoId, push) }];
    }
    case "choose_pull_mode":
      return []; // a dialog asks instead (see report)
    case "skip":
      return [
        {
          label: "Skip this commit",
          run: () => followUp(repoId, { op: "skip", expectHead: headOf(repoId), commit: hint.commit }),
        },
      ];
  }
}

/** HEAD of repository `repoId` as its tab shows it now. */
function headOf(repoId: number): RepoSnapshot["head"] {
  return findRepoView(repoId)?.snapshot.head ?? { branch: null, hash: null };
}
