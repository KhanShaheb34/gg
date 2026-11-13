// Whether the app window is focused (and visible), for the core's focus-dependent work.

/** The parts of `window` and `document` used, so tests can stand in for them. */
interface Target {
  addEventListener(name: string, listener: () => void): void;
  removeEventListener(name: string, listener: () => void): void;
}
interface Doc extends Target {
  hasFocus(): boolean;
  readonly visibilityState: string;
}

/**
 * Calls `report` with the window's current focus, then again each time it changes (never
 * twice in a row with the same value). Returns a function that stops tracking.
 */
export function trackWindowFocus(win: Target, doc: Doc, report: (focused: boolean) => void): () => void {
  let last: boolean | null = null;
  const set = (focused: boolean) => {
    if (focused === last) return;
    last = focused;
    report(focused);
  };
  const visible = () => doc.visibilityState === "visible";
  // The event says what happened; hasFocus() is only asked when the event does not.
  const onFocus = () => set(visible());
  const onBlur = () => set(false);
  const onVisibility = () => set(visible() && doc.hasFocus());
  win.addEventListener("focus", onFocus);
  win.addEventListener("blur", onBlur);
  doc.addEventListener("visibilitychange", onVisibility);
  onVisibility();
  return () => {
    win.removeEventListener("focus", onFocus);
    win.removeEventListener("blur", onBlur);
    doc.removeEventListener("visibilitychange", onVisibility);
  };
}
