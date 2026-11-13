// Commit search helpers: highlighting matches in a subject and stepping through the hits
// (ascending row indices from the core's `search_commits`).

export type Part = { text: string; match: boolean };

/**
 * A pattern finding `query` (trimmed, taken literally, any case) the way the core matches
 * subjects; null for a blank query.
 */
export function searchPattern(query: string): RegExp | null {
  const q = query.trim();
  if (q === "") return null;
  return new RegExp(q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "giu");
}

/** `text` cut into the stretches `pattern` matches and those between them. */
export function matchParts(text: string, pattern: RegExp | null): Part[] {
  if (!pattern) return [{ text, match: false }];
  const parts: Part[] = [];
  let last = 0;
  for (const m of text.matchAll(pattern)) {
    const at = m.index;
    if (m[0] === "") continue;
    if (at > last) parts.push({ text: text.slice(last, at), match: false });
    parts.push({ text: m[0], match: true });
    last = at + m[0].length;
  }
  if (last < text.length || parts.length === 0) parts.push({ text: text.slice(last), match: false });
  return parts;
}

/** Index of the first hit at or after row `row` (hits.length if none). */
function lowerBound(hits: number[], row: number): number {
  let lo = 0;
  let hi = hits.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (hits[mid] < row) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Position of row `row` among `hits`, or -1 when it is not a hit. */
export function hitPosition(hits: number[], row: number | null): number {
  if (row === null) return -1;
  const at = lowerBound(hits, row);
  return hits[at] === row ? at : -1;
}

/**
 * The hit after (`dir` 1) or before (-1) row `from`, wrapping around; from no row, the first
 * (or last) hit. Null without hits.
 */
export function stepHit(hits: number[], from: number | null, dir: 1 | -1): number | null {
  if (hits.length === 0) return null;
  if (from === null) return dir === 1 ? hits[0] : hits[hits.length - 1];
  if (dir === 1) {
    const at = lowerBound(hits, from + 1);
    return hits[at < hits.length ? at : 0];
  }
  const at = lowerBound(hits, from) - 1;
  return hits[at >= 0 ? at : hits.length - 1];
}

/** A query as matched: trimmed and lowercased; null when blank. */
export function needleOf(query: string): string | null {
  const q = query.trim().toLowerCase();
  return q === "" ? null : q;
}

/**
 * `text` split around the first stretch that matches `needle` (from `needleOf`) ignoring
 * case: [before, match, after]; [text, "", ""] without a match. Characters are lowercased
 * one at a time, as the core's search does (`char::to_lowercase`), so a match found there is
 * found here, also where lowercasing changes the text's length.
 */
export function splitMatch(text: string, needle: string): [string, string, string] {
  let at: number;
  let end: number;
  // ASCII lowercases in place: one indexOf.
  if (/^[\x00-\x7f]*$/.test(text)) {
    at = text.toLowerCase().indexOf(needle);
    end = at + needle.length;
  } else {
    let lowered = "";
    const starts: number[] = []; // index in `text` of each code unit of `lowered`
    for (let i = 0; i < text.length; ) {
      const cp = text.codePointAt(i)!;
      const ch = String.fromCodePoint(cp);
      const low = ch.toLowerCase();
      for (let k = 0; k < low.length; k++) starts.push(i);
      lowered += low;
      i += ch.length;
    }
    starts.push(text.length);
    const found = lowered.indexOf(needle);
    at = found < 0 ? -1 : starts[found];
    end = found < 0 ? -1 : starts[found + needle.length];
    // A match ending inside a character's lowercase form takes the whole character.
    if (found >= 0 && found + needle.length < lowered.length && starts[found + needle.length] === starts[found + needle.length - 1]) {
      const cp = text.codePointAt(end)!;
      end += String.fromCodePoint(cp).length;
    }
  }
  if (at < 0) return [text, "", ""];
  return [text.slice(0, at), text.slice(at, end), text.slice(end)];
}

/**
 * How many characters of the short hash to mark for `needle`: its length (at most 8) when it
 * is 4 or more hex digits the hash starts with, as the core matches hashes; else 0.
 */
export function hashMatch(hash: string, needle: string | null): number {
  if (needle === null || needle.length < 4 || !/^[0-9a-f]+$/.test(needle) || !hash.startsWith(needle)) return 0;
  return Math.min(needle.length, 8);
}
