// What the UI runs in.

/** Inside the Tauri app (rather than a browser talking to gg-devserver). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** On macOS (Cmd shortcuts, traffic-light window controls). */
export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac/i.test(navigator.platform || navigator.userAgent);
}
