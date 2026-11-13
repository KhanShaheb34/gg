import { describe, expect, it } from "vitest";
import { repeatable, shortcutFor } from "./shortcuts";

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
