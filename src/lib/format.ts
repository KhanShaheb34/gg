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

/** Separator used by `path`: backslash for a Windows path without forward slashes. */
function separatorOf(path: string): "/" | "\\" {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

/**
 * `path` with the user's home folder `home` shown as `~` ("~/Dev/x"). Only a whole leading
 * folder is replaced; Windows paths compare without case. Without `home`, `path` as is.
 */
export function shortenPath(path: string, home?: string | null): string {
  if (!home) return path;
  const sep = separatorOf(home);
  let root = home;
  while (root.length > 1 && root.endsWith(sep)) root = root.slice(0, -1);
  const fold = (s: string) => (sep === "\\" ? s.toLowerCase() : s);
  const p = fold(path);
  const h = fold(root);
  if (p === h) return "~";
  if (p.startsWith(h + sep)) return "~" + path.slice(root.length);
  return path;
}

/** The folder that contains `path`, for display, with the home folder `home` shown as `~`. */
export function displayFolder(path: string, home?: string | null): string {
  const sep = separatorOf(path);
  const cut = path.lastIndexOf(sep);
  let parent = cut <= 0 ? sep : path.slice(0, cut);
  if (/^[A-Za-z]:$/.test(parent)) parent += sep;
  return shortenPath(parent, home);
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const pad2 = (n: number) => (n < 10 ? `0${n}` : String(n));

/** A commit time (unix seconds) in local time: "12 Nov 2026 16:02". */
export function formatDate(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  return `${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/** A commit time in full, for tooltips: "Thursday, 12 Nov 2026 16:02:00 (UTC+01:00)". */
export function formatDateLong(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const offset = -d.getTimezoneOffset();
  const sign = offset < 0 ? "-" : "+";
  const zone = `UTC${sign}${pad2(Math.floor(Math.abs(offset) / 60))}:${pad2(Math.abs(offset) % 60)}`;
  return `${WEEKDAYS[d.getDay()]}, ${formatDate(unixSecs)}:${pad2(d.getSeconds())} (${zone})`;
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
