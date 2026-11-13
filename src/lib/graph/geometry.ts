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
/** Number of lane colours (`--lane-0` … `--lane-9`). */
export const LANE_COLORS = 10;
/** Rows rendered above and below the viewport. */
export const OVERSCAN = 10;
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

// --- Lane changes across two rows (R17) ------------------------------------------------------
// The core changes lane within half a row: a `bottom` half in one row, or a `top` half in the
// next. Drawn as they are, a change across two lanes squeezes into 13 px. Where a change is
// joined to a straight line in the neighbouring row at the row boundary, and nothing else
// meets there, the two halves are drawn as one S-curve between the two rows' centres instead.

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

/**
 * Whether the line through `lane` at the boundary between a row (edges `above`) and the next
 * (edges `below`) is one S-curve: exactly one edge reaches that point from each side, and
 * exactly one of the two changes lane.
 */
function joins(above: readonly Edge[] | undefined, below: readonly Edge[] | undefined, lane: number): boolean {
  if (!above || !below) return false;
  let up = 0;
  let upChanges = false;
  for (const e of above) {
    if (e.half === "full" && e.from === lane) up++;
    else if (e.half === "bottom" && e.to === lane) {
      up++;
      upChanges = e.from !== lane;
    }
  }
  let down = 0;
  let downChanges = false;
  for (const e of below) {
    if (e.half === "full" && e.from === lane) down++;
    else if (e.half === "top" && e.from === lane) {
      down++;
      downChanges = e.to !== lane;
    }
  }
  return up === 1 && down === 1 && upChanges !== downChanges;
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
  if (edge.half === "bottom") {
    if (!joins(row, next, edge.to)) return SHAPE_HALF;
    return edge.from !== edge.to ? SHAPE_S : SHAPE_SKIP;
  }
  if (edge.half === "top") {
    if (!joins(prev, row, edge.from)) return SHAPE_HALF;
    return edge.from !== edge.to ? SHAPE_S : SHAPE_SKIP;
  }
  const upperCovered = joins(prev, row, edge.from);
  const lowerCovered = joins(row, next, edge.from);
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
  const width = Math.max(1, Math.round(cssW * dpr));
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
