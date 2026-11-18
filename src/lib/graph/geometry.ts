// Pure geometry of the graph: where lanes, edges and rows sit, which rows a scroll position
// shows, and how to keep a row in place when the rows change under it.

import type { Edge } from "../types";

/** Height of one row, px. The graph sets its own `--row-h` from this. */
export const ROW_H = 26;
/** Height of the graph's column header, px. */
export const HEADER_H = 28;
/** Width of one lane, px. */
export const LANE_W = 16;
/** Radius of a commit dot, px. */
export const NODE_R = 4;
/** Space left of lane 0 (and right of the last lane, by default), px. */
export const GRAPH_PAD = 6;
/** Narrowest the Graph column sizes itself: enough to show its "Graph" header, px. */
export const GRAPH_MIN_AUTO_W = 52;
/** Widest the Graph column sizes itself (wider only when dragged), px. */
export const GRAPH_MAX_AUTO_W = 240;
/** Number of lane colours (`--lane-0` … `--lane-9`). */
export const LANE_COLORS = 10;
/**
 * Rows rendered above and below the viewport. With POOL_STEP rows more in the container,
 * the rendered rows are as many as with 10 and no step, so the layer costs no more memory.
 */
export const OVERSCAN = 6;
/** Rows loaded ahead in the direction of scrolling. */
export const PREFETCH = 400;
/** Most px of rows the scroll content may hold; browsers break somewhere above 16M. */
export const MAX_SCROLL_PX = 15_000_000;
/** How far a half-row curve's control points sit from its ends, as a share of half a row. */
const CURVE = 0.8;
/** How far a full-row S-curve's control points sit from its ends, as a share of the row. */
const S_CURVE = 0.5;

/** Centre x of `lane`. */
export function laneX(lane: number): number {
  return LANE_W * lane + LANE_W / 2;
}

/** Width that holds lanes 0..maxLane. */
export function graphWidth(maxLane: number): number {
  return LANE_W * (maxLane + 1);
}

/**
 * Width the Graph column gives itself: its lanes (0..maxLane) with padding, between
 * GRAPH_MIN_AUTO_W and GRAPH_MAX_AUTO_W, and never narrower than `widest`, the widest it has
 * been, so the columns to its right do not move when a filter shows fewer lanes.
 */
export function autoGraphWidth(maxLane: number, widest: number): number {
  const fit = Math.min(graphWidth(maxLane) + 2 * GRAPH_PAD, GRAPH_MAX_AUTO_W);
  return Math.max(GRAPH_MIN_AUTO_W, fit, widest);
}

export type Line = { x1: number; y1: number; x2: number; y2: number };
export type Curve = Line & { cx1: number; cy1: number; cx2: number; cy2: number };

/** The canvas calls `traceEdge` uses; a CanvasRenderingContext2D is one. */
export type PathSink = {
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  bezierCurveTo(cx1: number, cy1: number, cx2: number, cy2: number, x: number, y: number): void;
};

/**
 * Adds `edge` of the row whose top is at `rowTop` to `path`, without allocating: `full` runs
 * through the row on lane `from`; `top` joins lane `from` at the row's top to lane `to` at its
 * middle; `bottom` joins lane `from` at the middle to lane `to` at the bottom. A lane change is
 * a cubic curve that leaves and arrives vertically.
 */
export function traceEdge(path: PathSink, edge: Edge, rowTop: number, rowH: number): void {
  const half = rowH / 2;
  const x1 = laneX(edge.from);
  const x2 = edge.half === "full" ? x1 : laneX(edge.to);
  const y1 = edge.half === "bottom" ? rowTop + half : rowTop;
  const y2 = edge.half === "top" ? rowTop + half : rowTop + rowH;
  path.moveTo(x1, y1);
  if (x1 === x2) {
    path.lineTo(x2, y2);
  } else {
    const d = half * CURVE;
    path.bezierCurveTo(x1, y1 + d, x2, y2 - d, x2, y2);
  }
}

/** Records what a trace call draws: one line or curve, or null for nothing. */
function record(trace: (path: PathSink) => void): Line | Curve | null {
  const out: { shape: Line | Curve | null } = { shape: null };
  trace({
    moveTo(x, y) {
      out.shape = { x1: x, y1: y, x2: x, y2: y };
    },
    lineTo(x, y) {
      if (out.shape) out.shape = { ...out.shape, x2: x, y2: y };
    },
    bezierCurveTo(cx1, cy1, cx2, cy2, x, y) {
      if (out.shape) out.shape = { x1: out.shape.x1, y1: out.shape.y1, cx1, cy1, cx2, cy2, x2: x, y2: y };
    },
  });
  return out.shape;
}

/** The line or curve `traceEdge` draws for `edge`. */
export function edgePath(edge: Edge, rowTop: number, rowH: number): Line | Curve {
  return record((path) => traceEdge(path, edge, rowTop, rowH))!;
}

// --- Lane changes across two rows ------------------------------------------------------
// The core changes lane within half a row: a `bottom` half in one row, or a `top` half in the
// next. Drawn as they are, a change across two lanes squeezes into 13 px. Where exactly one
// change meets exactly one straight edge of the neighbouring row at the row boundary, the
// change is drawn as one S-curve between the two rows' centres instead, and the straight
// edge's half under it is left out unless another line still needs it.

/** Drawn as given (half-row curve or line). */
export const SHAPE_HALF = 0;
/** Not drawn: an S-curve drawn by a neighbouring edge covers it. */
export const SHAPE_SKIP = 1;
/** A lane change drawn as an S-curve over a full row (see `traceShape`). */
export const SHAPE_S = 2;
/** A `full` edge drawn only in its upper half (its lower half is under an S-curve). */
export const SHAPE_UPPER = 3;
/** A `full` edge drawn only in its lower half (its upper half is under an S-curve). */
export const SHAPE_LOWER = 4;
export type EdgeShape = 0 | 1 | 2 | 3 | 4;

/** At a junction: a lane change above meets one straight edge below. */
const J_DOWN = 1;
/** At a junction: a lane change below meets one straight edge above. */
const J_UP = 2;
/** The straight partner serves only that change, so its half under the S-curve is left out. */
const J_TRIM = 4;

/**
 * What happens at the point where `lane` crosses the boundary between a row (edges `above`)
 * and the next (edges `below`): whether exactly one lane change meets exactly one straight
 * edge there (J_DOWN or J_UP), and whether that straight edge is the only one on its side
 * (J_TRIM). 0 when either row is unavailable or the pairing is ambiguous.
 */
function junction(above: readonly Edge[] | undefined, below: readonly Edge[] | undefined, lane: number): number {
  if (!above || !below) return 0;
  let up = 0;
  let upChanges = 0;
  for (const e of above) {
    if (e.half === "full" && e.from === lane) up++;
    else if (e.half === "bottom" && e.to === lane) {
      up++;
      if (e.from !== lane) upChanges++;
    }
  }
  let down = 0;
  let downChanges = 0;
  for (const e of below) {
    if (e.half === "full" && e.from === lane) down++;
    else if (e.half === "top" && e.from === lane) {
      down++;
      if (e.to !== lane) downChanges++;
    }
  }
  if (upChanges === 1 && downChanges === 0 && down === 1) return J_DOWN | (up === 1 ? J_TRIM : 0);
  if (downChanges === 1 && upChanges === 0 && up === 1) return J_UP | (down === 1 ? J_TRIM : 0);
  return 0;
}

/** Whether a straight edge is the trimmed partner of an S-curve at junction `j` (`side`). */
function trimmed(j: number, side: number): boolean {
  return (j & side) !== 0 && (j & J_TRIM) !== 0;
}

/**
 * How `edge` of a row (all of whose edges are `row`) is drawn, given the edges of the rows
 * above (`prev`) and below (`next`); pass undefined for a neighbour that is not available
 * (not loaded, or past the drawn range), which keeps the half-row drawing. Allocates nothing.
 */
export function edgeShape(
  edge: Edge,
  prev: readonly Edge[] | undefined,
  row: readonly Edge[],
  next: readonly Edge[] | undefined,
): EdgeShape {
  const changes = edge.from !== edge.to;
  if (edge.half === "bottom") {
    const j = junction(row, next, edge.to);
    if (changes) return j & J_DOWN ? SHAPE_S : SHAPE_HALF;
    return trimmed(j, J_UP) ? SHAPE_SKIP : SHAPE_HALF;
  }
  if (edge.half === "top") {
    const j = junction(prev, row, edge.from);
    if (changes) return j & J_UP ? SHAPE_S : SHAPE_HALF;
    return trimmed(j, J_DOWN) ? SHAPE_SKIP : SHAPE_HALF;
  }
  const upperCovered = trimmed(junction(prev, row, edge.from), J_DOWN);
  const lowerCovered = trimmed(junction(row, next, edge.from), J_UP);
  if (upperCovered && lowerCovered) return SHAPE_SKIP;
  if (upperCovered) return SHAPE_LOWER;
  if (lowerCovered) return SHAPE_UPPER;
  return SHAPE_HALF;
}

/**
 * Adds `edge`, drawn as `shape`, of the row whose top is at `rowTop` to `path`. An S-curve
 * runs from the centre of the row the change starts in to the centre of the next: for a
 * `bottom` change that is this row and the next, for a `top` change the previous row and this.
 */
export function traceShape(path: PathSink, edge: Edge, shape: EdgeShape, rowTop: number, rowH: number): void {
  const half = rowH / 2;
  switch (shape) {
    case SHAPE_HALF:
      traceEdge(path, edge, rowTop, rowH);
      return;
    case SHAPE_SKIP:
      return;
    case SHAPE_UPPER:
    case SHAPE_LOWER: {
      const x = laneX(edge.from);
      const y = shape === SHAPE_UPPER ? rowTop : rowTop + half;
      path.moveTo(x, y);
      path.lineTo(x, y + half);
      return;
    }
    case SHAPE_S: {
      const x1 = laneX(edge.from);
      const x2 = laneX(edge.to);
      const y1 = edge.half === "bottom" ? rowTop + half : rowTop - half;
      const y2 = y1 + rowH;
      const d = rowH * S_CURVE;
      path.moveTo(x1, y1);
      path.bezierCurveTo(x1, y1 + d, x2, y2 - d, x2, y2);
    }
  }
}

// Counts kept per lane at a row boundary (see `junction`), four to a lane.
const UP = 0;
const UP_CHANGES = 1;
const DOWN = 2;
const DOWN_CHANGES = 3;

/** The edges meeting at one row boundary, counted per lane as `junction` counts them. */
class Boundary {
  private counts = new Int32Array(4 * 64);
  /** Lanes with a count set, to clear them again without touching the rest. */
  private touched: number[] = [];

  /** Counts the edges of `above` and `below` (either undefined: no junctions) at each lane. */
  fill(above: readonly Edge[] | undefined, below: readonly Edge[] | undefined): void {
    if (!above || !below) return;
    for (const e of above) {
      if (e.half === "full") this.add(e.from, UP);
      else if (e.half === "bottom") {
        this.add(e.to, UP);
        if (e.from !== e.to) this.add(e.to, UP_CHANGES);
      }
    }
    for (const e of below) {
      if (e.half === "full") this.add(e.from, DOWN);
      else if (e.half === "top") {
        this.add(e.from, DOWN);
        if (e.to !== e.from) this.add(e.from, DOWN_CHANGES);
      }
    }
  }

  /** `junction(above, below, lane)`, from the counts. */
  at(lane: number): number {
    const c = this.counts;
    const k = 4 * lane;
    if (k >= c.length) return 0;
    if (c[k + UP_CHANGES] === 1 && c[k + DOWN_CHANGES] === 0 && c[k + DOWN] === 1) {
      return J_DOWN | (c[k + UP] === 1 ? J_TRIM : 0);
    }
    if (c[k + DOWN_CHANGES] === 1 && c[k + UP_CHANGES] === 0 && c[k + UP] === 1) {
      return J_UP | (c[k + DOWN] === 1 ? J_TRIM : 0);
    }
    return 0;
  }

  clear(): void {
    for (const lane of this.touched) this.counts.fill(0, 4 * lane, 4 * lane + 4);
    this.touched.length = 0;
  }

  private add(lane: number, kind: number): void {
    const k = 4 * lane;
    if (k >= this.counts.length) {
      const bigger = new Int32Array(Math.max(k + 4, 2 * this.counts.length));
      bigger.set(this.counts);
      this.counts = bigger;
    }
    const c = this.counts;
    if (c[k] === 0 && c[k + 1] === 0 && c[k + 2] === 0 && c[k + 3] === 0) this.touched.push(lane);
    c[k + kind]++;
  }
}

/**
 * The shapes (see `edgeShape`) of every edge of a range of rows, worked out in one pass over
 * their edges, however many lanes the graph has: `edgeShape` looks at all the edges of the
 * neighbouring rows for each edge, which on a very wide graph is quadratic per row and frame.
 * Reused from frame to frame; it allocates only to grow.
 */
export class EdgeShapes {
  private shapes = new Uint8Array(256);
  private offsets = new Int32Array(64);
  private first = 0;
  private top = new Boundary();
  private bottom = new Boundary();

  /**
   * Works out the shapes of the edges of rows `first`..`last` - 1, where `edgesOf(i)` gives
   * row i's edges, or undefined for a row that is not loaded or is not a commit.
   */
  compute(first: number, last: number, edgesOf: (i: number) => readonly Edge[] | undefined): void {
    this.first = first;
    const count = Math.max(0, last - first);
    if (this.offsets.length < count + 1) this.offsets = new Int32Array(2 * (count + 1));
    let total = 0;
    for (let i = first; i < last; i++) total += edgesOf(i)?.length ?? 0;
    if (this.shapes.length < total) this.shapes = new Uint8Array(2 * total);

    this.top.clear();
    this.bottom.clear();
    let at = 0;
    let row = count > 0 ? edgesOf(first) : undefined;
    for (let i = first; i < last; i++) {
      this.offsets[i - first] = at;
      const next = i + 1 < last ? edgesOf(i + 1) : undefined;
      this.bottom.fill(row, next);
      if (row) {
        for (const e of row) this.shapes[at++] = this.shapeOf(e);
      }
      // This row's bottom boundary is the next row's top.
      const used = this.top;
      used.clear();
      this.top = this.bottom;
      this.bottom = used;
      row = next;
    }
    this.offsets[count] = at;
    this.top.clear();
  }

  /** The shape of edge `k` of row `i` (a row of the last `compute`). */
  shape(i: number, k: number): EdgeShape {
    return this.shapes[this.offsets[i - this.first] + k] as EdgeShape;
  }

  /** `edgeShape` with the junctions read from the boundary tables. */
  private shapeOf(edge: Edge): EdgeShape {
    const changes = edge.from !== edge.to;
    if (edge.half === "bottom") {
      const j = this.bottom.at(edge.to);
      if (changes) return j & J_DOWN ? SHAPE_S : SHAPE_HALF;
      return trimmed(j, J_UP) ? SHAPE_SKIP : SHAPE_HALF;
    }
    if (edge.half === "top") {
      const j = this.top.at(edge.from);
      if (changes) return j & J_UP ? SHAPE_S : SHAPE_HALF;
      return trimmed(j, J_DOWN) ? SHAPE_SKIP : SHAPE_HALF;
    }
    const upperCovered = trimmed(this.top.at(edge.from), J_DOWN);
    const lowerCovered = trimmed(this.bottom.at(edge.from), J_UP);
    if (upperCovered && lowerCovered) return SHAPE_SKIP;
    if (upperCovered) return SHAPE_LOWER;
    if (lowerCovered) return SHAPE_UPPER;
    return SHAPE_HALF;
  }
}

/** The line or curve `traceShape` draws, or null for nothing. */
export function shapePath(edge: Edge, shape: EdgeShape, rowTop: number, rowH: number): Line | Curve | null {
  return record((path) => traceShape(path, edge, shape, rowTop, rowH));
}

// --- Device pixels ---------------------------------------------------------------------------

/**
 * Backing-store size (`width`, `height`, whole device pixels) of a canvas meant to be
 * `cssW`×`cssH` px at pixel ratio `dpr`, and the element size (`cssWidth`, `cssHeight`) that
 * maps the backing store 1:1 onto device pixels.
 */
export function canvasSize(
  cssW: number,
  cssH: number,
  dpr: number,
): { width: number; height: number; cssWidth: number; cssHeight: number } {
  // Rounded down, so the canvas never reaches past its column.
  const width = Math.max(1, Math.floor(cssW * dpr + 1e-6));
  const height = Math.max(1, Math.round(cssH * dpr));
  return { width, height, cssWidth: width / dpr, cssHeight: height / dpr };
}

/** A line width close to `lineWidth` (px) that covers a whole number of device pixels. */
export function deviceLineWidth(lineWidth: number, dpr: number): number {
  return Math.max(1, Math.round(lineWidth * dpr)) / dpr;
}

/**
 * How far (px) to move a vertical line of `lineWidth` px at `x` so that it covers whole device
 * pixels: centred on a pixel boundary for an even device width, a pixel centre for an odd one.
 */
export function crispOffset(x: number, dpr: number, lineWidth: number): number {
  const device = x * dpr;
  const odd = Math.max(1, Math.round(lineWidth * dpr)) % 2 === 1;
  const target = odd ? Math.floor(device) + 0.5 : Math.round(device);
  return (target - device) / dpr;
}

/**
 * The row at or above `first` whose top (`row × rowH` px) falls on a whole device pixel, so a
 * canvas placed there is not resampled; `first` itself if none within 8 rows does.
 */
export function alignedFirstRow(first: number, rowH: number, dpr: number): number {
  for (let row = first; row >= Math.max(0, first - 8); row--) {
    const device = row * rowH * dpr;
    if (Math.abs(device - Math.round(device)) < 1e-6) return row;
  }
  return first;
}

/**
 * Rows the rendered rows' container moves by at most once in so many rows of scrolling: rows
 * are placed relative to it, so while it stays put, a row of scrolling restyles only the row
 * that comes into view, not all of them.
 */
export const POOL_STEP = 8;

/**
 * Where the rendered rows' container starts (a row index), given where it started (`prev`)
 * and the first row to render now (`first`): unchanged while `first` is at most `step` rows
 * below it; otherwise at `first` (scrolling down) or `step` rows above it (scrolling up), on a
 * row whose top is a whole device pixel (see alignedFirstRow). It is never below `first` and
 * at most `step` + 8 rows above it.
 */
export function poolStart(prev: number, first: number, step: number, rowH: number, dpr: number): number {
  if (first >= prev && first <= prev + step) return prev;
  return alignedFirstRow(first < prev ? Math.max(0, first - step) : first, rowH, dpr);
}

/**
 * The most rows `alignedFirstRow` moves a start up at pixel ratio `dpr` (0 when every row top
 * is a whole device pixel): the extra rows a canvas starting there must cover.
 */
export function alignSlackFor(dpr: number): number {
  let slack = 0;
  for (let f = 8; f < 16; f++) slack = Math.max(slack, f - alignedFirstRow(f, ROW_H, dpr));
  return slack;
}

/** How long rows stay unfilled after the last fast frame, ms, before they are all filled. */
export const FILL_DELAY_MS = 50;
/** During a fast jump, the least time between two frames that fill the rows, ms. */
export const FILL_THROTTLE_MS = 120;

/** What fillFrame remembers between frames (performance.now ms). */
export type FillState = {
  /** Slow frames fill again from this time on: FILL_DELAY_MS after the last fast frame. */
  fillAt: number;
  /** When a fast frame last filled the rows. */
  lastJumpFill: number;
};

/**
 * Whether the frame at `now`, which scrolled `movedPx` px with a viewport `viewportH` px
 * tall, fills the visible rows, and the state for the next frame. A frame that moves more
 * than a viewport (a fling, a scrollbar drag, End) shows rows nobody can read for long: such
 * frames fill at most once every FILL_THROTTLE_MS (the first one at once), so WebKit does not
 * lay out and paint thousands of rows nobody sees, yet no row stays blank for longer than
 * that. Slower frames that follow within FILL_DELAY_MS wait for it; then every frame fills.
 * Moves of up to a viewport (trackpad scrolling, page keys) always fill.
 */
export function fillFrame(
  movedPx: number,
  viewportH: number,
  now: number,
  state: FillState,
): { fill: boolean; state: FillState } {
  if (Math.abs(movedPx) > viewportH) {
    const fill = now - state.lastJumpFill >= FILL_THROTTLE_MS;
    return { fill, state: { fillAt: now + FILL_DELAY_MS, lastJumpFill: fill ? now : state.lastJumpFill } };
  }
  return { fill: now >= state.fillAt, state };
}

/** How many of `totalRows` rows the view shows: all, unless that passes MAX_SCROLL_PX. */
export function displayedRows(totalRows: number, rowH: number): number {
  return Math.min(totalRows, Math.floor(MAX_SCROLL_PX / rowH));
}

export type RowWindow = {
  /** First row to render (inclusive), with overscan. */
  first: number;
  /** Row after the last to render (exclusive), with overscan. */
  last: number;
  /** First row at least partly in view. */
  visibleFirst: number;
  /** Row after the last one at least partly in view. */
  visibleLast: number;
};

/** The rows a viewport `viewportH` px tall scrolled to `scrollTop` shows, ± `overscan`. */
export function rowWindow(
  scrollTop: number,
  viewportH: number,
  rowH: number,
  totalRows: number,
  overscan: number,
): RowWindow {
  const visibleFirst = Math.min(totalRows, Math.max(0, Math.floor(scrollTop / rowH)));
  const visibleLast = Math.min(totalRows, Math.max(visibleFirst, Math.ceil((scrollTop + viewportH) / rowH)));
  return {
    first: Math.max(0, visibleFirst - overscan),
    last: Math.min(totalRows, visibleLast + overscan),
    visibleFirst,
    visibleLast,
  };
}

/** Where a row sits: the first fully visible row, and how far below the viewport's top. */
export type Anchor = { index: number; offset: number };

/** The anchor of scroll position `scrollTop`: its first fully visible row and offset. */
export function computeAnchor(scrollTop: number, rowH: number): Anchor {
  const index = Math.max(0, Math.ceil(scrollTop / rowH));
  return { index, offset: index * rowH - scrollTop };
}

/** The scroll position that puts row `index` `offset` px below the viewport's top. */
export function restoreScrollTop(index: number, offset: number, rowH: number): number {
  return Math.max(0, index * rowH - offset);
}
