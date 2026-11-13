// Pure geometry of the graph: where lanes, edges and rows sit, which rows a scroll position
// shows, and how to keep a row in place when the rows change under it.

import type { Edge } from "../types";

/** Height of one row, px; the same as `--row-h` in app.css. */
export const ROW_H = 26;
/** Width of one lane, px. */
export const LANE_W = 16;
/** Radius of a commit dot, px. */
export const NODE_R = 4;
/** Most px of rows the scroll content may hold; browsers break somewhere above 16M. */
export const MAX_SCROLL_PX = 15_000_000;
/** How far a curve's control points sit from its ends, as a share of half a row. */
const CURVE = 0.8;

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

/** The line or curve `traceEdge` draws for `edge`. */
export function edgePath(edge: Edge, rowTop: number, rowH: number): Line | Curve {
  let shape: Line | Curve = { x1: 0, y1: 0, x2: 0, y2: 0 };
  traceEdge(
    {
      moveTo(x, y) {
        shape.x1 = x;
        shape.y1 = y;
      },
      lineTo(x, y) {
        shape.x2 = x;
        shape.y2 = y;
      },
      bezierCurveTo(cx1, cy1, cx2, cy2, x, y) {
        shape = { x1: shape.x1, y1: shape.y1, cx1, cy1, cx2, cy2, x2: x, y2: y };
      },
    },
    edge,
    rowTop,
    rowH,
  );
  return shape;
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
