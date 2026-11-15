// How a dialog hands its request to runOp. Most close first: what follows (a confirmation,
// the notice) is runOp's. A dialog that takes a name stays open until the core has
// answered, so that a name the core refuses is shown beside the field, the text kept.

import { CoreError } from "../../ipc";
import { reportError } from "../../state/app.svelte";
import { runOp } from "../../state/ops.svelte";
import type { OpRequest } from "../../types";

/** Closes the dialog (`close`) and runs `request`; runOp shows how it went. */
export function closeAndRun(close: () => void, repoId: number, request: OpRequest): void {
  close();
  runOp(repoId, request).catch(() => {
    // Reported by runOp.
  });
}

/**
 * Runs `request` with the dialog still open. Resolves to the core's message if it refused
 * the name (the dialog stays open, to show it), else closes the dialog and resolves to null.
 * `isOpen`: the dialog is still open (the user may have cancelled meanwhile; a refusal is
 * then reported as a notice).
 */
export async function runNamed(
  close: () => void,
  isOpen: () => boolean,
  repoId: number,
  request: OpRequest,
): Promise<string | null> {
  try {
    await runOp(repoId, request, { ownErrors: ["bad_name"] });
  } catch (e) {
    if (e instanceof CoreError && e.code === "bad_name") {
      if (isOpen()) return e.message;
      reportError(e);
    }
    // Anything else was reported by runOp.
  }
  close();
  return null;
}
