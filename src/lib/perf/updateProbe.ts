// Performance builds only (GG_PERF_SCROLL=update, loaded with import()): follows gg updating
// itself in its real window, for scripts/update-proof.sh. Marks the version running and the
// tabs open (`update-running <version>`, `update-tabs <names>`), each update state as it is
// seen (`update-state <state> <version or message>`), the notice's text (`update-notice
// <text>`), and, when a version is ready, clicks the notice's "Restart to update" (a DOM click
// on the real button) and marks `update-restart-clicked`; gg then restarts. When the status
// settles on anything else it marks `update-final <state>`.

import { api } from "../ipc";
import { app } from "../state/app.svelte";
import { updates } from "../state/updates.svelte";
import type { UpdateStatus } from "../types";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const mark = (name: string, value = 0) => api.perfMark(name, value).catch(() => {});
const text = (el: Element | null | undefined) => (el?.textContent ?? "").replace(/\s+/g, " ").trim();

/** What a status says, on one line. */
function said(s: UpdateStatus): string {
  switch (s.state) {
    case "failed":
      return `${s.step} ${s.message}${s.detail ? ` (${s.detail})` : ""}`;
    case "available":
      return `${s.version} ${s.reason}`;
    case "upToDate":
      return "";
    default:
      return "version" in s ? s.version : "";
  }
}

const SETTLED = new Set(["ready", "upToDate", "available", "failed", "unavailable"]);

export async function runUpdateProbe(): Promise<void> {
  await mark(`update-running ${app.version}`);
  const active = app.tabs.find((t) => t.repoId === app.activeRepoId)?.name ?? "";
  await mark(`update-tabs ${app.tabs.map((t) => t.name).join(",")} active ${active}`);
  // Follows the states for up to two minutes, until one settles.
  let last = "";
  const end = performance.now() + 120_000;
  let status = updates.status;
  for (;;) {
    status = updates.status;
    const now = `${status.state} ${said(status)}`.trim();
    if (now !== last) {
      last = now;
      await mark(`update-state ${now}`);
    }
    // "idle" until the checks start, a few seconds after the first rows.
    if (SETTLED.has(status.state) && status.state !== "unavailable") break;
    if (performance.now() > end) break;
    await sleep(50);
  }
  if (status.state !== "ready") {
    await mark(`update-final ${status.state}`);
    return;
  }
  const notice = [...document.querySelectorAll<HTMLElement>("[data-notices] .toast")].find((t) =>
    text(t).includes(`gg ${status.version} is ready.`),
  );
  await mark(`update-notice ${text(notice?.querySelector(".message"))}`);
  const restart = [...(notice?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((b) => text(b) === "Restart to update");
  if (!restart) {
    await mark("update-final no restart button");
    return;
  }
  await mark("update-restart-clicked");
  restart.click();
}
