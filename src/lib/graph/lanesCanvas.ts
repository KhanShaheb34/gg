// Draws the graph's lanes, edges and commit dots on a canvas, for the rows the graph renders.
// One object per graph, reused for every frame: drawing allocates nothing.

import type { CommitRow } from "../types";
import {
  canvasSize,
  crispOffset,
  deviceLineWidth,
  EdgeShapes,
  GRAPH_PAD,
  LANE_COLORS,
  laneX,
  NODE_R,
  ROW_H,
  traceEdge,
  traceShape,
  type RowWindow,
} from "./geometry";

/** Width of the graph's lines, px (rounded to whole device pixels when drawn). */
const LINE_W = 2;
const DASH = [2, 3];
const SOLID: number[] = [];

/** Where the rows come from (a RowCache). */
export type RowSource = { get(index: number): CommitRow | undefined };

export class LanesCanvas {
  private ctx: CanvasRenderingContext2D | null = null;
  private canvasW = 0;
  private canvasH = 0;
  private canvasRatio = 0;
  /** The shapes of the drawn rows' edges, worked out once per frame. */
  private readonly shapes = new EdgeShapes();
  private rows: RowSource | null = null;
  /** Row i's edges, for EdgeShapes; undefined when it is not loaded or is the uncommitted row. */
  private readonly edgesOf = (i: number) => {
    const r = this.rows?.get(i);
    return r && r.kind !== "uncommitted" ? r.edges : undefined;
  };
  private readonly laneColors: string[] = new Array(LANE_COLORS).fill("#888");
  private faintColor = "#888";

  /** Reads the lane colours of the current theme from `root`'s custom properties. */
  readColors(root: Element): void {
    const style = getComputedStyle(root);
    for (let i = 0; i < LANE_COLORS; i++) this.laneColors[i] = style.getPropertyValue(`--lane-${i}`).trim() || "#888";
    this.faintColor = style.getPropertyValue("--text-faint").trim() || "#888";
  }

  /**
   * Draws rows `w.first`..`w.last` of `rows` on `canvas`, sized `width` px by `height` rows,
   * with row `firstRow` at its top, at pixel ratio `ratio`. Returns whether any loaded row
   * was drawn.
   */
  draw(
    canvas: HTMLCanvasElement,
    rows: RowSource,
    w: RowWindow,
    firstRow: number,
    width: number,
    height: number,
    ratio: number,
  ): boolean {
    this.ctx ??= canvas.getContext("2d");
    const ctx = this.ctx;
    if (!ctx) return false;
    const heightPx = Math.max(1, height) * ROW_H;
    if (width !== this.canvasW || heightPx !== this.canvasH || ratio !== this.canvasRatio) {
      // Backing store in whole device pixels, mapped 1:1 (fractional ratios included).
      const size = canvasSize(width, heightPx, ratio);
      canvas.width = size.width;
      canvas.height = size.height;
      canvas.style.width = `${size.cssWidth}px`;
      canvas.style.height = `${size.cssHeight}px`;
      this.canvasW = width;
      this.canvasH = heightPx;
      this.canvasRatio = ratio;
    }

    // Cleared in device pixels, the whole backing store.
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    // Lanes are 16 px apart, a whole number of device pixels at the usual ratios, so one
    // offset puts every vertical line on whole device pixels.
    ctx.translate(GRAPH_PAD + crispOffset(GRAPH_PAD + laneX(0), ratio, LINE_W), 0);
    const lineW = deviceLineWidth(LINE_W, ratio);
    ctx.lineWidth = lineW;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";

    // Edges, one path per colour. A lane change split over two rows is drawn as one S-curve
    // when both rows are drawn here (see edgeShape); otherwise as its half-row curve.
    this.rows = rows;
    this.shapes.compute(w.first, w.last, this.edgesOf);
    let real = false;
    let uncommitted = -1;
    for (let c = 0; c < LANE_COLORS; c++) {
      let any = false;
      ctx.beginPath();
      for (let i = w.first; i < w.last; i++) {
        const r = rows.get(i);
        if (!r) continue;
        real = true;
        if (r.kind === "uncommitted") {
          uncommitted = i;
          continue;
        }
        const y = (i - firstRow) * ROW_H;
        const edges = r.edges;
        for (let k = 0; k < edges.length; k++) {
          const e = edges[k];
          if (e.color % LANE_COLORS !== c) continue;
          any = true;
          traceShape(ctx, e, this.shapes.shape(i, k), y, ROW_H);
        }
      }
      if (any) {
        ctx.strokeStyle = this.laneColors[c];
        ctx.stroke();
      }
    }
    if (uncommitted >= 0) {
      const r = rows.get(uncommitted)!;
      const y = (uncommitted - firstRow) * ROW_H;
      ctx.beginPath();
      for (const e of r.edges) traceEdge(ctx, e, y, ROW_H);
      ctx.setLineDash(DASH);
      ctx.lineCap = "butt";
      ctx.strokeStyle = this.faintColor;
      ctx.stroke();
      ctx.setLineDash(SOLID);
      ctx.lineCap = "round";
    }

    // Nodes on top. HEAD and the uncommitted row are rings with a punched-out centre, so the
    // row's own background (hover, selection) shows through.
    for (let i = w.first; i < w.last; i++) {
      const r = rows.get(i);
      if (!r) continue;
      const x = laneX(r.lane);
      const y = (i - firstRow) * ROW_H + ROW_H / 2;
      const color = this.laneColors[r.color % LANE_COLORS];
      if (r.kind === "uncommitted" || r.isHead) {
        ctx.globalCompositeOperation = "destination-out";
        ctx.beginPath();
        ctx.arc(x, y, NODE_R + 1, 0, Math.PI * 2);
        ctx.fill();
        ctx.globalCompositeOperation = "source-over";
        ctx.beginPath();
        ctx.arc(x, y, r.kind === "uncommitted" ? NODE_R - 0.25 : NODE_R, 0, Math.PI * 2);
        ctx.lineWidth = r.kind === "uncommitted" ? 1.5 : lineW;
        ctx.strokeStyle = r.kind === "uncommitted" ? this.faintColor : color;
        ctx.stroke();
        ctx.lineWidth = lineW;
      } else if (r.kind === "stash") {
        // A stash is a square: not a commit on a branch.
        const s = NODE_R - 0.5;
        ctx.fillStyle = color;
        ctx.fillRect(x - s, y - s, s * 2, s * 2);
      } else {
        ctx.beginPath();
        ctx.arc(x, y, r.isMerge ? NODE_R - 1 : NODE_R, 0, Math.PI * 2);
        ctx.fillStyle = color;
        ctx.fill();
      }
    }
    return real;
  }
}
