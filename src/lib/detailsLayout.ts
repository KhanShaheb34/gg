// How the details panel splits its width between the commit's information (left) and its
// changed files (right). The split is the user's (a share of the panel, dragged and
// remembered), never sized from what a commit holds, so the divider stays put from commit to
// commit.

/** Share of the panel the information column takes by default. */
export const DEFAULT_SPLIT = 0.44;
/** Least width of the information column, px. */
export const INFO_MIN = 320;
/** Least width of the files column, px. */
export const FILES_MIN = 280;

/**
 * Width of the information column, px, for share `split` of a panel `panelW` px wide, keeping
 * both columns' minimums; a panel too narrow for both is shared in their proportion.
 */
export function infoWidth(split: number, panelW: number): number {
  if (panelW < INFO_MIN + FILES_MIN) return Math.round((panelW * INFO_MIN) / (INFO_MIN + FILES_MIN));
  return Math.min(panelW - FILES_MIN, Math.max(INFO_MIN, Math.round(split * panelW)));
}

/** The share of a panel `panelW` px wide that an information column `width` px wide is. */
export function splitFor(width: number, panelW: number): number {
  if (panelW <= 0) return DEFAULT_SPLIT;
  return infoWidth(width / panelW, panelW) / panelW;
}

// --- The panels docked under the graph (details, command log) --------------------------------

/** Height the graph's rows always keep, px, below the conflict banner (when one shows). */
export const MIN_GRAPH_H = 160;
/** Least height of the details panel, px, when there is room for it. */
export const DETAILS_MIN_H = 120;
/** Least height of the command log, px, when there is room for it. */
export const LOG_MIN_H = 96;

/**
 * Room left for a bottom panel in a repository view `bodyH` px tall: the graph keeps
 * MIN_GRAPH_H under a banner `bannerH` px tall, and `reserved` px go to the other panel.
 */
export function panelRoom(bodyH: number, bannerH: number, reserved: number): number {
  return Math.max(0, bodyH - MIN_GRAPH_H - bannerH - reserved);
}

/** A panel's height: `wanted`, at least `min`, but never more than `room` (the graph's minimum wins). */
export function panelHeight(wanted: number, min: number, room: number): number {
  return Math.max(0, Math.min(Math.max(wanted, min), room));
}

/**
 * Room for the details panel out of `avail` (what the graph leaves): with the log open, the
 * log keeps its minimum, or half of `avail` when that is less than twice it.
 */
export function detailsRoom(avail: number, logOpen: boolean): number {
  return logOpen ? avail - Math.min(LOG_MIN_H, avail / 2) : avail;
}
