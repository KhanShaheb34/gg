// What the UI runs in.

/** Inside the Tauri app (rather than a browser talking to gg-devserver). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** On macOS (Cmd shortcuts, traffic-light window controls). */
export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac/i.test(navigator.platform || navigator.userAgent);
}

/** On Windows (the file manager is File Explorer). */
export function isWindows(): boolean {
  return typeof navigator !== "undefined" && /Win/i.test(navigator.platform || navigator.userAgent);
}

/** What showing a folder in the file manager is called here. */
export function revealLabel(): string {
  return isMac() ? "Reveal in Finder" : isWindows() ? "Show in File Explorer" : "Open containing folder";
}
