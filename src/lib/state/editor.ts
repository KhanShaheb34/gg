// Opening things outside the app, through the core: conflicted files (or the worktree folder)
// in the user's editor, which the app never edits itself; any worktree in the editor, a
// terminal or the file manager; a web page (https only) in the browser.

import { api } from "../ipc";
import type { ExternalRequest } from "../types";
import { notify, reportError } from "./app.svelte";

/** The notes the core sent back (something else than asked was opened) shown so far. */
const shownNotes = new Set<string>();

/**
 * Opens `path` (relative to the worktree), or the worktree folder when null, in the editor.
 * A note the core sends back (no editor command is set, so the folder opened instead of the
 * file) is shown as a notice once per run of the app; an error is reported.
 */
export async function openInEditor(repoId: number, path: string | null): Promise<void> {
  let note: string | null;
  try {
    note = await api.openInEditor(repoId, path);
  } catch (e) {
    reportError(e);
    return;
  }
  showNote(note);
}

/** Shows `note` (something else than asked was opened) once per run of the app. */
function showNote(note: string | null): void {
  if (note !== null && !shownNotes.has(note)) {
    shownNotes.add(note);
    notify({ kind: "info", message: note });
  }
}

/**
 * Opens what `request` names outside the app: worktree `path` of repository `repoId` (any of
 * its worktrees) in the editor, a terminal or the file manager, or an https page in the
 * browser. The core's note is shown once; an error is reported.
 */
export async function openExternal(repoId: number, request: ExternalRequest): Promise<void> {
  try {
    showNote((await api.openExternal(repoId, request)).note);
  } catch (e) {
    reportError(e);
  }
}
