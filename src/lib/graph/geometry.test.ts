import { describe, expect, it } from "vitest";
import type { Edge } from "../types";
import {
  alignedFirstRow,
  canvasSize,
  crispOffset,
  deviceLineWidth,
  edgeShape,
  GRAPH_PAD,
  HEADER_H,
  LANE_COLORS,
  OVERSCAN,
  PREFETCH,
  ROW_H,
  SHAPE_HALF,
  SHAPE_LOWER,
  SHAPE_S,
  SHAPE_SKIP,
  SHAPE_UPPER,
  shapePath,
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

describe("layout constants (single source)", () => {
  it("defines the graph's sizes in one place", () => {
    expect(ROW_H).toBe(26);
    expect(HEADER_H).toBe(28);
    expect(GRAPH_PAD).toBe(6);
    expect(LANE_COLORS).toBe(10);
    expect(OVERSCAN).toBe(10);
    expect(PREFETCH).toBe(400);
  });
});

// R17: a lane change split across two rows (a `bottom` half in one, a straight continuation
// in the next, or a straight lower half followed by a `top` change) is drawn as one S-curve
// between the two rows' centres.
const E = (half: Edge["half"], from: number, to = from, color = 0): Edge => ({ half, from, to, color });

describe("edgeShape / shapePath (R17)", () => {
  it("merge-in: a bottom lane change continued straight below becomes one full-row S-curve", () => {
    const row = [E("top", 0), E("bottom", 0), E("bottom", 0, 1, 1)];
    const next = [E("top", 0), E("bottom", 0), E("full", 1, 1, 1)];
    expect(edgeShape(row[2], undefined, row, next)).toBe(SHAPE_S);
    expect(shapePath(row[2], SHAPE_S, 0, 26)).toEqual({ x1: 8, y1: 13, cx1: 8, cy1: 26, cx2: 24, cy2: 26, x2: 24, y2: 39 });
    // The straight first-parent line keeps its half-row drawing.
    expect(edgeShape(row[1], undefined, row, next)).toBe(SHAPE_HALF);
    // The continuation below only draws its lower half; the S-curve covers its upper half.
    expect(edgeShape(next[2], row, next, undefined)).toBe(SHAPE_LOWER);
    expect(shapePath(next[2], SHAPE_LOWER, 26, 26)).toEqual({ x1: 24, y1: 39, x2: 24, y2: 52 });
  });

  it("merge-in into a node: the straight top half below is left out", () => {
    const row = [E("bottom", 0), E("bottom", 0, 1, 1)];
    const next = [E("top", 1, 1, 1), E("bottom", 1, 1, 1)];
    expect(edgeShape(row[1], undefined, row, next)).toBe(SHAPE_S);
    expect(edgeShape(next[0], row, next, undefined)).toBe(SHAPE_SKIP);
    expect(shapePath(next[0], SHAPE_SKIP, 26, 26)).toBeNull();
  });

  it("branch-off: a top lane change below a straight line becomes one S-curve from the row above", () => {
    const prev = [E("full", 0), E("full", 1, 1, 1)];
    const row = [E("top", 0), E("top", 1, 0, 1), E("bottom", 0)];
    expect(edgeShape(row[1], prev, row, undefined)).toBe(SHAPE_S);
    expect(shapePath(row[1], SHAPE_S, 26, 26)).toEqual({ x1: 24, y1: 13, cx1: 24, cy1: 26, cx2: 8, cy2: 26, x2: 8, y2: 39 });
    expect(edgeShape(prev[1], undefined, prev, row)).toBe(SHAPE_UPPER);
    expect(shapePath(prev[1], SHAPE_UPPER, 0, 26)).toEqual({ x1: 24, y1: 0, x2: 24, y2: 13 });
    expect(edgeShape(prev[0], undefined, prev, row)).toBe(SHAPE_HALF);
  });

  it("falls back to half-row drawing when the pairing is not unambiguous", () => {
    const row = [E("bottom", 0), E("bottom", 0, 1, 1)];
    // Next row not available (not loaded, or past the canvas edge).
    expect(edgeShape(row[1], undefined, row, undefined)).toBe(SHAPE_HALF);
    // Two lines meet at lane 1 at the row boundary: which one continues is ambiguous.
    const converging = [E("full", 1, 1, 2), E("bottom", 0, 1, 1)];
    expect(edgeShape(converging[1], undefined, converging, [E("full", 1, 1, 1)])).toBe(SHAPE_HALF);
    // The line changes lane in both halves: no straight partner.
    const next = [E("top", 1, 2, 1)];
    expect(edgeShape(row[1], undefined, row, next)).toBe(SHAPE_HALF);
    expect(edgeShape(next[0], row, next, undefined)).toBe(SHAPE_HALF);
    // A full edge with nothing to pair keeps its whole line.
    expect(edgeShape(E("full", 3), undefined, [E("full", 3)], undefined)).toBe(SHAPE_HALF);
  });

  it("pairs two lane changes in the same row independently", () => {
    const prev = [E("full", 0), E("full", 2, 2, 2)];
    const row = [E("top", 0), E("top", 2, 0, 2), E("bottom", 0), E("bottom", 0, 1, 1)];
    const next = [E("full", 0), E("full", 1, 1, 1)];
    expect(edgeShape(row[1], prev, row, next)).toBe(SHAPE_S);
    expect(edgeShape(row[3], prev, row, next)).toBe(SHAPE_S);
    expect(edgeShape(row[0], prev, row, next)).toBe(SHAPE_HALF);
    expect(edgeShape(row[2], prev, row, next)).toBe(SHAPE_HALF);
    expect(edgeShape(prev[1], undefined, prev, row)).toBe(SHAPE_UPPER);
    expect(edgeShape(next[1], row, next, undefined)).toBe(SHAPE_LOWER);
  });

  it("drops a full edge whose both halves are covered by S-curves", () => {
    const above = [E("bottom", 0), E("bottom", 0, 1, 1)];
    const middle = [E("full", 0), E("full", 1, 1, 1)];
    const below = [E("top", 0), E("top", 1, 0, 1)];
    expect(edgeShape(middle[1], above, middle, below)).toBe(SHAPE_SKIP);
  });
});

describe("canvas sizing for the device pixel ratio", () => {
  it("rounds the backing store to whole device pixels and sizes the element to match", () => {
    expect(canvasSize(60, 806, 1)).toEqual({ width: 60, height: 806, cssWidth: 60, cssHeight: 806 });
    expect(canvasSize(60, 806, 2)).toEqual({ width: 120, height: 1612, cssWidth: 60, cssHeight: 806 });
    expect(canvasSize(60, 806, 1.5)).toEqual({ width: 90, height: 1209, cssWidth: 60, cssHeight: 806 });
    const odd = canvasSize(61, 806, 1.5);
    expect(odd.width).toBe(92);
    expect(odd.cssWidth * 1.5).toBeCloseTo(92);
  });

  it("gives lines a whole number of device pixels, centred on pixel boundaries or centres", () => {
    expect(deviceLineWidth(2, 1)).toBe(2);
    expect(deviceLineWidth(2, 2)).toBe(2);
    expect(deviceLineWidth(2, 1.5)).toBe(2); // 3 device px
    expect(deviceLineWidth(2, 1.25) * 1.25).toBeCloseTo(3);
    expect(crispOffset(14, 1, 2)).toBe(0);
    expect(crispOffset(14, 2, 2)).toBe(0);
    // 3 device px wide: centre on a pixel centre (21.5), not on the boundary at 21.
    expect((14 + crispOffset(14, 1.5, 2)) * 1.5).toBeCloseTo(21.5);
    expect(crispOffset(15, 1, 1)).toBe(0.5);
  });

  it("starts the canvas on a row whose top is a whole device pixel", () => {
    expect(alignedFirstRow(7, 26, 1)).toBe(7);
    expect(alignedFirstRow(7, 26, 2)).toBe(7);
    expect(alignedFirstRow(7, 26, 1.5)).toBe(7); // 273 device px
    expect(alignedFirstRow(7, 26, 1.25)).toBe(6); // 7 rows = 227.5 device px
    expect(alignedFirstRow(0, 26, 1.25)).toBe(0);
  });
});
