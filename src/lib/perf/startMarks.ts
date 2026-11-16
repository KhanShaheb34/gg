// Start-up timing marks for performance runs only: the perf build sets
// `window.__GG_PERF_START__` when it has a perf log (src-tauri, `perf-start`), and then the
// page marks its phases (scripts run, mounted, the app state and the launch repository
// answered), so a launch's time after navigation can be split. Nothing is sent otherwise.
import { api } from "../ipc";

declare global {
  interface Window {
    __GG_PERF_START__?: boolean;
    /** When the bundle's first line ran (set by its banner, vite.config.ts). */
    __GG_T_BUNDLE__?: number;
  }
}

/** Records start-up mark `name` at this moment, in a performance run only. */
export function startMark(name: string): void {
  if (typeof window === "undefined" || window.__GG_PERF_START__ !== true) return;
  void api.perfMark(name, performance.now()).catch(() => {});
}

/**
 * Records, in a performance run only, how the page itself loaded, from the browser's own
 * timing entries: the HTML's response and its parsing, and each script's and stylesheet's
 * fetch (`res-<file>-start`, `-end`), so the time before `page-main` can be split between
 * fetching and running the code.
 */
export function markLoadTimings(): void {
  if (typeof window === "undefined" || window.__GG_PERF_START__ !== true) return;
  const send = (name: string, ms: number) => void api.perfMark(name, ms).catch(() => {});
  if (typeof window.__GG_T_BUNDLE__ === "number") send("bundle-start", window.__GG_T_BUNDLE__);
  const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  if (nav) {
    send("nav-response-end", nav.responseEnd);
    send("nav-dom-interactive", nav.domInteractive);
  }
  for (const entry of performance.getEntriesByType("resource") as PerformanceResourceTiming[]) {
    const file = entry.name.split("/").pop()?.split("?")[0] ?? entry.name;
    if (!/\.(js|css)$/.test(file)) continue;
    send(`res-${file}-start`, entry.startTime);
    send(`res-${file}-end`, entry.responseEnd);
  }
}

/**
 * Performance runs only, 3 s after start-up (so it cannot slow the launch it explains): how
 * long the app takes to serve the page's script and stylesheet again through its own
 * protocol (`exp-fetch-js`, `exp-fetch-css`, ms, and their sizes): what the page waits for
 * before its first line runs, beside WebKit's parsing of the script (which the app's CSP
 * leaves no way to time from the page).
 */
export function measureLoadCosts(): void {
  if (typeof window === "undefined" || window.__GG_PERF_START__ !== true) return;
  const send = (name: string, ms: number) => void api.perfMark(name, ms).catch(() => {});
  setTimeout(() => {
    void (async () => {
      const script = document.querySelector<HTMLScriptElement>('script[type="module"][src]');
      const style = document.querySelector<HTMLLinkElement>('link[rel="stylesheet"][href]');
      for (const [what, url] of [
        ["js", script?.src],
        ["css", style?.href],
      ] as const) {
        if (!url) continue;
        const t0 = performance.now();
        const text = await (await fetch(url, { cache: "no-store" })).text();
        send(`exp-fetch-${what}`, performance.now() - t0);
        send(`exp-size-${what}`, text.length);
      }
    })();
  }, 3000);
}
