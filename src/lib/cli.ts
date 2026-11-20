// The `gg` terminal command, as the UI words it: where the launcher goes and what the user
// is told about it. The core writes and removes it (install_cli, uninstall_cli).

import type { CliLink, CliState } from "./types";

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

/**
 * What is said of a command gg did not make, a link to its launcher (Homebrew's, into the app
 * bundle, or someone else's): who looks after it. Null when gg wrote it, or there is none.
 */
export function linkNote(link: CliLink | null): string | null {
  switch (link) {
    case "homebrew":
      return "Homebrew made it, a link to the launcher inside gg.app, and removes it when you uninstall gg with Homebrew; gg leaves it alone.";
    case "other":
      return "gg did not make this link and leaves it alone.";
    default:
      return null;
  }
}

/**
 * Where the command is, as "installed at ~/bin/gg", "installed by Homebrew at
 * /opt/homebrew/bin/gg" or "installed at /usr/local/bin/gg, a link to gg’s launcher" (paths
 * through `short`); null when it is not installed.
 */
export function installedAt(state: CliState, short: (path: string) => string): string | null {
  if (state.installed === null) return null;
  const at = short(state.installed);
  switch (state.link) {
    case "homebrew":
      return `installed by Homebrew at ${at}`;
    case "other":
      return `installed at ${at}, a link to gg’s launcher`;
    default:
      return `installed at ${at}`;
  }
}
