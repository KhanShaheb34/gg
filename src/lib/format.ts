/** Abbreviated commit hash for display: the first 8 characters. */
export function shortHash(hash: string): string {
  return hash.slice(0, 8);
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** How long ago `ms` (epoch milliseconds) was, as of `now`: "3 days ago". */
export function relativeTime(ms: number, now: number = Date.now()): string {
  const age = now - ms;
  const count = (unit: number) => Math.floor(age / unit);
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"} ago`;
  if (age < MINUTE) return "just now";
  if (age < HOUR) return `${count(MINUTE)} min ago`;
  if (age < DAY) return `${count(HOUR)} h ago`;
  if (age < 2 * DAY) return "yesterday";
  if (age < 14 * DAY) return plural(count(DAY), "day");
  if (age < 60 * DAY) return plural(count(7 * DAY), "week");
  if (age < 365 * DAY) return plural(count(30 * DAY), "month");
  return plural(count(365 * DAY), "year");
}

/**
 * The folder that contains `path`, for display: the user's home folder (guessed from the
 * usual `/Users/<name>`, `/home/<name>` and `C:\Users\<name>` layouts) shown as `~`.
 */
export function displayFolder(path: string): string {
  const sep = path.includes("\\") && !path.includes("/") ? "\\" : "/";
  const cut = path.lastIndexOf(sep);
  let parent = cut <= 0 ? sep : path.slice(0, cut);
  if (/^[A-Za-z]:$/.test(parent)) parent += sep;
  const home = sep === "/" ? /^\/(?:Users|home)\/[^/]+/ : /^[A-Za-z]:\\Users\\[^\\]+/;
  return parent.replace(home, "~");
}

/** Number of lane colours (`--lane-0` … `--lane-9`). */
export const LANE_COLORS = 10;

/** A stable lane colour index for `key` (a repository path, say): FNV-1a, folded to 0..9. */
export function colorIndex(key: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < key.length; i++) {
    hash ^= key.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % LANE_COLORS;
}
