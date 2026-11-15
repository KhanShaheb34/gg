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

/** What the side panel shows of the worktrees' folders (see `worktreeFolders`). */
export type WorktreeFolders = {
  /** The folder most linked worktrees are in, home as `~`; null with no linked worktree. */
  parent: string | null;
  /** That folder in full. */
  parentPath: string | null;
  /** Each worktree's label: its folder name, or for one elsewhere its shortest distinguishing end ("~/…/other/fix"). */
  labels: Map<string, string>;
};

/**
 * How the side panel names the worktrees at `paths` (`main`: the main worktree's). The
 * folder most of the linked worktrees are in is the panel's `parent` (said once, in its
 * header); a worktree in it, and the main worktree, are named by their folder name alone. A
 * linked worktree elsewhere shows the shortest end of its path that tells it apart, at least
 * two folders, with what is left out as `…` ("~/…/other/fix"). Names that would be the same
 * get longer ends too. Folder names are never cut here: the row cuts them in the middle.
 */
export function worktreeFolders(paths: string[], main: string | null, home?: string | null): WorktreeFolders {
  const sep = separatorOf(paths[0] ?? "/");
  const parentOf = (p: string) => p.slice(0, Math.max(0, p.lastIndexOf(sep)));
  const nameOf = (p: string) => p.slice(p.lastIndexOf(sep) + 1);
  const linked = paths.filter((p) => p !== main);
  // The folder most linked worktrees are in (the first such, on a tie).
  const counts = new Map<string, number>();
  for (const p of linked) counts.set(parentOf(p), (counts.get(parentOf(p)) ?? 0) + 1);
  let parent: string | null = null;
  for (const [folder, n] of counts) if (parent === null || n > counts.get(parent)!) parent = folder;

  /** The last `k` folders of `p` (home as ~), with what is left out as "…". */
  const end = (p: string, k: number) => {
    const parts = shortenPath(p, home).split(sep);
    if (parts.length <= k + 1) return parts.join(sep);
    const lead = parts[0] === "~" ? ["~", "…"] : ["…"];
    return [...lead, ...parts.slice(-k)].join(sep);
  };
  const depth = new Map<string, number>();
  for (const p of paths) depth.set(p, p === main || parentOf(p) === parent ? 1 : 2);
  const label = (p: string) => (depth.get(p) === 1 ? nameOf(p) : end(p, depth.get(p)!));
  // Longer ends for names that would be the same, until each is told apart.
  for (let round = 0; round < 64; round++) {
    const seen = new Map<string, string[]>();
    for (const p of paths) seen.set(label(p), [...(seen.get(label(p)) ?? []), p]);
    const same = [...seen.values()].filter((group) => group.length > 1).flat();
    const longer = same.filter((p) => end(p, depth.get(p)!) !== end(p, depth.get(p)! + 1));
    if (longer.length === 0) break;
    for (const p of longer) depth.set(p, Math.max(2, depth.get(p)! + 1));
  }
  const labels = new Map(paths.map((p) => [p, label(p)] as [string, string]));
  return { parent: parent === null ? null : shortenPath(parent, home), parentPath: parent, labels };
}

/**
 * `text` cut in the middle to fit `room` (in the units of `width`, which measures a string):
 * its end (up to `keep` characters, at most half) stays and its start gives way, with "…"
 * between ("feature/pay…service-v2"); when even "…" and the end do not fit, the end gives way
 * too. Null when the whole text fits.
 */
export function cutToFit(text: string, room: number, keep: number, width: (s: string) => number): string | null {
  if (width(text) <= room) return null;
  const chars = [...text];
  let tail = Math.min(keep, Math.floor(chars.length / 2));
  const cut = (head: number, tailN: number) => chars.slice(0, head).join("") + "…" + chars.slice(chars.length - tailN).join("");
  const fits = (head: number, tailN: number) => width(cut(head, tailN)) <= room;
  while (tail > 0 && !fits(0, tail)) tail--;
  // The most of the start that fits beside the end.
  let lo = 0;
  let hi = chars.length - tail - 1;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (fits(mid, tail)) lo = mid;
    else hi = mid - 1;
  }
  return cut(lo, tail);
}

/**
 * `text` (a message from the core) with every absolute path under the home folder `home`
 * shown as `~/…`, as paths are shown elsewhere. Only a path that starts the text or follows a
 * space, quote or bracket is changed. Without `home`, `text` as is.
 */
export function homeInText(text: string, home?: string | null): string {
  if (!home) return text;
  const sep = separatorOf(home);
  let root = home;
  while (root.length > 1 && root.endsWith(sep)) root = root.slice(0, -1);
  if (root.length <= 1) return text;
  const escaped = (root + sep).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const flags = sep === "\\" ? "gi" : "g";
  return text.replace(new RegExp(`(^|[\\s"'“‘(\\[])${escaped}`, flags), `$1~${sep}`);
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
  // Safe characters only, and not first a character zsh expands there (=word, ~user).
  if (/^[A-Za-z0-9_@%+:,./-][A-Za-z0-9_@%+=:,./-]*$/.test(arg)) return arg;
  return `'${arg.replaceAll("'", "'\\''")}'`;
}

/** The command line of git with `args`, quoted for a shell ("git commit -m 'Fix it'"). */
export function commandLine(args: string[], program = "git"): string {
  return [program, ...args].map(shellQuote).join(" ");
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

/**
 * Whether `subject` is git's own message for merging `name` ("Merge branch 'name'", "Merge
 * remote-tracking branch 'name'", "Merge tag 'name'", "Merge commit 'name'", each optionally
 * followed by " into <branch>"), which only repeats a headline that names it.
 */
export function repeatsMergeName(subject: string, name: string): boolean {
  for (const kind of ["branch", "remote-tracking branch", "tag", "commit"]) {
    const head = `Merge ${kind} '${name}'`;
    if (subject === head) return true;
    if (subject.startsWith(`${head} into `) && !/\s/.test(subject.slice(head.length + 6))) return true;
  }
  return false;
}
