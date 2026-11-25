import { render } from "svelte/server";
import { describe, expect, it } from "vitest";
import Markdown from "./components/Markdown.svelte";
// The captured CodeRabbit comment (crates/gg-core/tests/fixtures/gh), as text.
import capturedView from "../../crates/gg-core/tests/fixtures/gh/pr22-view.json?raw";
import { capBody, decodeEntities, htmlPieces, MAX_BODY, renderMarkdown, safeUrl, type Block, type Inline } from "./markdown";

/** A body drawn by the real component (server-side), as HTML. */
function drawn(body: string): string {
  return render(Markdown, { props: { blocks: renderMarkdown(body).blocks, onlink: () => {} } }).body;
}

/**
 * The tags, attributes and addresses in drawn HTML, for the rules every body must keep. The
 * renderer escapes every < of text, so each < here starts one of the component's own tags.
 */
function inspect(html: string) {
  const tagTexts = [...html.matchAll(/<([a-zA-Z][a-zA-Z0-9-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>/g)];
  const tags = tagTexts.map((m) => m[1].toLowerCase());
  const pairs = tagTexts.flatMap((m) => [...m[2].matchAll(/([^\s=/"']+)(?:\s*=\s*("([^"]*)"|'([^']*)'|[^\s"'>]+))?/g)]);
  const attrs = [...new Set(pairs.map((p) => p[1].toLowerCase()))];
  const hrefs = pairs.filter((p) => /^(href|src|srcset|data-url|action|formaction|xlink:href|title)$/i.test(p[1]) && /^[a-z][a-z0-9+.-]*:/i.test(p[3] ?? p[4] ?? "")).map((p) => p[3] ?? p[4] ?? "");
  return { tags, attrs, hrefs };
}

/** Only this component's own elements, never a comment's. */
const ALLOWED = new Set(["div", "p", "span", "strong", "em", "del", "code", "pre", "br", "ul", "ol", "li", "input", "blockquote", "hr", "table", "thead", "tbody", "tr", "th", "td", "details", "summary"]);

function text(nodes: Inline[]): string {
  return nodes.map((n) => (n.k === "text" ? n.text : n.k === "br" ? "\n" : n.k === "code" ? n.text : n.k === "image" ? `‹img:${n.alt}›` : "children" in n ? text(n.children) : "")).join("");
}

function first(body: string): Block {
  return renderMarkdown(body).blocks[0];
}

describe("comment bodies never bring HTML, scripts or remote content into the page", () => {
  const hostile: [string, string][] = [
    ["a script tag", "<script>alert(1)</script>\n\nafter"],
    ["a script inline", "before <script>alert(1)</script> after"],
    ["an unterminated script", "text <script>alert(1)"],
    ["an event handler", '<b onclick="alert(1)" onmouseover=alert(2)>bold</b> and <div onload="x()">div</div>'],
    ["an image with onerror", '<img src="x" onerror="alert(1)" alt="cat">'],
    ["a Markdown image", "![a cat](https://example.com/cat.png) and ![](https://example.com/x.png)"],
    ["a javascript: link", "[click](javascript:alert(1)) and [CLICK](JAVASCRIPT:alert(1))"],
    ["an entity-hidden javascript: link", "[x](&#106;avascript:alert(1)) [y](java&#x73;cript:alert(1))"],
    ["a data: link", "[d](data:text/html,<script>alert(1)</script>)"],
    ["an http link", "[plain](http://example.com/)"],
    ["a relative link and an anchor", "[rel](/octo/cat/blob/main/x) [top](#top) [dot](./x)"],
    ["an iframe", '<iframe src="https://example.com"></iframe>text'],
    ["a style tag", "<style>body{display:none}</style>visible"],
    ["a style attribute", '<span style="position:fixed;inset:0">over everything</span>'],
    ["a form", '<form action="https://evil.example"><input name=x><button formaction="https://evil.example">go</button></form>'],
    ["an object and embed", '<object data="https://x"></object><embed src="https://x">'],
    ["svg with script", '<svg onload="alert(1)"><script>alert(2)</script></svg>after'],
    ["a picture", '<picture><source srcset="https://x/a.svg"><img src="https://x/b.svg" alt="badge"></picture>'],
    ["nested details", "<details><summary>One</summary>\n\n<details><summary>Two <img src=x onerror=alert(1)></summary>\n\ninner\n\n</details>\n\n</details>"],
    ["unterminated HTML", '<a href="javascript:alert(1)" onclick="alert(1)'],
    ["an unterminated comment", "shown <!-- hidden <script>alert(1)</script>"],
    ["a meta refresh", '<meta http-equiv="refresh" content="0;url=https://evil.example">'],
    ["a base tag", '<base href="https://evil.example/">'],
    ["a link tag", '<link rel="stylesheet" href="https://evil.example/x.css">'],
    ["autolinks", "<javascript:alert(1)> <https://ok.example.com/a> www.example.com"],
  ];

  it.each(hostile)("%s", (_name, body) => {
    const html = drawn(body);
    const { tags, attrs, hrefs } = inspect(html);
    for (const tag of tags) expect(ALLOWED.has(tag), `<${tag}> in ${html}`).toBe(true);
    for (const banned of ["img", "script", "iframe", "style", "object", "embed", "form", "meta", "base", "link", "svg", "a", "source", "picture"]) {
      expect(tags).not.toContain(banned);
    }
    for (const a of attrs) expect(a.startsWith("on"), `${a} in ${html}`).toBe(false);
    expect(attrs).not.toContain("style");
    expect(attrs).not.toContain("href");
    expect(attrs).not.toContain("src");
    for (const h of hrefs) expect(h, html).toMatch(/^https:\/\//);
  });

  it("a link is a link only for an https address, shown in full in its tooltip, with no href", () => {
    const html = drawn("[docs](https://docs.example.com/a?b=1&amp;c=2) [bad](javascript:alert(1))");
    expect(html).toContain('role="link"');
    expect(html).toContain('title="https://docs.example.com/a?b=1&amp;c=2"');
    expect(html).not.toContain("href=");
    // The refused link is its text.
    const b = first("[bad](javascript:alert(1))");
    expect(b).toEqual({ k: "p", children: [{ k: "link", url: null, children: [{ k: "text", text: "bad" }] }] });
  });

  it("images are never loaded: their alt text, muted, without brackets, named as not loaded", () => {
    expect(first("![a cat](https://x/cat.png)")).toEqual({ k: "p", children: [{ k: "image", alt: "a cat" }] });
    const html = drawn('<img src="https://x/y.png" alt="Passed">');
    expect(html).toMatch(/<span class="md-image[^"]*" role="img" title="An image, not loaded" aria-label="Image, not loaded: Passed">Passed<\/span>/);
    expect(html).not.toContain("[image");
    expect(html).not.toContain("https://x/y.png");
  });
});

describe("what a comment's Markdown draws", () => {
  it("paragraphs, emphasis, code, a line break per newline (as GitHub's comments)", () => {
    const b = first("one **two** _three_ ~~four~~ `five`\nsix");
    expect(b.k).toBe("p");
    const kinds = (b as { children: Inline[] }).children.map((n) => n.k);
    expect(kinds).toEqual(["text", "strong", "text", "em", "text", "del", "text", "code", "br", "text"]);
  });

  it("headings are kept small, fenced code keeps its text, lists, tasks, quotes, rules, tables", () => {
    const blocks = renderMarkdown("# Big\n\n```ts\nconst a = 1;\n```\n\n- [x] done\n- [ ] not\n\n1. one\n2. two\n\n> quoted\n\n---\n\n|a|b|\n|-|:-:|\n|1|2|").blocks;
    expect(blocks.map((b) => b.k)).toEqual(["heading", "code", "list", "list", "quote", "hr", "table"]);
    expect(blocks[1]).toEqual({ k: "code", text: "const a = 1;", lang: "ts" });
    const tasks = blocks[2] as Extract<Block, { k: "list" }>;
    expect(tasks.items.map((i) => i.task)).toEqual([true, false]);
    const table = blocks[6] as Extract<Block, { k: "table" }>;
    expect(table.align).toEqual([null, "center"]);
    const html = drawn("# Big\n\n- [x] done");
    expect(html).toMatch(/<p class="md-h md-h1[^"]*"/);
    expect(html).toMatch(/<input[^>]*disabled/);
    expect(html).not.toMatch(/<h1/);
  });

  it("GitHub's alerts are named", () => {
    const q = first("> [!WARNING]\n> ## Review limit reached\n> Wait a little.") as Extract<Block, { k: "quote" }>;
    expect(q.alert).toBe("Warning");
    expect(q.blocks[0].k).toBe("heading");
    expect((first("> [!note] hi") as Extract<Block, { k: "quote" }>).alert).toBe("Note");
    expect((first("> plain") as Extract<Block, { k: "quote" }>).alert).toBe(null);
  });

  it("HTML: comments dropped, details a disclosure, other tags dropped with their text kept", () => {
    expect(renderMarkdown("<!-- hidden -->\n\nshown <!-- also hidden --> here").blocks).toEqual([{ k: "p", children: [{ k: "text", text: "shown  here" }] }]);
    const d = first("<details>\n<summary><strong>Walk</strong>through</summary>\n\nInside **it**.\n\n</details>") as Extract<Block, { k: "details" }>;
    expect(d.k).toBe("details");
    expect(text(d.summary)).toBe("Walkthrough");
    expect(d.blocks[0].k).toBe("p");
    expect(first("<details>\n\nno summary\n\n</details>")).toMatchObject({ k: "details", summary: [{ k: "text", text: "Details" }] });
    // Nested, and closed in the right places.
    const nested = renderMarkdown("<details><summary>A</summary>\n\n<details><summary>B</summary>\n\nb\n\n</details>\n\na\n\n</details>\n\nafter").blocks;
    expect(nested.map((b) => b.k)).toEqual(["details", "p"]);
    const outer = nested[0] as Extract<Block, { k: "details" }>;
    expect(outer.blocks.map((b) => b.k)).toEqual(["details", "p"]);
    // Text of other tags kept, the tags gone.
    expect(text((first('<div align="center"><b>Bold</b> <i>it</i> <a href="https://x">link</a></div>') as { children: Inline[] }).children)).toBe("Bold it link");
    expect(drawn("<details><summary>S</summary>\n\nx\n\n</details>")).toMatch(/<details class="md-details[^"]*">/);
    // A details that is never closed holds the rest; a stray close does nothing.
    expect(renderMarkdown("</details>\n\nx").blocks).toEqual([{ k: "p", children: [{ k: "text", text: "x" }] }]);
  });

  it("character references are decoded as text, never as markup", () => {
    expect(decodeEntities("a &amp; b &lt;b&gt; &#169; &#x2192; &rarr; &nosuch; &#0; &#xD800;")).toBe("a & b <b> © → → &nosuch; � �");
    const b = first("&lt;script&gt;alert(1)&lt;/script&gt; &amp;");
    expect(text((b as { children: Inline[] }).children)).toBe("<script>alert(1)</script> &");
    const html = drawn("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(html).toContain("&lt;script>");
    expect(inspect(html).tags).not.toContain("script");
    // In code, as written.
    expect(first("`&amp;`")).toEqual({ k: "p", children: [{ k: "code", text: "&amp;" }] });
  });

  it("bidirectional overrides are shown, not obeyed", () => {
    expect(text((first("abc‮def") as { children: Inline[] }).children)).toBe("abc‹U+202E›def");
  });

  it("HTML is read in pieces: quotes in attributes, a lone <, comments, unterminated tags", () => {
    expect(htmlPieces('<a title="x>y">t</a> 1 < 2 <!-- c --> <b')).toEqual([
      { t: "open", name: "a", attrs: ' title="x>y"' },
      { t: "text", text: "t" },
      { t: "close", name: "a", attrs: "" },
      { t: "text", text: " 1 < 2 " },
      { t: "comment" },
      { t: "text", text: " " },
    ]);
  });
});

/** The inline nodes of a body's first block (a paragraph or a plain item). */
function nodes(body: string): Inline[] {
  return (first(body) as { children: Inline[] }).children;
}

describe("images, never loaded, drawn by what they are next to", () => {
  // Written by hand in the shape of a deployment bot's comment: a status badge before a link
  // with the same word in a table cell, and a button that is an image in a link.
  const bot = [
    "[vc]: #abc",
    "**The latest updates on your projects**. Learn more about [the bot](https://bot.example/learn).",
    "",
    "| Project | Deployment | Actions | Updated (UTC) |",
    "| :--- | :----- | :------ | :------ |",
    "| [web](https://bot.example/web) | ![Ready](https://bot.example/status/ready.svg) [Ready](https://bot.example/web/inspect) | [Preview](https://preview.example), [Comment](https://bot.example/c) | Nov 25, 2026 1:00am |",
    "",
    "[![Request Review](https://bot.example/button/review.svg)](https://bot.example/review)",
    "",
    '<a href="https://bot.example/review"><img src="https://bot.example/button/review.svg" alt="Request Review"></a>',
  ].join("\n");

  it("an image whose alt text repeats the link beside it is nothing (the bot's status cell)", () => {
    const blocks = renderMarkdown(bot).blocks;
    const table = blocks.find((b) => b.k === "table") as Extract<Block, { k: "table" }>;
    const cell = table.rows[0][1];
    expect(cell).toEqual([
      { k: "text", text: " " },
      { k: "link", url: "https://bot.example/web/inspect", children: [{ k: "text", text: "Ready" }] },
    ]);
    expect(text(cell).trim()).toBe("Ready");
    expect(drawn(bot)).not.toContain("md-image");
  });

  it("…or the text beside it, before or after, ignoring case and whitespace", () => {
    expect(nodes("![Ready](https://x/r.svg) Ready (Inspect)")).toEqual([{ k: "text", text: " Ready (Inspect)" }]);
    expect(text(nodes("Status: ready ![READY](https://x/r.svg)"))).toBe("Status: ready ");
    expect(text(nodes("![Request  Review](https://x/r.svg) **request review**"))).toBe(" request review");
    expect(text(nodes("![Passed](https://x/p.svg) [ passed ](https://x/p)"))).toBe("  passed ");
    // Not a repeat: another word, a longer word, text further away, or none beside it.
    expect(text(nodes("![Ready](https://x/r.svg) Failed"))).toBe("‹img:Ready› Failed");
    expect(text(nodes("![Ready](https://x/r.svg) Readying"))).toBe("‹img:Ready› Readying");
    expect(text(nodes("Ready, then ![Ready](https://x/r.svg) done"))).toBe("Ready, then ‹img:Ready› done");
    expect(text(nodes("![Ready](https://x/r.svg)\nReady"))).toBe("‹img:Ready›\nReady");
  });

  it("an image that is a link's only content is that link, its alt text as its words (the bot's button)", () => {
    const blocks = renderMarkdown(bot).blocks;
    const button = { k: "link", url: "https://bot.example/review", children: [{ k: "text", text: "Request Review" }] };
    expect(blocks.slice(-2)).toEqual([
      { k: "p", children: [button] },
      { k: "p", children: [button] },
    ]);
    const html = drawn("[![Request Review](https://bot.example/b.svg)](https://bot.example/review)").replace(/<!--[^>]*-->/g, "");
    expect(html).toMatch(/role="link"[^>]*data-url="https:\/\/bot.example\/review"[^>]*>Request Review<\/span>/);
    expect(html).not.toContain("md-image");
    // Not https: its words, as text.
    expect(nodes("[![Docs](https://x/d.svg)](http://x/docs)")).toEqual([{ k: "link", url: null, children: [{ k: "text", text: "Docs" }] }]);
    // Around a <picture>, as CodeRabbit writes its buttons.
    expect(nodes('<a href="https://x/explain"><picture><source srcset="https://x/e.svg"><img src="https://x/e.svg" alt="Explain Change →"></picture></a>')).toEqual([
      { k: "link", url: "https://x/explain", children: [{ k: "text", text: "Explain Change →" }] },
    ]);
  });

  it("any other image is its alt text alone, muted", () => {
    expect(nodes("See ![a cat](https://x/cat.png) here")).toEqual([
      { k: "text", text: "See " },
      { k: "image", alt: "a cat" },
      { k: "text", text: " here" },
    ]);
    expect(nodes('Checks <img src="https://x/p.svg" alt="Passed"> 4')).toEqual([
      { k: "text", text: "Checks " },
      { k: "image", alt: "Passed" },
      { k: "text", text: " 4" },
    ]);
    // In a link with other content too.
    expect(nodes("[![logo](https://x/l.svg) Home](https://x/home)")).toEqual([
      { k: "link", url: "https://x/home", children: [{ k: "image", alt: "logo" }, { k: "text", text: " Home" }] },
    ]);
  });

  it("an image without alt text is nothing in a link with other content, else a muted “image”", () => {
    expect(nodes("[![](https://x/i.svg) Home](https://x/home)")).toEqual([{ k: "link", url: "https://x/home", children: [{ k: "text", text: " Home" }] }]);
    expect(nodes('<a href="https://x/home"><img src="https://x/i.svg" alt=""> Home</a>')).toEqual([{ k: "link", url: "https://x/home", children: [{ k: "text", text: " Home" }] }]);
    expect(first("![](https://x/y.png)")).toEqual({ k: "p", children: [{ k: "image", alt: "" }] });
    expect(first('<img src="https://x/y.png">')).toEqual({ k: "p", children: [{ k: "image", alt: "" }] });
    expect(nodes('**![](https://x/cog.svg) Run configuration**')).toEqual([
      { k: "strong", children: [{ k: "image", alt: "" }, { k: "text", text: " Run configuration" }] },
    ]);
    // A link whose only content is one: the link, drawn as “image”.
    expect(nodes("[![](https://x/y.png)](https://x/go)")).toEqual([{ k: "link", url: "https://x/go", children: [{ k: "image", alt: "" }] }]);
    const html = drawn("![](https://x/y.png)").replace(/<!--[^>]*-->/g, "");
    expect(html).toMatch(/<span class="md-image[^"]*" role="img" title="An image, not loaded" aria-label="Image, not loaded">image<\/span>/);
  });

  it("<a href> is a link by the same rules as Markdown's; links never nest", () => {
    expect(nodes('go <a href="https://x/a">there</a> now')).toEqual([
      { k: "text", text: "go " },
      { k: "link", url: "https://x/a", children: [{ k: "text", text: "there" }] },
      { k: "text", text: " now" },
    ]);
    expect(nodes('<a href="javascript:alert(1)">x</a> <a name="top">y</a>')).toEqual([
      { k: "link", url: null, children: [{ k: "text", text: "x" }] },
      { k: "text", text: " " },
      { k: "link", url: null, children: [{ k: "text", text: "y" }] },
    ]);
    // Never closed: it runs to the end of its run; another <a> closes it; a stray </a> does nothing.
    expect(nodes('<a href="https://x/a">one <a href="https://x/b">two</a> three</a>')).toEqual([
      { k: "link", url: "https://x/a", children: [{ k: "text", text: "one " }] },
      { k: "link", url: "https://x/b", children: [{ k: "text", text: "two" }] },
      { k: "text", text: " three" },
    ]);
    expect(nodes('<a href="https://x/a">[inner](https://x/b) text')).toEqual([{ k: "link", url: "https://x/a", children: [{ k: "text", text: "inner text" }] }]);
    expect(renderMarkdown('<a href="https://x/a"></a>').blocks).toEqual([]);
    const html = drawn('<a href="https://x/a" onclick="alert(1)">t</a>');
    expect(inspect(html).tags).not.toContain("a");
    expect(inspect(html).attrs).not.toContain("onclick");
  });
});

describe("only safe link addresses", () => {
  it.each([
    ["https://github.com/o/r/pull/1#discussion_r2", "https://github.com/o/r/pull/1#discussion_r2"],
    ["HTTPS://Example.COM/a", "https://example.com/a"],
    ["http://example.com", null],
    ["javascript:alert(1)", null],
    ["data:text/html,x", null],
    ["//example.com/x", null],
    ["/relative", null],
    ["#anchor", null],
    ["https://user:pw@example.com/", null],
    ["https://exa mple.com/", null],
    ["https://example.com/\u0000", null],
    [`https://example.com/${"a".repeat(3000)}`, null],
    ["", null],
  ])("%s", (href, want) => {
    expect(safeUrl(href)).toBe(want);
  });
});

describe("huge bodies are cut, never hang", () => {
  it("cuts at MAX_BODY, at a line's end when near, and says so", () => {
    const body = "word ".repeat(10_000);
    const { text: cut, cut: was } = capBody(body);
    expect(was).toBe(true);
    expect(cut.length).toBeLessThanOrEqual(MAX_BODY);
    const lines = "line\n".repeat(5000);
    expect(capBody(lines).text.endsWith("line")).toBe(true);
    expect(renderMarkdown(body).cut).toBe(true);
    expect(renderMarkdown("short").cut).toBe(false);
    // Not between the halves of a surrogate pair.
    const emoji = "a".repeat(MAX_BODY - 1) + "😀".repeat(10);
    const c = capBody(emoji).text;
    expect(c.charCodeAt(c.length - 1)).not.toBe(0xd83d);
  });

  it("a deeply nested body is flattened, and a long one is drawn quickly", () => {
    const deep = ">".repeat(500) + " deep";
    const t0 = performance.now();
    const r = renderMarkdown(deep);
    expect(performance.now() - t0).toBeLessThan(1000);
    expect(JSON.stringify(r.blocks)).toContain("deep");
    const nested = "<details><summary>x</summary>\n\n".repeat(200) + "core";
    expect(JSON.stringify(renderMarkdown(nested).blocks)).toContain("core");
    const t1 = performance.now();
    renderMarkdown("**a** _b_ `c` [d](https://e) <b>f</b>\n".repeat(3000));
    expect(performance.now() - t1).toBeLessThan(3000);
  });
});

describe("the captured CodeRabbit comment", () => {
  const view = JSON.parse(capturedView);
  const body: string = view.comments[0].body;

  it("reads as a few calm sections: the alert, then closed disclosures", () => {
    const blocks = renderMarkdown(body).blocks;
    const top = blocks.map((b) => (b.k === "details" ? `details:${text(b.summary)}` : b.k));
    expect(top).toContain("quote");
    expect(top).toContain("details:Walkthrough");
    expect(top.filter((t) => t.startsWith("details:")).length).toBeGreaterThanOrEqual(4);
    // None of its HTML comments, tags or image addresses are text.
    const all = JSON.stringify(blocks);
    expect(all).not.toContain("<!--");
    expect(all).not.toContain("<picture");
    expect(all).not.toContain("storage.googleapis.com");
  });

  it("draws with no image, script or remote address", () => {
    const html = drawn(body);
    const { tags, attrs, hrefs } = inspect(html);
    for (const tag of tags) expect(ALLOWED.has(tag), tag).toBe(true);
    for (const a of attrs) expect(a.startsWith("on")).toBe(false);
    for (const h of hrefs) expect(h).toMatch(/^https:\/\//);
    expect(html).not.toContain("storage.googleapis.com");
  });
});

describe("names that are an object's own members", () => {
  it("are not entities: they stay as written", () => {
    expect(decodeEntities("&constructor; &toString; &__proto__; &amp;")).toBe("&constructor; &toString; &__proto__; &");
  });
});
