// What a notice says, as the toast draws it: a headline and a quieter line under it, each
// plain text or parts (text, names set off, paths). The core gives an operation's notice in
// parts (`OpOutcome.notice`); the UI's own notices are mostly plain text.

import type { Part } from "./types";

/** A line of a notice: plain text, or parts. */
export type Rich = string | Part[];

/** `line` as parts. */
export function partsOf(line: Rich | undefined): Part[] {
  if (line === undefined) return [];
  if (typeof line === "string") return line === "" ? [] : [{ text: line }];
  return line;
}

/** The text of `part`, a name or path as it is. */
export function textOf(part: Part): string {
  return "text" in part ? part.text : "ref" in part ? part.ref : part.path;
}

/** `line` as plain text. */
export function plainText(line: Rich | undefined): string {
  return partsOf(line).map(textOf).join("");
}

/** Most characters before a colon for what comes before it to be read as a headline. */
const MAX_LEAD = 48;

/**
 * A notice written as plain sentences, with nothing under its headline, in the shape notices
 * have: split at a colon in its first sentence after a short lead ("Removed the gg command:
 * ~/bin/gg." reads "Removed the gg command", then "~/bin/gg."), else after its first sentence;
 * one sentence stays a headline. As the core shapes its own (`notice.rs`, `plain_split`).
 */
export function shape(message: Rich, note?: Rich): { title: Rich; note: Rich } {
  if (typeof message !== "string" || partsOf(note).length > 0) return { title: message, note: note ?? [] };
  const sentence = /[.!?] (?=[A-Z\u201c])/.exec(message);
  const colon = /: /.exec(message);
  // The programs keep their lower case, as gg writes them.
  const cap = (s: string) => (/^(git|gg|gh|ssh)\b/.test(s) ? s : s.charAt(0).toUpperCase() + s.slice(1));
  if (colon && (!sentence || colon.index < sentence.index) && [...message.slice(0, colon.index)].length <= MAX_LEAD) {
    return { title: message.slice(0, colon.index), note: cap(message.slice(colon.index + 2)) };
  }
  if (sentence) {
    const end = message[sentence.index] === "." ? sentence.index : sentence.index + 1;
    return { title: message.slice(0, end), note: message.slice(sentence.index + 2) };
  }
  return { title: message, note: [] };
}

/**
 * Most characters of text (names aside) in a line that is kept on one line: a headline's
 * words are bolder and wider than the quieter line's, and its dismiss button is beside it.
 */
const MAX_COMPACT_TEXT = { title: 24, note: 44 } as const;

/**
 * Whether `line` (a headline or the line under it) is a short line of names ("feature/x →
 * main · pushed to origin"), kept on one line with its names cut in the middle to fit, rather
 * than wrapped around them.
 */
export function compact(line: Rich, as: "title" | "note"): boolean {
  const parts = partsOf(line);
  const names = parts.filter((p) => !("text" in p)).length;
  const text = parts.reduce((n, p) => n + ("text" in p ? [...p.text].length : 0), 0);
  return names > 0 && text <= MAX_COMPACT_TEXT[as];
}

/**
 * A headline as drawn: its closing full stop goes ("Copied the hash."), an ellipsis or a
 * question stays. Only a text part at the end is touched.
 */
export function headline(line: Rich): Part[] {
  const parts = partsOf(line);
  const last = parts[parts.length - 1];
  if (!last || !("text" in last) || !/[^.]\.$/.test(last.text)) return parts;
  return [...parts.slice(0, -1), { text: last.text.slice(0, -1) }];
}

/**
 * The notice as one sentence, for what is read out: the headline, a full stop unless it ends
 * with one (or a colon), then the line under it; its short forms in words (" → " as " into ",
 * " · " as ", "), as the core's `message` says it.
 */
export function spoken(title: Rich, note?: Rich): string {
  const words = (s: string) => s.replaceAll(" → ", " into ").replaceAll(" · ", ", ");
  const head = words(plainText(headline(title)));
  const rest = words(plainText(note));
  if (rest === "") return head;
  const stop = /[.:!?]$/.test(head) ? " " : ". ";
  const end = /[.!?)”]$/.test(rest) ? "" : ".";
  return `${head}${stop}${rest}${end}`;
}
