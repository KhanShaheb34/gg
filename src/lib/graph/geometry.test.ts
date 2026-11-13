import { describe, expect, it } from "vitest";
import type { Edge } from "../types";
import {
  alignedFirstRow,
  alignSlackFor,
  FILL_DELAY_MS,
  fillFrame,
  type FillState,
  FILL_THROTTLE_MS,
  autoGraphWidth,
  GRAPH_MAX_AUTO_W,
  GRAPH_MIN_AUTO_W,
  canvasSize,
  crispOffset,
  deviceLineWidth,
  edgeShape,
  EdgeShapes,
  GRAPH_PAD,
  HEADER_H,
  LANE_COLORS,
  OVERSCAN,
  POOL_STEP,
  poolStart,
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
    expect(OVERSCAN).toBe(6);
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
    // Two straight lines leave the boundary point below: which one continues is ambiguous.
    const split = [E("full", 1, 1, 1), E("top", 1, 1, 1)];
    expect(edgeShape(row[1], undefined, row, split)).toBe(SHAPE_HALF);
    // The line changes lane in both halves: no straight partner.
    const next = [E("top", 1, 2, 1)];
    expect(edgeShape(row[1], undefined, row, next)).toBe(SHAPE_HALF);
    expect(edgeShape(next[0], row, next, undefined)).toBe(SHAPE_HALF);
    // A full edge with nothing to pair keeps its whole line.
    expect(edgeShape(E("full", 3), undefined, [E("full", 3)], undefined)).toBe(SHAPE_HALF);
  });

  it("merge into a lane that already carries a line: S-curve, and the shared line stays whole", () => {
    // Row 0: a line passes on lane 0; the merge node on lane 2 sends its second parent to lane 0.
    const row = [E("full", 0), E("top", 2, 2, 2), E("bottom", 2, 2, 2), E("bottom", 2, 0, 0)];
    const next = [E("top", 0), E("bottom", 0), E("full", 2, 2, 2)];
    expect(edgeShape(row[3], undefined, row, next)).toBe(SHAPE_S);
    expect(shapePath(row[3], SHAPE_S, 0, 26)).toEqual({ x1: 40, y1: 13, cx1: 40, cy1: 26, cx2: 8, cy2: 26, x2: 8, y2: 39 });
    // The passing line and its continuation into the parent are still drawn in full.
    expect(edgeShape(row[0], undefined, row, next)).toBe(SHAPE_HALF);
    expect(edgeShape(next[0], row, next, undefined)).toBe(SHAPE_HALF);
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
    expect(odd.width).toBe(91);
    expect(odd.cssWidth * 1.5).toBeCloseTo(91);
  });

  it("is never wider than its column, at any pixel ratio", () => {
    for (const ratio of [1, 1.25, 1.5, 1.75, 2, 2.5, 3]) {
      for (let w = 16; w <= 240; w++) {
        const size = canvasSize(w, 806, ratio);
        expect(size.cssWidth, `${w} px at ${ratio}`).toBeLessThanOrEqual(w);
        expect(size.cssWidth).toBeGreaterThan(w - 1);
      }
    }
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

describe("autoGraphWidth", () => {
  it("fits the lanes with padding, between the floor and the cap", () => {
    expect(autoGraphWidth(0, 0)).toBe(GRAPH_MIN_AUTO_W);
    expect(autoGraphWidth(4, 0)).toBe(graphWidth(4) + 2 * GRAPH_PAD);
    expect(autoGraphWidth(100, 0)).toBe(GRAPH_MAX_AUTO_W);
  });
  it("never gets narrower than the widest it has been", () => {
    expect(autoGraphWidth(0, 120)).toBe(120);
    expect(autoGraphWidth(9, 120)).toBe(graphWidth(9) + 2 * GRAPH_PAD);
  });
  it("leaves room for the Graph header's label", () => {
    expect(GRAPH_MIN_AUTO_W).toBeGreaterThanOrEqual(52);
  });
});

describe("EdgeShapes", () => {
  const HALVES: Edge["half"][] = ["full", "top", "bottom"];
  /** A pseudo-random generator, so failures repeat. */
  function rng(seed: number) {
    return () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed / 0x7fffffff;
    };
  }

  it("gives every edge the shape edgeShape gives it, for any rows", () => {
    const shapes = new EdgeShapes();
    for (let seed = 1; seed <= 300; seed++) {
      const r = rng(seed);
      const lanes = 1 + Math.floor(r() * 8);
      const count = 1 + Math.floor(r() * 12);
      const rows: (Edge[] | undefined)[] = Array.from({ length: count + 4 }, () => {
        if (r() < 0.1) return undefined; // not loaded, or the uncommitted row
        return Array.from({ length: Math.floor(r() * 6) }, () => {
          const from = Math.floor(r() * lanes);
          const half = HALVES[Math.floor(r() * 3)];
          const to = half === "full" || r() < 0.4 ? from : Math.floor(r() * lanes);
          return { from, to, color: 0, half };
        });
      });
      const first = 2;
      const last = 2 + count;
      shapes.compute(first, last, (i) => rows[i]);
      for (let i = first; i < last; i++) {
        const row = rows[i];
        if (!row) continue;
        const prev = i > first ? rows[i - 1] : undefined;
        const next = i + 1 < last ? rows[i + 1] : undefined;
        row.forEach((e, k) => expect(shapes.shape(i, k), `seed ${seed} row ${i} edge ${k}`).toBe(edgeShape(e, prev, row, next)));
      }
    }
  });

  it("handles lanes far apart, and is reused without carrying anything over", () => {
    const shapes = new EdgeShapes();
    const wide: Edge[][] = [
      [{ from: 900, to: 3, color: 0, half: "bottom" }, { from: 3, to: 3, color: 0, half: "full" }],
      [{ from: 3, to: 3, color: 0, half: "full" }],
    ];
    shapes.compute(0, 2, (i) => wide[i]);
    expect(shapes.shape(0, 0)).toBe(edgeShape(wide[0][0], undefined, wide[0], wide[1]));
    expect(shapes.shape(1, 0)).toBe(edgeShape(wide[1][0], wide[0], wide[1], undefined));
    const plain: Edge[][] = [[{ from: 3, to: 3, color: 0, half: "full" }], [{ from: 3, to: 3, color: 0, half: "full" }]];
    shapes.compute(0, 2, (i) => plain[i]);
    expect(shapes.shape(0, 0)).toBe(SHAPE_HALF);
    expect(shapes.shape(1, 0)).toBe(SHAPE_HALF);
  });
});

describe("poolStart", () => {
  it("keeps the rendered rows where they are while the window moves less than a step", () => {
    expect(poolStart(100, 100, 8, 26, 2)).toBe(100);
    expect(poolStart(100, 105, 8, 26, 2)).toBe(100);
    expect(poolStart(100, 108, 8, 26, 2)).toBe(100);
  });

  it("moves a whole step ahead when the window leaves it, either way", () => {
    expect(poolStart(100, 109, 8, 26, 2)).toBe(109);
    expect(poolStart(100, 99, 8, 26, 2)).toBe(91); // room to scroll up a step
    expect(poolStart(100, 3, 8, 26, 2)).toBe(0);
    expect(poolStart(100, 5000, 8, 26, 2)).toBe(5000);
  });

  it("moves once every 8 rows or less often at fractional pixel ratios, both ways", () => {
    for (const dpr of [1.25, 1.75, 1.5, 2]) {
      for (const dir of [1, -1]) {
        let first = dir === 1 ? 0 : 400;
        let start = alignedFirstRow(first, 26, dpr);
        let moves = 0;
        for (let k = 0; k < 200; k++) {
          first += dir;
          const next = poolStart(start, first, POOL_STEP, 26, dpr);
          if (next !== start) moves++;
          start = next;
          expect(first - start, `dpr ${dpr} dir ${dir}`).toBeLessThanOrEqual(POOL_STEP + 8);
          expect(start).toBeLessThanOrEqual(first);
        }
        expect(moves, `dpr ${dpr} dir ${dir}`).toBeLessThanOrEqual(200 / POOL_STEP);
      }
    }
  });

  it("starts on a whole device pixel, never below the window's first row", () => {
    for (const dpr of [1, 1.25, 1.5, 2]) {
      let start = 0;
      for (const first of [0, 7, 9, 30, 29, 13, 12, 400, 391, 2]) {
        start = poolStart(start, first, POOL_STEP, 26, dpr);
        expect(start).toBeLessThanOrEqual(first);
        expect(first - start).toBeLessThanOrEqual(POOL_STEP + 8);
        expect(alignedFirstRow(start, 26, dpr)).toBe(start);
      }
    }
  });
});

describe("alignSlackFor", () => {
  it("is how many rows a whole-device-pixel start may lie above a row", () => {
    expect(alignSlackFor(1)).toBe(0);
    expect(alignSlackFor(2)).toBe(0);
    expect(alignSlackFor(1.5)).toBe(0);
    expect(alignSlackFor(1.25)).toBe(1);
    expect(alignSlackFor(1.75)).toBe(1);
  });
});

describe("fillFrame (rows are filled at most every FILL_THROTTLE_MS during fast jumps)", () => {
  const idle: FillState = { fillAt: 0, lastJumpFill: -Infinity };

  it("fills the first frame of a jump, then at most once per throttle interval while it goes on", () => {
    expect(FILL_DELAY_MS).toBe(50);
    expect(FILL_THROTTLE_MS).toBe(120);
    let s = idle;
    const fills: number[] = [];
    // A scrollbar drag: two screens a frame, every 16 ms, for about a second.
    for (let now = 1000; now <= 2000; now += 16) {
      const d = fillFrame(1600, 800, now, s);
      s = d.state;
      if (d.fill) fills.push(now);
    }
    expect(fills[0]).toBe(1000);
    for (let k = 1; k < fills.length; k++) expect(fills[k] - fills[k - 1]).toBeGreaterThanOrEqual(120);
    // Never blank for much longer than the interval plus a frame.
    for (let k = 1; k < fills.length; k++) expect(fills[k] - fills[k - 1]).toBeLessThan(120 + 16);
    expect(fills.length).toBeGreaterThanOrEqual(7);
    // And 50 ms after the last fast frame, everything fills.
    expect(s.fillAt).toBe(1992 + 50);
  });

  it("holds slow frames until FILL_DELAY_MS after the last fast one, then fills them", () => {
    let { state } = fillFrame(-5000, 800, 2000, idle);
    expect(state.fillAt).toBe(2050);
    const held = fillFrame(13, 800, 2020, state);
    expect(held.fill).toBe(false);
    state = held.state;
    expect(fillFrame(13, 800, 2050, state).fill).toBe(true);
  });

  it("fills every frame of moves up to a viewport: smooth scrolling and page keys", () => {
    for (const moved of [13, 800, -774]) {
      const d = fillFrame(moved, 800, 1000, idle);
      expect(d.fill).toBe(true);
      expect(d.state).toEqual(idle);
    }
  });
});
