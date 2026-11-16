// The `gg` terminal command, as the UI words it: where the launcher goes and what the user
// is told about it. The core writes and removes it (install_cli, uninstall_cli).

import type { CliState } from "./types";

/** The launcher's path in folder `dir` (`dir/gg`, with the folder's own separator). */
export function launcherPath(dir: string): string {
  const windows = dir.includes("\\") && !dir.includes("/");
  const sep = windows ? "\\" : "/";
  // On Windows the launcher is a batch file, gg.cmd.
  const name = windows ? "gg.cmd" : "gg";
  return dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`;
}

/** The folder of launcher path `file`. */
export function folderOf(file: string): string {
  const at = Math.max(file.lastIndexOf("/"), file.lastIndexOf("\\"));
  return at <= 0 ? file.slice(0, at + 1) : file.slice(0, at);
}

/**
 * The folder offered first: where the launcher is now, if that is still a candidate, else the
 * best candidate; null when there is none.
 */
export function preferredFolder(state: CliState): string | null {
  const here = state.installed === null ? null : folderOf(state.installed);
  if (here !== null && state.candidates.includes(here)) return here;
  return state.candidates[0] ?? null;
}
