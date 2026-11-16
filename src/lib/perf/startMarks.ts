// Start-up timing marks for performance runs only: the perf build sets
// `window.__GG_PERF_START__` when it has a perf log (src-tauri, `perf-start`), and then the
// page marks its phases (scripts run, mounted, the app state and the launch repository
// answered), so a launch's time after navigation can be split. Nothing is sent otherwise.
import { api } from "../ipc";

declare global {
  interface Window {
    __GG_PERF_START__?: boolean;
  }
}

/** Records start-up mark `name` at this moment, in a performance run only. */
export function startMark(name: string): void {
  if (typeof window === "undefined" || window.__GG_PERF_START__ !== true) return;
  void api.perfMark(name, performance.now()).catch(() => {});
}
