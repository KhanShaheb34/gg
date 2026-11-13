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
