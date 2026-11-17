// Performance runs only (GG_PERF_SCROLL=actions, loaded with import()): the feedback of the
// graph's actions in the real window. On a repository it may write to (a scratch clone), it
// measures, in the page, from just before the event to the first animation frame after the
// change is in the DOM (the frame that paints it):
//   - right-click on a row -> its menu painted (action-menu-*);
//   - "Create branch here…" picked -> its dialog painted (action-dialog-*);
//   - Enter in that dialog -> it shows it is creating, painted (action-confirm-*: the
//     feedback), and -> it is gone (action-closed-*: the operation and its refresh);
//   - the operation's outcome received -> the new branch's label painted in the graph
//     (action-graph-*), and Enter -> label (action-total-*);
//   - the toolbar's busy indicator, if it showed (it shows only after 150 ms by design).
// It creates branches `perf-<n>-<k>`; the repository is meant to be thrown away.

import { api } from "../ipc";
import { percentile } from "./scrollProbe";

const nextFrame = () => new Promise<number>((resolve) => requestAnimationFrame(() => resolve(performance.now())));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Waits, a frame at a time, until `ready()` holds; returns the time of that frame, or null after `frames`. */
async function paintedWhen(ready: () => boolean, frames = 300): Promise<number | null> {
  for (let f = 0; f < frames; f++) {
    const at = await nextFrame();
    if (ready()) return at;
  }
  return null;
}

function visibleRows(scroller: HTMLElement): HTMLElement[] {
  const top = scroller.querySelector("[data-header]")?.getBoundingClientRect().bottom ?? 0;
  const bottom = scroller.getBoundingClientRect().bottom;
  return [...scroller.querySelectorAll<HTMLElement>('[role="row"][aria-busy="false"]:not([hidden])')].filter((row) => {
    const r = row.getBoundingClientRect();
    return r.top >= top && r.bottom <= bottom;
  });
}

function contextMenuOn(el: HTMLElement): void {
  const r = el.getBoundingClientRect();
  el.dispatchEvent(
    new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      button: 2,
      clientX: r.left + Math.min(40, r.width / 2),
      clientY: r.top + r.height / 2,
    }),
  );
}

const menu = () => document.querySelector<HTMLElement>('[data-testid="context-menu"]');
const dialog = () => document.querySelector<HTMLElement>('[role="dialog"], [role="alertdialog"]');

function escape(): void {
  (document.activeElement ?? document.body).dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
  );
}

export type ActionTimes = {
  menu: number[];
  dialog: number[];
  confirm: number[];
  closed: number[];
  graph: number[];
  total: number[];
  busy: number;
};

/** Runs `count` rounds against the graph in `scroller`. */
export async function runActionsProbe(scroller: HTMLElement, count = 10): Promise<ActionTimes> {
  const times: ActionTimes = { menu: [], dialog: [], confirm: [], closed: [], graph: [], total: [], busy: 0 };
  // When each outcome arrived (the operation's end, as the page learns it).
  let outcomeAt = 0;
  const runOp = api.runOp.bind(api);
  api.runOp = async (...args: Parameters<typeof api.runOp>) => {
    const outcome = await runOp(...args);
    outcomeAt = performance.now();
    return outcome;
  };
  const run = Date.now().toString(36);
  // The rows may still be skeletons when the repository has loaded: wait for some to show.
  await paintedWhen(() => visibleRows(scroller).length > 0, 600);
  try {
    for (let k = 0; k < count; k++) {
      // Right-click -> menu.
      const rows = visibleRows(scroller);
      const row = rows[k % Math.max(1, rows.length)];
      const subject = row?.querySelector<HTMLElement>(".subject");
      if (!subject) break;
      let started = performance.now();
      contextMenuOn(subject);
      const shown = await paintedWhen(() => menu() !== null);
      if (shown === null) break;
      times.menu.push(shown - started);

      // Pick "Create branch here…" -> dialog.
      const item = [...(menu()?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])].find((el) =>
        el.textContent?.includes("Create branch here"),
      );
      if (!item) break;
      started = performance.now();
      item.click();
      const opened = await paintedWhen(() => dialog() !== null);
      if (opened === null) break;
      times.dialog.push(opened - started);

      // Type a name; Enter -> the dialog closes (feedback), the operation ends, the label shows.
      const input = dialog()?.querySelector<HTMLInputElement>("input[type=text], input:not([type])");
      if (!input) break;
      const name = `perf-${run}-${k}`;
      input.value = name;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await nextFrame();
      outcomeAt = 0;
      let busySeen = false;
      started = performance.now();
      input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      // The feedback: the dialog shows it is creating (its field turns read-only, the button
      // says "Creating…"; R27: it stays open while a bad name could still come back), or it
      // has closed.
      let feedback: number | null = null;
      const closed = await paintedWhen(() => {
        busySeen ||= document.querySelector('[data-testid="busy"]') !== null;
        const open = dialog();
        if (feedback === null && (open === null || input.readOnly)) feedback = performance.now();
        return open === null;
      });
      if (closed === null || feedback === null) {
        escape();
        break;
      }
      times.confirm.push(feedback - started);
      times.closed.push(closed - started);
      const labelled = await paintedWhen(() => {
        busySeen ||= document.querySelector('[data-testid="busy"]') !== null;
        return [...scroller.querySelectorAll(".ref")].some((el) => el.textContent?.trim() === name);
      }, 600);
      if (busySeen) times.busy++;
      if (labelled !== null) {
        times.total.push(labelled - started);
        if (outcomeAt > 0) times.graph.push(labelled - outcomeAt);
      }
      await sleep(300);
    }
  } finally {
    api.runOp = runOp;
    if (menu()) escape();
  }
  return times;
}

/** The marks for `times`: per measure its count, p50, p95 and max. */
export function actionMarks(times: ActionTimes): [string, number][] {
  const marks: [string, number][] = [];
  for (const key of ["menu", "dialog", "confirm", "closed", "graph", "total"] as const) {
    const xs = times[key];
    marks.push(
      [`action-${key}-n`, xs.length],
      [`action-${key}-p50`, percentile(xs, 50)],
      [`action-${key}-p95`, percentile(xs, 95)],
      [`action-${key}-max`, xs.length ? Math.max(...xs) : 0],
    );
  }
  marks.push(["action-busy-shown", times.busy]);
  return marks;
}
