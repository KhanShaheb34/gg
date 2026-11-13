import { describe, expect, it } from "vitest";
import {
  computeAnchor,
  displayedRows,
  edgePath,
  graphWidth,
  laneX,
  LANE_W,
  MAX_SCROLL_PX,
  NODE_R,
  restoreScrollTop,
  rowWindow,
} from "./geometry";

describe("lanes", () => {
  it("places lanes on a 16 px grid, centred", () => {
    expect(LANE_W).toBe(16);
    expect(NODE_R).toBe(4);
    expect(laneX(0)).toBe(8);
    expect(laneX(2)).toBe(40);
    expect(graphWidth(1)).toBe(32);
    expect(graphWidth(0)).toBe(16);
  });
});

describe("edgePath", () => {
  it("draws a full edge as a vertical line through the row", () => {
    expect(edgePath({ from: 1, to: 1, color: 0, half: "full" }, 100, 26)).toEqual({ x1: 24, y1: 100, x2: 24, y2: 126 });
  });

  it("curves a top edge from the row's top into the node at its middle", () => {
    const p = edgePath({ from: 1, to: 0, color: 0, half: "top" }, 0, 26);
    expect(p).toMatchObject({ x1: 24, y1: 0, x2: 8, y2: 13 });
    // Leaves and arrives vertically: control points straight below/above the ends.
    expect(p).toMatchObject({ cx1: 24, cx2: 8 });
    if (!("cx1" in p)) throw new Error("expected a curve");
    expect(p.cy1).toBeCloseTo(0 + 13 * 0.8);
    expect(p.cy2).toBeCloseTo(13 - 13 * 0.8);
  });

  it("curves a bottom edge from the node down to the row's bottom", () => {
    const p = edgePath({ from: 0, to: 1, color: 0, half: "bottom" }, 0, 26);
    expect(p).toMatchObject({ x1: 8, y1: 13, x2: 24, y2: 26, cx1: 8, cx2: 24 });
  });

  it("keeps a half edge on one lane straight", () => {
    expect(edgePath({ from: 2, to: 2, color: 0, half: "top" }, 26, 26)).toEqual({ x1: 40, y1: 26, x2: 40, y2: 39 });
    expect(edgePath({ from: 2, to: 2, color: 0, half: "bottom" }, 26, 26)).toEqual({ x1: 40, y1: 39, x2: 40, y2: 52 });
  });
});

describe("scroll anchoring", () => {
  it("anchors on the first fully visible row and its offset from the top", () => {
    expect(computeAnchor(0, 26)).toEqual({ index: 0, offset: 0 });
    expect(computeAnchor(26 * 40, 26)).toEqual({ index: 40, offset: 0 });
    expect(computeAnchor(26 * 40 + 5, 26)).toEqual({ index: 41, offset: 21 });
  });

  it("restores the scroll position that puts a row at the same offset", () => {
    const a = computeAnchor(26 * 40 + 5, 26);
    expect(restoreScrollTop(a.index, a.offset, 26)).toBe(26 * 40 + 5);
    // The anchored commit moved down three rows (three new commits above it).
    expect(restoreScrollTop(a.index + 3, a.offset, 26)).toBe(26 * 43 + 5);
    // Never above the top.
    expect(restoreScrollTop(0, 21, 26)).toBe(0);
  });
});

describe("displayedRows", () => {
  it("shows every row while the content stays under the height cap", () => {
    expect(displayedRows(3000, 26)).toBe(3000);
    expect(displayedRows(0, 26)).toBe(0);
  });

  it("caps the rows so the scroll height stays under 15,000,000 px", () => {
    expect(MAX_SCROLL_PX).toBe(15_000_000);
    const cap = Math.floor(15_000_000 / 26);
    expect(displayedRows(2_000_000, 26)).toBe(cap);
    expect(cap * 26).toBeLessThanOrEqual(15_000_000);
  });
});

describe("rowWindow", () => {
  it("covers the visible rows plus overscan, clipped to the rows there are", () => {
    // 260 px viewport = 10 rows; scrolled to row 50.
    expect(rowWindow(50 * 26, 260, 26, 1000, 10)).toEqual({ first: 40, last: 70, visibleFirst: 50, visibleLast: 60 });
    expect(rowWindow(0, 260, 26, 1000, 10)).toEqual({ first: 0, last: 20, visibleFirst: 0, visibleLast: 10 });
    expect(rowWindow(995 * 26, 260, 26, 1000, 10)).toEqual({ first: 985, last: 1000, visibleFirst: 995, visibleLast: 1000 });
    expect(rowWindow(0, 260, 26, 0, 10)).toEqual({ first: 0, last: 0, visibleFirst: 0, visibleLast: 0 });
  });

  it("counts a partly visible row as visible", () => {
    expect(rowWindow(13, 260, 26, 1000, 0)).toEqual({ first: 0, last: 11, visibleFirst: 0, visibleLast: 11 });
  });
});
