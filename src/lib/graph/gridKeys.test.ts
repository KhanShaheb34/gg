import { describe, expect, it } from "vitest";
import { gridKey } from "./gridKeys";

const at = (current: number | null, scrollTop = 0) => ({ count: 100, viewportH: 260, scrollTop, current, rowH: 26 });

describe("gridKey", () => {
  it("moves the selection by one with the arrows, from the first visible row when there is none", () => {
    expect(gridKey("ArrowDown", at(5))).toEqual({ select: 6 });
    expect(gridKey("ArrowUp", at(5))).toEqual({ select: 4 });
    expect(gridKey("ArrowDown", at(null, 26 * 10 + 3))).toEqual({ select: 11 });
    expect(gridKey("ArrowUp", at(0))).toEqual({ select: 0 });
    expect(gridKey("ArrowDown", at(99))).toEqual({ select: 99 });
  });

  it("pages by a screen less a row, scrolling with the selection", () => {
    expect(gridKey("PageDown", at(5))).toEqual({ select: 14, scrollTop: 26 * 9 });
    expect(gridKey("PageUp", at(50, 26 * 50))).toEqual({ select: 41, scrollTop: 26 * 41 });
    expect(gridKey("PageDown", at(95, 26 * 90))).toEqual({ select: 99, scrollTop: 26 * 100 - 260 });
  });

  it("goes to the ends with Home and End", () => {
    expect(gridKey("Home", at(40))).toEqual({ select: 0 });
    expect(gridKey("End", at(40))).toEqual({ select: 99 });
  });

  it("clears the selection with Escape, and ignores other keys or an empty grid", () => {
    expect(gridKey("Escape", at(3))).toEqual({ select: null });
    expect(gridKey("Escape", at(null))).toBeNull();
    expect(gridKey("a", at(3))).toBeNull();
    expect(gridKey("ArrowDown", { ...at(3), count: 0 })).toBeNull();
  });
});
