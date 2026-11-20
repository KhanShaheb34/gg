// The `gg` terminal command: reading its state, installing and removing it, and the dialog
// that does both (the app menu's "Install ‘gg’ Command…", the welcome screen's hint).

import InstallCliDialog from "../components/dialogs/InstallCliDialog.svelte";
import { installedAt, linkNote } from "../cli";
import { api } from "../ipc";
import type { CliState } from "../types";
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

/** Writes the launcher into `dir`; the new state, or the core's error (thrown). */
export async function installCli(dir: string): Promise<CliState> {
  const state = await api.installCli(dir);
  app.cli = state;
  return state;
}

/**
 * Removes the launchers the app wrote; the files that went (none when there was nothing to
 * remove), or the core's error (thrown).
 */
export async function uninstallCli(): Promise<string[]> {
  const { removed, ...state } = await api.uninstallCli();
  app.cli = state;
  return removed;
}

/** What removing the command did, in one line: only the files that actually went. */
export function removedMessage(removed: string[]): string {
  if (removed.length === 0) return "Nothing to remove: no gg command written by gg was found on your PATH.";
  return `Removed the gg command: ${removed.map((p) => shortenPath(p, app.homeDir)).join(", ")}.`;
}

/**
 * Removes the command (the welcome screen's "Remove"; on Linux and Windows the only way
 * besides the dialog): says what went, or why it could not.
 */
export async function removeCli(): Promise<void> {
  let removed;
  try {
    removed = await uninstallCli();
  } catch (e) {
    reportError(e, "Could not remove the gg command");
    return;
  }
  toast("info", removedMessage(removed));
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
    notify({ kind: "info", message: `The gg command is already ${at}.`, detail: note });
    return;
  }
  showDialog(InstallCliDialog, {});
}
