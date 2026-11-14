// Operations from the UI: `runOp` sends one request to the core and sees it through: it asks
// the user to confirm the dangers the core finds, applies the repository's new state, and
// shows how it went. The running operation itself is part of the repository's snapshot.

import ConfirmDialog from "../components/dialogs/ConfirmDialog.svelte";
import PullDialog from "../components/dialogs/PullDialog.svelte";
import { api, CoreError } from "../ipc";
import { shown } from "../menus/common";
import type { CoreErrorDto, Danger, DangerKind, OpHint, OpOutcome, OpRequest, RepoSnapshot } from "../types";
import { app, notify, reportError, showLog, type ToastAction } from "./app.svelte";
import { closeDialog, openDialog, showDialog } from "./overlays.svelte";
import { findRepoView } from "./repo.svelte";

/**
 * Runs `request` on repository `repoId` (queued behind the operations before it, by the
 * core). When the core finds dangers in it, nothing has run yet: the user is shown them and,
 * on confirming, the request is sent again with them confirmed, as often as the core asks
 * (a force push asks again if the remote moved meanwhile); cancelling ends it there. The
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
  let confirmed: DangerKind[] = [];
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
    if (!(await confirm(repoId, outcome))) return outcome;
    confirmed = [...new Set([...confirmed, ...outcome.dangers.map((d) => d.kind)])];
    request = withTokens(request, outcome.dangers);
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
 * a repository other than the active tab's, the confirmation names it.
 */
function confirm(repoId: number, outcome: OpOutcome): Promise<boolean> {
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
      onConfirm: () => end(true),
      onCancel: () => end(false),
    });
  });
}

/**
 * `request` carrying what the user was shown, where it has a field for it: a force push's
 * lease is the remote commit its danger was worked out from (R11). Other dangers need only
 * their kind confirmed.
 */
function withTokens(request: OpRequest, dangers: Danger[]): OpRequest {
  const force = dangers.find((d) => d.kind === "force_push");
  if (request.op === "push" && force) return { ...request, lease: force.token };
  return request;
}

function report(repoId: number, outcome: OpOutcome): void {
  const choose = outcome.hints.find((h) => h.kind === "choose_pull_mode");
  if (outcome.status === "failed" && choose) {
    const upstream = findRepoView(repoId)?.loadedLabel(choose.branch)?.upstream ?? null;
    showDialog(PullDialog, { repoId, branch: choose.branch, upstream, message: outcome.message });
    return;
  }
  const actions = outcome.hints.flatMap((hint) => hintActions(repoId, hint));
  const detail = outcome.detail ?? undefined;
  // A notice that offers what to do next waits for the user, until the next operation there.
  const until = actions.length > 0 ? { repoId, opId: outcome.opId } : undefined;
  if (outcome.status === "done") {
    notify({ kind: "info", message: outcome.message, detail, foldDetail: true, actions, persistent: !!until, until });
  } else if (outcome.status === "failed") {
    notify({
      kind: "error",
      message: outcome.message,
      detail,
      foldDetail: true,
      persistent: true,
      until,
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
      const { branch, remote, setUpstream } = hint;
      const push: OpRequest = { op: "push", branch, remote, setUpstream, force: false, lease: null };
      return [{ label: `Push ${shown(branch)}`, run: () => followUp(repoId, push) }];
    }
    case "fetch":
      return [{ label: "Fetch", run: () => findRepoView(repoId)?.fetch() }];
    case "pull":
      return [
        { label: `Pull ${shown(hint.branch)}`, run: () => followUp(repoId, { op: "pull", branch: hint.branch, mode: "default" }) },
      ];
    case "force_push": {
      const { branch, remote } = hint;
      const push: OpRequest = { op: "push", branch, remote, setUpstream: false, force: true, lease: null };
      return [{ label: `Force push ${shown(branch)}…`, danger: true, run: () => followUp(repoId, push) }];
    }
    case "choose_pull_mode":
      return []; // a dialog asks instead (see report)
  }
}
