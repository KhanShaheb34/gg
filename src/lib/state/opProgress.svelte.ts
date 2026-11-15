// What the running operation of each repository last said (`op-progress`): git's latest
// progress line ("Writing objects: 45% (9/20)", masked by the core), or the step of a pull
// request being made ("Pushing…", "Creating…"). The busy indicator shows it once the
// operation has run for PROGRESS_AFTER_MS, and forgets it when the operation ends.

import { visibleText } from "../format";
import { onEvent } from "../ipc";
import type { OpProgress } from "../types";

/** An operation's progress line shows once it has run this long, ms. */
export const PROGRESS_AFTER_MS = 1000;

/** The latest line of each repository's running operation, by repository id. */
const latest = $state<Record<number, { opId: number; line: string }>>({});

let watching = false;

/** Starts taking `op-progress` events (once). */
export function watchOpProgress(): void {
  if (watching) return;
  watching = true;
  onEvent<OpProgress>("op-progress", ({ repoId, opId, line }) => {
    latest[repoId] = { opId, line: tidy(line) };
  });
}

/** `line` as one line to show: spaces run together, ends trimmed, hidden characters shown. */
function tidy(line: string): string {
  return visibleText(line.replace(/\s+/g, " ").trim());
}

/** What operation `opId` of repository `repoId` last said, or null. */
export function progressLine(repoId: number, opId: number): string | null {
  const last = latest[repoId];
  return last && last.opId === opId && last.line !== "" ? last.line : null;
}

/** Forgets repository `repoId`'s line: its operation ended. */
export function clearProgress(repoId: number): void {
  delete latest[repoId];
}

/** Whether an operation that started at `startedAtMs` has run long enough, at `now`, for its line to show. */
export function progressDue(startedAtMs: number, now: number): boolean {
  return now - startedAtMs >= PROGRESS_AFTER_MS;
}
