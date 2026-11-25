// A pull request comment's body (untrusted GitHub Markdown with HTML in it) as a tree of the
// few things the Pull request tab draws, for Markdown.svelte to render with its own elements:
// no HTML of a comment ever reaches the DOM, and nothing in it is ever fetched.
//
// `marked`'s lexer (only its lexer: none of its HTML output) reads the Markdown, as GitHub
// does for comments (GFM, a newline a line break). From its tokens:
//   - paragraphs, headings (drawn small), bold, italic, strikethrough, inline code, fenced
//     code, lists with their task boxes (read-only), quotes (GitHub's "> [!NOTE]" alerts
//     named), rules and tables;
//   - links: a link only for an absolute https:// address (shown in full in its tooltip,
//     opened through the core); anything else (another scheme, a relative address, an
//     #anchor) is its text;
//   - images (Markdown or <img>): never loaded; "[image: alt]", or nothing without alt text;
//   - HTML: comments dropped with what is in them; <details><summary> a closed disclosure;
//     <br> a line break; every other tag dropped, its text kept (script and style dropped
//     with what is in them); entities decoded as text;
//   - bodies longer than MAX_BODY characters cut (said, with a way to GitHub), nesting deeper
//     than MAX_DEPTH flattened to text, so no comment can hang the panel.

import { Lexer, type Token, type Tokens } from "marked";

export type Inline =
  | { k: "text"; text: string }
  | { k: "strong" | "em" | "del"; children: Inline[] }
  | { k: "code"; text: string }
  /** `url`: an absolute https:// address (null: shown as its text, never a link). */
  | { k: "link"; url: string | null; children: Inline[] }
  | { k: "image"; alt: string }
  | { k: "br" };

export type ListItem = { task: boolean | null; blocks: Block[] };

export type Block =
  | { k: "p"; children: Inline[] }
  /** A tight list item's text: inline, without a paragraph's spacing. */
  | { k: "plain"; children: Inline[] }
  | { k: "heading"; level: number; children: Inline[] }
  | { k: "code"; text: string; lang: string }
  | { k: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { k: "quote"; alert: Alert | null; blocks: Block[] }
  | { k: "hr" }
  | { k: "table"; align: ("left" | "center" | "right" | null)[]; head: Inline[][]; rows: Inline[][][] }
  | { k: "details"; summary: Inline[]; blocks: Block[] };

export type Alert = "Note" | "Tip" | "Important" | "Warning" | "Caution";

/** A body as drawn: its blocks, and whether it was cut (MAX_BODY). */
export type Rendered = { blocks: Block[]; cut: boolean };

/** Most characters of a body read; the rest is on GitHub. */
export const MAX_BODY = 20_000;
/** Deepest nesting drawn (quotes, lists, disclosures); deeper is flattened to text. */
export const MAX_DEPTH = 10;
/** Longest link address taken (the core refuses longer). */
export const MAX_LINK = 2048;

// --- Text ----------------------------------------------------------------------------------

/** The named character references comments use; any other is left as written. */
const NAMED: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  ensp: " ",
  emsp: " ",
  thinsp: " ",
  hellip: "…",
  mdash: "—",
  ndash: "–",
  lsquo: "‘",
  rsquo: "’",
  ldquo: "“",
  rdquo: "”",
  laquo: "«",
  raquo: "»",
  middot: "·",
  bull: "•",
  copy: "©",
  reg: "®",
  trade: "™",
  times: "×",
  divide: "÷",
  plusmn: "±",
  deg: "°",
  para: "¶",
  sect: "§",
  larr: "←",
  rarr: "→",
  uarr: "↑",
  darr: "↓",
  harr: "↔",
  check: "✓",
  cross: "✗",
  euro: "€",
  pound: "£",
  yen: "¥",
  cent: "¢",
  zwj: "‍",
  zwnj: "‌",
};

/** Bidirectional overrides and isolates, which could make text read other than it is. */
const BIDI = /[‪-‮⁦-⁩]/g;

/** `text` with its character references (`&amp;`, `&#x2192;`) decoded, as text. */
export function decodeEntities(text: string): string {
  if (!text.includes("&")) return text;
  return text.replace(/&(#[xX][0-9a-fA-F]{1,6}|#[0-9]{1,7}|[A-Za-z][A-Za-z0-9]{1,31});/g, (whole, ref: string) => {
    if (ref[0] !== "#") return Object.hasOwn(NAMED, ref) ? NAMED[ref] : whole;
    const code = ref[1] === "x" || ref[1] === "X" ? parseInt(ref.slice(2), 16) : parseInt(ref.slice(1), 10);
    if (code === 0 || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) return "�";
    return String.fromCodePoint(code);
  });
}

/** `text` as drawn: bidirectional controls shown by code point (‹U+202E›), never acted on. */
function shown(text: string): string {
  return text.replace(BIDI, (c) => `‹U+${c.charCodeAt(0).toString(16).toUpperCase()}›`);
}

/**
 * `href` (as written in a comment, references decoded) as a link's address: an absolute
 * https:// address, at most MAX_LINK long, with no user name or password, spaces or control
 * characters; null for anything else (another scheme, a relative address, an #anchor).
 */
export function safeUrl(href: string | null | undefined): string | null {
  if (!href) return null;
  const raw = href.trim();
  if (raw.length > MAX_LINK || /[\s\u0000-\u001f\u007f]/.test(raw)) return null;
  if (!/^https:\/\//i.test(raw)) return null;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" || url.hostname === "" || url.username !== "" || url.password !== "") return null;
  return url.href.length > MAX_LINK ? null : url.href;
}

// --- HTML, read as text ----------------------------------------------------------------------

/** A piece of HTML: text, a tag (`attrs`: what follows its name), or a comment (dropped). */
export type HtmlPiece = { t: "text"; text: string } | { t: "open" | "close"; name: string; attrs: string } | { t: "comment" };

/**
 * `html` in pieces. A tag or comment left open at the end is dropped with everything after
 * it; `<!…>` and `<?…>` are dropped; a `<` that starts no tag is text.
 */
export function htmlPieces(html: string): HtmlPiece[] {
  const out: HtmlPiece[] = [];
  let i = 0;
  let text = "";
  const flush = () => {
    if (text !== "") out.push({ t: "text", text });
    text = "";
  };
  while (i < html.length) {
    const lt = html.indexOf("<", i);
    if (lt < 0) {
      text += html.slice(i);
      break;
    }
    text += html.slice(i, lt);
    if (html.startsWith("<!--", lt)) {
      flush();
      const end = html.indexOf("-->", lt + 4);
      out.push({ t: "comment" });
      if (end < 0) return out;
      i = end + 3;
      continue;
    }
    const tag = /^<(\/?)([A-Za-z][A-Za-z0-9-]*)/.exec(html.slice(lt, lt + 64));
    if (!tag && !/^<[!?]/.test(html.slice(lt, lt + 2))) {
      text += "<";
      i = lt + 1;
      continue;
    }
    // The tag's end: the first > outside quotes.
    let j = lt + (tag ? tag[0].length : 2);
    let quote: string | null = null;
    for (; j < html.length; j++) {
      const c = html[j];
      if (quote) {
        if (c === quote) quote = null;
      } else if (c === '"' || c === "'") quote = c;
      else if (c === ">") break;
    }
    flush();
    if (j >= html.length) return out;
    if (tag) {
      out.push({ t: tag[1] ? "close" : "open", name: tag[2].toLowerCase(), attrs: html.slice(lt + tag[0].length, j) });
    }
    i = j + 1;
  }
  flush();
  return out;
}

/** An attribute's value in a tag's `attrs`, decoded; null when it has none. */
function attribute(attrs: string, name: string): string | null {
  const m = new RegExp(`(?:^|\\s)${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s"'>]+))`, "i").exec(attrs);
  return m ? decodeEntities(m[1] ?? m[2] ?? m[3] ?? "") : null;
}

/** Elements whose content is not text to show: dropped with it. */
const RAW = new Set(["script", "style", "template", "textarea", "title", "noscript", "iframe", "object", "svg", "math"]);
/** Elements that end a run of text (a paragraph break between what is before and after). */
const BREAKS = new Set([
  "p",
  "div",
  "dl",
  "dt",
  "dd",
  "ul",
  "ol",
  "li",
  "table",
  "thead",
  "tbody",
  "tr",
  "td",
  "th",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "blockquote",
  "pre",
  "hr",
  "section",
  "article",
  "header",
  "footer",
  "center",
]);

// --- Building the tree ---------------------------------------------------------------------

/** Adds `node` to `out`, joining text to the text before it. */
function add(out: Inline[], node: Inline): void {
  const last = out[out.length - 1];
  if (node.k === "text" && last?.k === "text") last.text += node.text;
  else if (node.k !== "text" || node.text !== "") out.push(node);
}

/** What a run of inline tokens reads, across tokens: dropping a raw element's content. */
type InlineState = { skip: string | null };

/** Adds what HTML piece `piece` draws inline to `out` (see the module comment). */
function pieceInline(piece: HtmlPiece, out: Inline[], state: InlineState): void {
  if (state.skip !== null) {
    if (piece.t === "close" && piece.name === state.skip) state.skip = null;
    return;
  }
  if (piece.t === "text") {
    add(out, { k: "text", text: shown(decodeEntities(piece.text).replace(/\s+/g, " ")) });
  } else if (piece.t === "open" && RAW.has(piece.name) && !/\/\s*$/.test(piece.attrs)) {
    state.skip = piece.name;
  } else if (piece.t === "open" && piece.name === "img") {
    const alt = attribute(piece.attrs, "alt")?.trim();
    if (alt) add(out, { k: "image", alt: shown(alt) });
  } else if (piece.t === "open" && piece.name === "br") {
    add(out, { k: "br" });
  }
}

/** `list` without the line breaks and blank text at its ends. */
function trimmed(list: Inline[]): Inline[] {
  const out = [...list];
  const blank = (n: Inline | undefined) => n !== undefined && (n.k === "br" || (n.k === "text" && n.text.trim() === ""));
  while (blank(out[0])) out.shift();
  while (blank(out[out.length - 1])) out.pop();
  const first = out[0];
  if (first?.k === "text") out[0] = { k: "text", text: first.text.trimStart() };
  const last = out[out.length - 1];
  if (last?.k === "text") out[out.length - 1] = { k: "text", text: last.text.trimEnd() };
  return out;
}

function inlines(tokens: readonly Token[] | undefined, depth: number, state: InlineState = { skip: null }): Inline[] {
  const out: Inline[] = [];
  for (const t of tokens ?? []) {
    if (state.skip !== null && t.type !== "html") continue;
    switch (t.type) {
      case "text":
      case "escape": {
        // An escaped character ("\*") is itself, never a reference to decode.
        const tok = t as Tokens.Text;
        const literal = t.type === "escape" || tok.escaped === true;
        if (tok.tokens && tok.tokens.length > 0) for (const n of inlines(tok.tokens, depth + 1, state)) add(out, n);
        else add(out, { k: "text", text: shown(literal ? tok.text : decodeEntities(tok.text)) });
        break;
      }
      case "strong":
      case "em":
      case "del": {
        const children = depth >= MAX_DEPTH ? [{ k: "text" as const, text: shown(textOf(t)) }] : inlines((t as Tokens.Strong).tokens, depth + 1, state);
        if (children.length > 0) out.push({ k: t.type, children });
        break;
      }
      case "codespan":
        out.push({ k: "code", text: shown((t as Tokens.Codespan).text) });
        break;
      case "br":
        add(out, { k: "br" });
        break;
      case "link": {
        const link = t as Tokens.Link;
        const url = safeUrl(decodeEntities(link.href));
        const children = depth >= MAX_DEPTH ? [] : inlines(link.tokens, depth + 1, state).filter((n) => n.k !== "link");
        if (children.length === 0) children.push({ k: "text", text: shown(decodeEntities(link.text || link.href)) });
        out.push({ k: "link", url, children });
        break;
      }
      case "image": {
        const alt = decodeEntities((t as Tokens.Image).text).trim();
        if (alt) add(out, { k: "image", alt: shown(alt) });
        break;
      }
      case "html":
        for (const piece of htmlPieces((t as Tokens.HTML).text)) pieceInline(piece, out, state);
        break;
      case "checkbox":
        break;
      default: {
        const g = t as Tokens.Generic;
        if (g.tokens) for (const n of inlines(g.tokens, depth + 1, state)) add(out, n);
        else if (typeof g.text === "string") add(out, { k: "text", text: shown(decodeEntities(g.text)) });
      }
    }
  }
  return out;
}

/** Everything `token` says, as plain text (for what is nested too deep to draw). */
function textOf(token: Token): string {
  const g = token as Tokens.Generic;
  if (g.type === "html") {
    return htmlPieces(g.text as string)
      .map((p) => (p.t === "text" ? decodeEntities(p.text) : ""))
      .join("");
  }
  if (g.type === "list") return (g.items as Tokens.ListItem[]).map((i) => i.tokens.map(textOf).join(" ")).join("\n");
  if (g.tokens && g.tokens.length > 0) return g.tokens.map(textOf).join("");
  return typeof g.text === "string" ? decodeEntities(g.text) : "";
}

const ALERT = /^\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\][ \t]*/i;

/** A quote's GitHub alert ("> [!WARNING]"), taken off its first paragraph. */
function alertOf(blocks: Block[]): Alert | null {
  const first = blocks[0];
  if (first?.k !== "p") return null;
  const lead = first.children[0];
  if (lead?.k !== "text") return null;
  const m = ALERT.exec(lead.text);
  if (!m) return null;
  lead.text = lead.text.slice(m[0].length);
  if (lead.text === "") first.children.shift();
  while (first.children[0]?.k === "br") first.children.shift();
  if (first.children.length === 0) blocks.shift();
  const name = m[1].toLowerCase();
  return (name[0].toUpperCase() + name.slice(1)) as Alert;
}

type Details = Extract<Block, { k: "details" }>;

/** Blocks for `tokens` at nesting `depth`: the HTML among them read as text, its <details> disclosures. */
function blocks(tokens: readonly Token[], depth: number): Block[] {
  if (depth > MAX_DEPTH) {
    const text = tokens.map(textOf).join("\n").trim();
    return text ? [{ k: "p", children: [{ k: "text", text: shown(text) }] }] : [];
  }
  const root: Block[] = [];
  const open: Details[] = [];
  const here = () => open[open.length - 1]?.blocks ?? root;
  // Text of HTML blocks, waiting to be a paragraph.
  let pending: Inline[] = [];
  // Where a <summary>'s text goes, while one is open.
  let summary: Inline[] | null = null;
  const state: InlineState = { skip: null };
  const flush = () => {
    const children = trimmed(pending);
    pending = [];
    if (children.length > 0) here().push({ k: "p", children });
  };
  for (const t of tokens) {
    if (t.type === "space") continue;
    if (t.type !== "html") {
      summary = null;
      if (state.skip !== null) continue;
      flush();
      here().push(...block(t, depth + open.length));
      continue;
    }
    for (const piece of htmlPieces((t as Tokens.HTML).text)) {
      if (state.skip !== null || piece.t === "comment") {
        pieceInline(piece, pending, state);
        continue;
      }
      if (piece.t === "text") {
        if (summary) pieceInline(piece, summary, state);
        // Space between two tags' text is kept (one space); at a paragraph's start it is not.
        else if (piece.text.trim() !== "" || pending.length > 0) pieceInline(piece, pending, state);
        continue;
      }
      if (piece.name === "details") {
        flush();
        summary = null;
        if (piece.t === "close") open.pop();
        else if (depth + open.length < MAX_DEPTH) {
          const d: Details = { k: "details", summary: [], blocks: [] };
          here().push(d);
          open.push(d);
        }
      } else if (piece.name === "summary") {
        summary = piece.t === "open" && open.length > 0 ? open[open.length - 1].summary : null;
      } else if (!summary && BREAKS.has(piece.name)) {
        flush();
      } else {
        pieceInline(piece, summary ?? pending, state);
      }
    }
  }
  flush();
  for (const b of root) if (b.k === "details") trimSummary(b);
  return root;
}

/** A summary's text without the space around it; "Details" when it has none. */
function trimSummary(d: Details): void {
  d.summary = trimmed(d.summary);
  if (d.summary.length === 0) d.summary = [{ k: "text", text: "Details" }];
  for (const b of d.blocks) if (b.k === "details") trimSummary(b);
}

/** The blocks of one block token (none for what draws nothing). */
function block(t: Token, depth: number): Block[] {
  switch (t.type) {
    case "paragraph": {
      const children = inlines((t as Tokens.Paragraph).tokens, depth);
      return children.length > 0 ? [{ k: "p", children }] : [];
    }
    case "text": {
      const tok = t as Tokens.Text;
      const children = tok.tokens ? inlines(tok.tokens, depth) : [{ k: "text" as const, text: shown(decodeEntities(tok.text)) }];
      return children.length > 0 ? [{ k: "plain", children }] : [];
    }
    case "heading": {
      const h = t as Tokens.Heading;
      const children = inlines(h.tokens, depth);
      return children.length > 0 ? [{ k: "heading", level: h.depth, children }] : [];
    }
    case "code": {
      const c = t as Tokens.Code;
      return [{ k: "code", text: shown(c.text), lang: (c.lang ?? "").split(/\s/)[0].slice(0, 32) }];
    }
    case "blockquote": {
      const inner = blocks((t as Tokens.Blockquote).tokens, depth + 1);
      const alert = alertOf(inner);
      return inner.length > 0 || alert ? [{ k: "quote", alert, blocks: inner }] : [];
    }
    case "list": {
      const l = t as Tokens.List;
      const start = typeof l.start === "number" ? l.start : Number.parseInt(String(l.start), 10);
      return [
        {
          k: "list",
          ordered: l.ordered,
          start: Number.isFinite(start) ? start : 1,
          items: l.items.map((item) => ({
            task: item.task ? !!item.checked : null,
            blocks: blocks(item.tokens, depth + 1),
          })),
        },
      ];
    }
    case "hr":
      return [{ k: "hr" }];
    case "table": {
      const tb = t as Tokens.Table;
      return [
        {
          k: "table",
          align: tb.align,
          head: tb.header.map((c) => inlines(c.tokens, depth)),
          rows: tb.rows.map((r) => r.map((c) => inlines(c.tokens, depth))),
        },
      ];
    }
    case "def":
    case "space":
      return [];
    default: {
      const text = textOf(t).trim();
      return text ? [{ k: "p", children: [{ k: "text", text: shown(text) }] }] : [];
    }
  }
}

/** `body` cut to at most `max` characters, at a line's end when one is near. */
export function capBody(body: string, max = MAX_BODY): { text: string; cut: boolean } {
  if (body.length <= max) return { text: body, cut: false };
  let end = body.lastIndexOf("\n", max);
  if (end < max * 0.8) end = max;
  // Not between the halves of a surrogate pair.
  const code = body.charCodeAt(end - 1);
  if (code >= 0xd800 && code <= 0xdbff) end--;
  return { text: body.slice(0, end), cut: true };
}

/** Comment `body` as drawn (see the module comment). */
export function renderMarkdown(body: string, max = MAX_BODY): Rendered {
  const { text, cut } = capBody(body.replace(/\r\n?/g, "\n").replace(/\u0000/g, "�"), max);
  let tokens: Token[];
  try {
    tokens = new Lexer({ gfm: true, breaks: true }).lex(text);
  } catch {
    // Never expected; the text as it is, then.
    return { blocks: [{ k: "p", children: [{ k: "text", text: shown(text) }] }], cut };
  }
  return { blocks: blocks(tokens, 0), cut };
}

/** How many lines a body takes at most, roughly (to tell a long one before it is drawn). */
export function roughLines(body: string): number {
  let lines = 1;
  for (let i = body.indexOf("\n"); i >= 0 && lines < 1000; i = body.indexOf("\n", i + 1)) lines++;
  return lines;
}
