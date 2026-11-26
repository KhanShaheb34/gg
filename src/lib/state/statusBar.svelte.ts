// Whether the status bar shows at the bottom of the window (View › Status bar). One choice for
// the whole app, not per repository: it is part of the window, like the command log's, and
// remembered in localStorage as the other view choices are. Shown unless turned off.

const STORAGE_KEY = "gg.statusBar.v1";

/** The remembered choice; shown when nothing (or nothing readable) is remembered. */
export function readStatusBarShown(): boolean {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    if (raw && typeof raw === "object" && typeof (raw as { shown?: unknown }).shown === "boolean") {
      return (raw as { shown: boolean }).shown;
    }
  } catch {
    // Shown, then.
  }
  return true;
}

export const statusBar = $state({ shown: typeof localStorage === "undefined" ? true : readStatusBarShown() });

/** Shows or hides the status bar, and remembers it (the default by forgetting). */
export function setStatusBarShown(shown: boolean): void {
  statusBar.shown = shown;
  try {
    if (shown) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, JSON.stringify({ shown }));
  } catch {
    // Kept until the window closes.
  }
}

/** Takes over the choice another window made, when storage key `key` changed. */
export function syncStatusBar(key: string | null): void {
  if (key === STORAGE_KEY || key === null) statusBar.shown = readStatusBarShown();
}

if (typeof window !== "undefined") window.addEventListener("storage", (e) => syncStatusBar(e.key));
