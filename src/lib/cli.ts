// The `gg` terminal command, as the UI words it: where the launcher goes and what the user
// is told about it. The core writes and removes it (install_cli, uninstall_cli).

import type { CliLink, CliState, ShellConflict, ShellLineAdded, ShellLineRemoved } from "./types";

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

/** "alias" or "function": the shell's own `gg`, in a word. */
export function conflictWord(conflict: ShellConflict): string {
  return conflict === "alias" ? "alias" : "function";
}

/** The command gg's line runs to turn the shell's `gg` off (the core writes it, with a note). */
export function turnOffCommand(conflict: ShellConflict): string {
  return conflict === "alias" ? "unalias gg 2>/dev/null || :" : "unset -f gg 2>/dev/null || :";
}

/**
 * What adding gg's line did, in a sentence or two, as the shell said after (`shadowedBy`, what
 * it still runs for `gg`, and `shadowConflict`, whether that is an alias or a function): turned
 * off; turned off, but a program called gg still comes first; added but something after it
 * makes `gg` again; added but the shell could not be asked; or not added, and why. Paths
 * through `short`.
 */
export function lineAddedNote(
  added: ShellLineAdded,
  after: Pick<CliState, "shadowedBy" | "shadowConflict">,
  short: (path: string) => string,
): string {
  const { shadowedBy } = after;
  const file = short(added.file);
  const word = conflictWord(added.conflict);
  if (added.problem !== null) return `Did not turn off your shell’s gg ${word}: ${added.problem}`;
  const where = added.added ? `Added one line to the end of ${file}` : `gg’s line is already in ${file}`;
  if (!added.checked) {
    return `${where} to turn off your shell’s gg ${word}, but gg could not ask your shell whether that worked. It takes effect in new terminal windows; run type gg in one to see.`;
  }
  if (shadowedBy !== null && after.shadowConflict === null) {
    // The alias is gone; what is left is no alias or function, which a line cannot turn off.
    const done = added.added
      ? `Turned off your shell’s gg ${word}: added one line to the end of ${file}.`
      : `Your shell’s gg ${word} is turned off by gg’s line in ${file}.`;
    return `${done} But your shell still runs ${shadowedBy} for gg, which wins over this command until you remove it.`;
  }
  if (shadowedBy !== null) {
    return `${where}, but your shell still defines gg as ${shadowedBy}: something that runs after that line defines it again.`;
  }
  return added.added
    ? `Turned off your shell’s gg ${word}: added one line to the end of ${file}. It takes effect in new terminal windows.`
    : `Your shell’s gg ${word} is turned off by gg’s line in ${file}. It takes effect in new terminal windows.`;
}

/**
 * Whether gg's line is in a start-up file and the shell still has an alias or function called
 * `gg` that no other line would turn off: something that runs after the line makes it again
 * (another `gg`, a program, is not the line's to turn off).
 */
export function lineFailed(state: CliState): boolean {
  return state.shellLine !== null && state.shadowConflict !== null && state.turnOff === null;
}

/** What taking gg's line out did, in a sentence or two; null when there was no line and nothing to say. */
export function lineRemovedNote(removed: ShellLineRemoved, short: (path: string) => string): string | null {
  const parts: string[] = [];
  if (removed.lineRemovedFrom.length > 0) {
    parts.push(`Took gg’s line out of ${removed.lineRemovedFrom.map(short).join(", ")}; new terminal windows no longer run it.`);
  }
  if (removed.lineProblem !== null) parts.push(removed.lineProblem);
  return parts.length === 0 ? null : parts.join(" ");
}
