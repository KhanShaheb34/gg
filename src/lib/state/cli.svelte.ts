// The `gg` terminal command: reading its state, installing and removing it, and the dialog
// that does both (the app menu's "Install ‘gg’ Command…", the welcome screen's hint).

import InstallCliDialog from "../components/dialogs/InstallCliDialog.svelte";
import { api } from "../ipc";
import type { CliState } from "../types";
import { app } from "./app.svelte";
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

/** Removes the launchers the app wrote; the new state, or the core's error (thrown). */
export async function uninstallCli(): Promise<CliState> {
  const state = await api.uninstallCli();
  app.cli = state;
  return state;
}

/** Shows the install dialog with the command's state read afresh; nothing if one is open. */
export async function openCliDialog(): Promise<void> {
  if (overlays.dialogs.some((d) => d.component === InstallCliDialog)) return;
  await refreshCli();
  if (overlays.dialogs.some((d) => d.component === InstallCliDialog)) return;
  showDialog(InstallCliDialog, {});
}
