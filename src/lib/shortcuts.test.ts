import { describe, expect, it } from "vitest";
import { shortcutFor } from "./shortcuts";

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
    expect(mac(key("9", { metaKey: true }))).toEqual({ kind: "jump", index: 8 });
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
});

describe("shortcutFor elsewhere (Ctrl)", () => {
  const pc = (e: ReturnType<typeof key>) => shortcutFor(e, false);

  it("uses Ctrl as the command modifier", () => {
    expect(pc(key("o", { ctrlKey: true }))).toEqual({ kind: "open" });
    expect(pc(key("w", { ctrlKey: true }))).toEqual({ kind: "close" });
    expect(pc(key("3", { ctrlKey: true }))).toEqual({ kind: "jump", index: 2 });
    expect(pc(key("Tab", { ctrlKey: true }))).toEqual({ kind: "cycle", delta: 1 });
    expect(pc(key("o", { metaKey: true }))).toBeNull();
  });
});
