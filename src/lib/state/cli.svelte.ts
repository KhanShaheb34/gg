// The `gg` terminal command: reading its state, installing and removing it, and the dialog
// that does both (the app menu's "Install ‘gg’ Command…", the welcome screen's hint).

import InstallCliDialog from "../components/dialogs/InstallCliDialog.svelte";
import { installedAt, lineAddedNote, lineRemovedNote, linkNote } from "../cli";
import { api } from "../ipc";
import type { CliState, ShellLineAdded, ShellLineRemoved } from "../types";
import { shortenPath } from "../format";
import { app, notify, reportError, toast } from "./app.svelte";
import { overlays, showDialog } from "./overlays.svelte";

/** Reads the command's state again (it may have changed outside the app); keeps the old one if that fails. */
export async function refreshCli(): Promise<void> {
  try {
    app.cli = (await api.getAppState()).cli;
  } catch {
    // Shown as last read; the dialog's own requests report failures.
  }
}

const short = (path: string) => shortenPath(path, app.homeDir);

/**
 * Writes the launcher into `dir`, and with `turnOff` (the box the user ticked) gg's line to
 * turn off the shell's own `gg`; the new state and what adding the line did (null when not
 * asked, or there was nothing to turn off), or the core's error (thrown: nothing installed).
 */
export async function installCli(
  dir: string,
  turnOff = false,
): Promise<{ state: CliState; shellLineAdded: ShellLineAdded | null }> {
  const { shellLineAdded, ...state } = await api.installCli(dir, turnOff);
  app.cli = state;
  return { state, shellLineAdded };
}

/**
 * Removes the launchers the app wrote, and gg's line in a shell file; the files that went (none
 * when there was nothing to remove) and what became of the line, or the core's error (thrown).
 */
export async function uninstallCli(): Promise<{ removed: string[]; line: ShellLineRemoved }> {
  const { removed, lineRemovedFrom, lineProblem, ...state } = await api.uninstallCli();
  app.cli = state;
  return { removed, line: { lineRemovedFrom, lineProblem } };
}

/** What removing the command did: a headline, and the files that actually went. */
export function removedMessage(removed: string[]): { message: string; note: string } {
  if (removed.length === 0) return { message: "Nothing to remove", note: "No gg command written by gg was found on your PATH." };
  return { message: "Removed the gg command", note: `${removed.map(short).join(", ")}.` };
}

/** Says what removing the command did: what went, and what became of gg's line. */
export function reportRemoved(removed: string[], line: ShellLineRemoved): void {
  const lineNote = lineRemovedNote(line, short);
  const { message, note } = removedMessage(removed);
  notify({
    kind: line.lineProblem === null ? "info" : "error",
    message,
    note: lineNote === null ? note : `${note} ${lineNote}`,
  });
}

/**
 * Removes the command (the welcome screen's "Remove"; on Linux and Windows the only way
 * besides the dialog): says what went, or why it could not.
 */
export async function removeCli(): Promise<void> {
  let done;
  try {
    done = await uninstallCli();
  } catch (e) {
    reportError(e, "Could not remove the gg command");
    return;
  }
  reportRemoved(done.removed, done.line);
}

/**
 * Settings' "Turn off that alias for me": adds gg's line to the shell's start-up file, then
 * says what the shell, asked again, makes of it (or why the line could not be added).
 */
export async function turnOffShellGg(): Promise<void> {
  let added;
  try {
    const { shellLineAdded, ...state } = await api.turnOffShellGg();
    app.cli = state;
    added = shellLineAdded;
  } catch (e) {
    reportError(e, "Could not turn off your shell’s gg");
    return;
  }
  const worked = added.problem === null && added.checked && app.cli.shadowedBy === null;
  notify({ kind: added.problem === null ? "info" : "error", message: lineAddedNote(added, app.cli, short), persistent: !worked });
}

/** Settings' "Take the line out": takes gg's line out of the shell's start-up files and says so. */
export async function removeShellLine(): Promise<void> {
  let line: ShellLineRemoved;
  try {
    const { lineRemovedFrom, lineProblem, ...state } = await api.removeShellLine();
    app.cli = state;
    line = { lineRemovedFrom, lineProblem };
  } catch (e) {
    reportError(e, "Could not take gg’s line out");
    return;
  }
  const note = lineRemovedNote(line, short) ?? "No line of gg’s was found in your shell’s start-up files.";
  if (line.lineProblem === null) toast("info", note);
  else notify({ kind: "error", message: note, persistent: true });
}

/**
 * Shows the install dialog with the command's state read afresh; nothing if one is open. When
 * the command is a link to gg's launcher made outside the app (Homebrew's), there is nothing
 * for the dialog to do: a notice says where it is and who looks after it.
 */
export async function openCliDialog(): Promise<void> {
  if (overlays.dialogs.some((d) => d.component === InstallCliDialog)) return;
  await refreshCli();
  if (overlays.dialogs.some((d) => d.component === InstallCliDialog)) return;
  const at = installedAt(app.cli, (p) => shortenPath(p, app.homeDir));
  const note = linkNote(app.cli.link);
  if (at !== null && note !== null) {
    notify({ kind: "info", message: `The gg command is already ${at}`, note });
    return;
  }
  showDialog(InstallCliDialog, {});
}
