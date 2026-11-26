// What the status bar at the bottom of the window says about when the repository was last
// fetched, and when that wording next changes (so one timer, set for then, keeps it current).

import { relativeTime } from "./format";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * How long ago the repository was last fetched (`fetchedAtMs`, epoch milliseconds), as of
 * `now`: "Fetched just now", "Fetched 2 min ago", "Fetched yesterday". Nothing (null) while
 * that is unknown: "Never fetched" would be wrong for a fresh clone (git writes no FETCH_HEAD
 * when it clones) and for a repository whose last fetch failed (git empties it first).
 */
export function fetchedText(fetchedAtMs: number | null, now: number = Date.now()): string | null {
  if (fetchedAtMs === null) return null;
  return `Fetched ${relativeTime(fetchedAtMs, now)}`;
}

/**
 * Milliseconds from `now` until `relativeTime(ms, now)` may read differently: the next whole
 * minute of age within the hour, the next hour within the day, then the next whole day (from
 * two weeks on, the wording changes on some of them only). A time a
 * little in the future (another machine's clock) reads "just now" until it is a minute old.
 */
export function nextChangeIn(ms: number, now: number = Date.now()): number {
  const age = now - ms;
  if (age < MINUTE) return MINUTE - age;
  if (age < HOUR) return MINUTE - (age % MINUTE);
  if (age < DAY) return HOUR - (age % HOUR);
  if (age < 2 * DAY) return 2 * DAY - age;
  return DAY - (age % DAY);
}
