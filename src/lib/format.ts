import { LANE_COLORS } from "./graph/geometry";

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


/** A stable lane colour index for `key` (a repository path, say): FNV-1a, folded to 0..9. */
export function colorIndex(key: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < key.length; i++) {
    hash ^= key.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % LANE_COLORS;
}

/**
 * Characters that are invisible or rearrange the text around them: C0 controls, DEL and the
 * C1 controls; the soft hyphen; the Arabic letter mark; zero-width space, joiners and the
 * left-to-right and right-to-left marks; line and paragraph separators; bidirectional
 * embeddings and overrides; the word joiner, invisible operators and bidirectional isolates;
 * and the zero-width no-break space (byte order mark).
 */
const INVISIBLE = /[\u0000-\u001F\u007F-\u009F\u00AD\u061C\u200B-\u200F\u2028-\u202E\u2060-\u2069\uFEFF]/g;

/**
 * `text` (a path from git, say) with characters that would hide or rearrange it made
 * visible (see INVISIBLE): C0 controls as their Unicode pictures (a newline as ␊), DEL as ␡,
 * the others by code point (‹U+202E›, ‹U+200B›). For display only; copies and tooltips keep
 * the exact text.
 */
export function visibleText(text: string): string {
  return text.replace(INVISIBLE, (c) => {
    const code = c.charCodeAt(0);
    if (code < 0x20) return String.fromCharCode(0x2400 + code);
    if (code === 0x7f) return "\u2421";
    return `\u2039U+${code.toString(16).toUpperCase().padStart(4, "0")}\u203A`;
  });
}

/**
 * A whole number with its thousands grouped ("10,000"), as `toLocaleString("en-US")` gives
 * but without starting the locale machinery, which costs tens of ms on a slow machine.
 */
export function groupDigits(n: number): string {
  if (!Number.isFinite(n)) return String(n);
  const whole = Math.trunc(Math.abs(n));
  // From 1e21 up, String() writes an exponent; BigInt writes every digit.
  const digits = whole < 1e21 ? String(whole) : BigInt(whole).toString();
  let out = "";
  for (let i = digits.length; i > 0; i -= 3) out = digits.slice(Math.max(0, i - 3), i) + (out ? `,${out}` : "");
  return (n < 0 ? "-" : "") + out;
}

/**
 * `arg` as a POSIX shell would need it written: as is when it holds only characters no shell
 * treats specially, else in single quotes (an embedded quote as '\''). For display and copying
 * only: the app never runs anything through a shell.
 */
export function shellQuote(arg: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(arg)) return arg;
  return `'${arg.replaceAll("'", "'\\''")}'`;
}

/** The command line of git with `args`, quoted for a shell ("git commit -m 'Fix it'"). */
export function commandLine(args: string[]): string {
  return ["git", ...args].map(shellQuote).join(" ");
}

/**
 * The lines of a command's output as a terminal would leave them: a line redrawn with
 * carriage returns (git's progress) shows its last state; a CRLF ending is one line end.
 * Characters that would hide or rearrange text are made visible (see visibleText); tabs stay.
 */
export function outputLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.map((line) => {
    const body = line.endsWith("\r") ? line.slice(0, -1) : line;
    const last = body.slice(body.lastIndexOf("\r") + 1);
    return last.split("\t").map(visibleText).join("\t");
  });
}

/** How long something took: "120 ms", "12.3 s", "1 min 4 s". */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const secs = Math.round(ms / 1000);
  return `${Math.floor(secs / 60)} min ${secs % 60} s`;
}

/** The local time of day of `ms` (epoch milliseconds): "09:05:07". */
export function clockTime(ms: number): string {
  const d = new Date(ms);
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}
