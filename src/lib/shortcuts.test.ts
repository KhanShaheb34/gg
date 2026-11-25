import { describe, expect, it } from "vitest";
import { repeatable, SHORTCUT_GROUPS, SHORTCUT_KINDS, SHORTCUT_TABLE, shortcutFor, shortcutSheet } from "./shortcuts";

const key = (k: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "shiftKey" | "altKey", boolean>> = {}) => ({
  key: k,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

describe("shortcutFor on macOS (Cmd)", () => {
  const mac = (e: ReturnType<typeof key>) => shortcutFor(e, true);

  it("maps Cmd+O, Cmd+W and Cmd+1..9", () => {
    expect(mac(key("o", { metaKey: true }))).toEqual({ kind: "open" });
    expect(mac(key("w", { metaKey: true }))).toEqual({ kind: "close" });
    expect(mac(key("1", { metaKey: true }))).toEqual({ kind: "jump", index: 0 });
    expect(mac(key("8", { metaKey: true }))).toEqual({ kind: "jump", index: 7 });
  });

  it("Cmd+9 is the last tab, however many there are", () => {
    expect(mac(key("9", { metaKey: true }))).toEqual({ kind: "jump", index: -1 });
  });

  it("ignores keys typed into an input method's composition", () => {
    expect(shortcutFor({ ...key("w", { metaKey: true }), isComposing: true }, true)).toBeNull();
    expect(shortcutFor({ ...key("Tab", { ctrlKey: true }), isComposing: true }, true)).toBeNull();
  });

  it("lets only tab cycling repeat while a key is held", () => {
    expect(repeatable({ kind: "cycle", delta: 1 })).toBe(true);
    for (const kind of ["open", "close", "search", "fetch", "refresh"] as const) {
      expect(repeatable({ kind })).toBe(false);
    }
    expect(repeatable({ kind: "jump", index: 0 })).toBe(false);
  });

  it("cycles tabs with Ctrl+Tab and Ctrl+Shift+Tab", () => {
    expect(mac(key("Tab", { ctrlKey: true }))).toEqual({ kind: "cycle", delta: 1 });
    expect(mac(key("Tab", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "cycle", delta: -1 });
  });

  it("ignores Ctrl+O, plain keys and extra modifiers", () => {
    expect(mac(key("o", { ctrlKey: true }))).toBeNull();
    expect(mac(key("o"))).toBeNull();
    expect(mac(key("o", { metaKey: true, altKey: true }))).toBeNull();
    expect(mac(key("w", { metaKey: true, shiftKey: true }))).toBeNull();
    expect(mac(key("0", { metaKey: true }))).toBeNull();
  });

  it("accepts an upper-case key (caps lock)", () => {
    expect(mac(key("O", { metaKey: true }))).toEqual({ kind: "open" });
  });

  it("maps Cmd+F to search, Cmd+Shift+F to fetch and Cmd+R to refresh", () => {
    expect(mac(key("f", { metaKey: true }))).toEqual({ kind: "search" });
    expect(mac(key("F", { metaKey: true, shiftKey: true }))).toEqual({ kind: "fetch" });
    expect(mac(key("f", { metaKey: true, shiftKey: true }))).toEqual({ kind: "fetch" });
    expect(mac(key("r", { metaKey: true }))).toEqual({ kind: "refresh" });
    expect(mac(key("r", { metaKey: true, shiftKey: true }))).toBeNull();
    expect(mac(key("o", { metaKey: true, shiftKey: true }))).toBeNull();
    expect(mac(key("f", { ctrlKey: true }))).toBeNull();
  });
});

describe("shortcutFor elsewhere (Ctrl)", () => {
  const pc = (e: ReturnType<typeof key>) => shortcutFor(e, false);

  it("uses Ctrl as the command modifier", () => {
    expect(pc(key("o", { ctrlKey: true }))).toEqual({ kind: "open" });
    expect(pc(key("w", { ctrlKey: true }))).toEqual({ kind: "close" });
    expect(pc(key("3", { ctrlKey: true }))).toEqual({ kind: "jump", index: 2 });
    expect(pc(key("Tab", { ctrlKey: true }))).toEqual({ kind: "cycle", delta: 1 });
    expect(pc(key("o", { metaKey: true }))).toBeNull();
    expect(pc(key("f", { ctrlKey: true }))).toEqual({ kind: "search" });
    expect(pc(key("F", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "fetch" });
    expect(pc(key("r", { ctrlKey: true }))).toEqual({ kind: "refresh" });
    expect(pc(key("r", { metaKey: true }))).toBeNull();
  });
});

describe("region and panel shortcuts", () => {
  it("F6 moves to the next region (the graph, the newest notice, the side panel), Shift+F6 back, on every platform", () => {
    expect(shortcutFor(key("F6"), true)).toEqual({ kind: "regions", delta: 1 });
    expect(shortcutFor(key("F6"), false)).toEqual({ kind: "regions", delta: 1 });
    expect(shortcutFor(key("F6", { shiftKey: true }), true)).toEqual({ kind: "regions", delta: -1 });
    expect(shortcutFor(key("F6", { metaKey: true }), true)).toBeNull();
    expect(repeatable({ kind: "regions", delta: 1 })).toBe(false);
  });

  it("Cmd+B shows or hides the side panel (Ctrl+B elsewhere)", () => {
    expect(shortcutFor(key("b", { metaKey: true }), true)).toEqual({ kind: "sidePanel" });
    expect(shortcutFor(key("B", { ctrlKey: true }), false)).toEqual({ kind: "sidePanel" });
    expect(shortcutFor(key("b", { ctrlKey: true }), true)).toBeNull();
    expect(shortcutFor(key("b", { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(repeatable({ kind: "sidePanel" })).toBe(false);
  });

  it("Cmd+K opens the commit panel (Ctrl+K elsewhere), once per press", () => {
    expect(shortcutFor(key("k", { metaKey: true }), true)).toEqual({ kind: "commit" });
    expect(shortcutFor(key("K", { ctrlKey: true }), false)).toEqual({ kind: "commit" });
    expect(shortcutFor(key("k", { ctrlKey: true }), true)).toBeNull();
    expect(shortcutFor(key("k", { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(repeatable({ kind: "commit" })).toBe(false);
  });

  it("Cmd+P opens the branch picker (Ctrl+P elsewhere), also while typing, once per press", () => {
    expect(shortcutFor(key("p", { metaKey: true }), true)).toEqual({ kind: "branches" });
    expect(shortcutFor(key("P", { ctrlKey: true }), false)).toEqual({ kind: "branches" });
    // In a text field too: the window's handler takes it, so the webview never prints.
    expect(shortcutFor(key("p", { metaKey: true }), true, true)).toEqual({ kind: "branches" });
    expect(shortcutFor(key("p", { ctrlKey: true }), false, true)).toEqual({ kind: "branches" });
    expect(shortcutFor(key("p", { ctrlKey: true }), true)).toBeNull();
    expect(shortcutFor(key("p", { metaKey: true }), false)).toBeNull();
    expect(shortcutFor(key("p", { metaKey: true, shiftKey: true }), true)).toEqual({ kind: "pullRequest" });
    expect(shortcutFor(key("p"), true)).toBeNull();
    expect(repeatable({ kind: "branches" })).toBe(false);
  });

  it("Cmd+Shift+P shows the Pull request tab (Ctrl+Shift+P elsewhere), also while typing; Cmd+P stays the branch picker", () => {
    expect(shortcutFor(key("P", { metaKey: true, shiftKey: true }), true)).toEqual({ kind: "pullRequest" });
    expect(shortcutFor(key("p", { metaKey: true, shiftKey: true }), true)).toEqual({ kind: "pullRequest" });
    expect(shortcutFor(key("P", { ctrlKey: true, shiftKey: true }), false)).toEqual({ kind: "pullRequest" });
    expect(shortcutFor(key("P", { metaKey: true, shiftKey: true }), true, true)).toEqual({ kind: "pullRequest" });
    expect(shortcutFor(key("p", { metaKey: true }), true)).toEqual({ kind: "branches" });
    expect(shortcutFor(key("P", { ctrlKey: true, shiftKey: true }), true)).toBeNull();
    expect(shortcutFor(key("P", { metaKey: true, shiftKey: true }), false)).toBeNull();
    expect(shortcutFor(key("P", { metaKey: true, shiftKey: true, altKey: true }), true)).toBeNull();
    expect(repeatable({ kind: "pullRequest" })).toBe(false);
  });

  it("Cmd+Shift+L shows or hides the command log (Ctrl+Shift+L elsewhere)", () => {
    expect(shortcutFor(key("L", { metaKey: true, shiftKey: true }), true)).toEqual({ kind: "log" });
    expect(shortcutFor(key("l", { ctrlKey: true, shiftKey: true }), false)).toEqual({ kind: "log" });
    expect(shortcutFor(key("l", { metaKey: true }), true)).toBeNull();
    expect(shortcutFor(key("l", { ctrlKey: true, shiftKey: true }), true)).toBeNull();
    expect(repeatable({ kind: "log" })).toBe(false);
  });
});

describe("settings, worktree and help shortcuts", () => {
  it("Cmd+, opens the settings (Ctrl+, elsewhere)", () => {
    expect(shortcutFor(key(",", { metaKey: true }), true)).toEqual({ kind: "settings" });
    expect(shortcutFor(key(",", { ctrlKey: true }), false)).toEqual({ kind: "settings" });
    expect(shortcutFor(key(",", { ctrlKey: true }), true)).toBeNull();
    expect(repeatable({ kind: "settings" })).toBe(false);
  });

  it("? shows the keyboard shortcuts, unless a text field has the focus", () => {
    expect(shortcutFor(key("?", { shiftKey: true }), true)).toEqual({ kind: "help" });
    expect(shortcutFor(key("?", { shiftKey: true }), false)).toEqual({ kind: "help" });
    expect(shortcutFor(key("?", { shiftKey: true }), true, true)).toBeNull();
    expect(shortcutFor(key("?", { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(shortcutFor(key("/"), true)).toBeNull();
  });

  it("F1 (Windows, Linux) and Cmd+/ (macOS) show them too, even while typing: ? needs AltGr on some layouts", () => {
    expect(shortcutFor(key("F1"), false)).toEqual({ kind: "help" });
    expect(shortcutFor(key("F1"), false, true)).toEqual({ kind: "help" });
    expect(shortcutFor(key("F1", { ctrlKey: true }), false)).toBeNull();
    expect(shortcutFor(key("/", { metaKey: true }), true)).toEqual({ kind: "help" });
    // A layout where / is Shift+7.
    expect(shortcutFor(key("/", { metaKey: true, shiftKey: true }), true)).toEqual({ kind: "help" });
    expect(shortcutFor(key("/", { metaKey: true }), true, true)).toEqual({ kind: "help" });
    expect(shortcutFor(key("/", { ctrlKey: true }), false)).toBeNull();
  });

  it("switches worktree with Cmd+Option+1..9 on macOS (Cmd+Shift+3/4/5 are the system's screenshots), Ctrl+Shift+1..9 elsewhere; never a tab", () => {
    const at = (n: number, mods: Parameters<typeof key>[1]) => ({ ...key(String(n), mods), code: `Digit${n}` });
    // Option and Shift change the character, not the key's position.
    expect(shortcutFor({ ...at(3, { metaKey: true, altKey: true }), key: "£" }, true)).toEqual({ kind: "worktree", index: 2 });
    expect(shortcutFor(at(1, { metaKey: true, altKey: true }), true)).toEqual({ kind: "worktree", index: 0 });
    expect(shortcutFor({ ...at(2, { ctrlKey: true, shiftKey: true }), key: "@" }, false)).toEqual({ kind: "worktree", index: 1 });
    expect(shortcutFor(at(9, { ctrlKey: true, shiftKey: true }), false)).toEqual({ kind: "worktree", index: 8 });
    // Cmd+Shift+digit on macOS is not taken: the system has some of them.
    expect(shortcutFor({ ...at(1, { metaKey: true, shiftKey: true }), key: "!" }, true)).toBeNull();
    // Without the second modifier it is a tab, as before.
    expect(shortcutFor(at(2, { metaKey: true }), true)).toEqual({ kind: "jump", index: 1 });
    expect(shortcutFor(at(2, { ctrlKey: true }), false)).toEqual({ kind: "jump", index: 1 });
    expect(shortcutFor(at(0, { metaKey: true, altKey: true }), true)).toBeNull();
    expect(repeatable({ kind: "worktree", index: 0 })).toBe(false);
  });
});

/** The key press the sheet's `tokens` stand for, on `isMac` ("1…8" as its first key). */
function pressOf(tokens: string[], isMac: boolean) {
  const mods = { metaKey: false, ctrlKey: false, shiftKey: false, altKey: false };
  let k = "";
  for (const t of tokens) {
    if (t === "Mod") mods[isMac ? "metaKey" : "ctrlKey"] = true;
    else if (t === "Ctrl") mods.ctrlKey = true;
    else if (t === "Shift") mods.shiftKey = true;
    else if (t === "Alt") mods.altKey = true;
    else k = t.replace(/….*$/, "");
  }
  const digit = /^[0-9]$/.test(k) ? { code: `Digit${k}` } : {};
  return { key: k.length === 1 ? k.toLowerCase() : k, ...mods, ...digit };
}

describe("the table", () => {
  it("has a row for every shortcut the window handles, and each row's keys do what it says (the sheet cannot drift)", () => {
    const kinds = new Set(SHORTCUT_TABLE.filter((r) => r.match).map((r) => r.kind));
    expect([...kinds].sort()).toEqual([...SHORTCUT_KINDS].sort());
    for (const isMac of [true, false]) {
      for (const row of SHORTCUT_TABLE.filter((r) => r.match)) {
        for (const tokens of (isMac && row.macKeys) || row.keys) {
          expect(shortcutFor(pressOf(tokens, isMac), isMac)?.kind, `${row.does} ${tokens.join("+")}`).toBe(row.kind);
        }
      }
    }
  });

  it("no two rows of the window's own take the same keys", () => {
    for (const isMac of [true, false]) {
      const seen = new Map<string, string>();
      for (const row of SHORTCUT_TABLE.filter((r) => r.match)) {
        for (const tokens of (isMac && row.macKeys) || row.keys) {
          const id = tokens.join("+");
          expect(seen.get(id), `${id}: ${row.does}`).toBeUndefined();
          seen.set(id, row.does);
        }
      }
    }
  });

  it("the sheet lists every row, by group, in the platform's own key names", () => {
    const mac = shortcutSheet(true);
    expect(mac.map((g) => g.group)).toEqual([...SHORTCUT_GROUPS]);
    expect(mac.flatMap((g) => g.rows)).toHaveLength(SHORTCUT_TABLE.length);
    const settings = mac.flatMap((g) => g.rows).find((r) => r.does === "Settings")!;
    expect(settings.keys).toEqual([["⌘", ","]]);
    const pc = shortcutSheet(false).flatMap((g) => g.rows);
    expect(pc.find((r) => r.does === "Settings")!.keys).toEqual([["Ctrl", ","]]);
    expect(pc.find((r) => r.does.startsWith("Make worktree"))!.keys).toEqual([["Ctrl", "Shift", "1…9"]]);
    expect(mac.flatMap((g) => g.rows).find((r) => r.does.startsWith("Make worktree"))!.keys).toEqual([["⌘", "⌥", "1…9"]]);
  });
});
