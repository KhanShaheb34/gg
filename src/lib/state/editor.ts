// Opening conflicted files (or the worktree folder) in the user's editor, through the core.
// The app never edits them itself.

import { api } from "../ipc";
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
  if (note !== null && !shownNotes.has(note)) {
    shownNotes.add(note);
    notify({ kind: "info", message: note });
  }
}
