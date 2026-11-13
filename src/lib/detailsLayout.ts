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
