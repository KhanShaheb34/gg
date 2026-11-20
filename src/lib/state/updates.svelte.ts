// gg updating itself, as the UI shows it. The core decides when to check (shortly after the
// first rows are on screen, then about once a day) and downloads a newer version in the
// background; this keeps its status for Settings → About, says once per run in a quiet notice
// that a version is ready ("Restart to update") or, where this copy cannot update itself,
// available (with the download page), and asks the core to restart. A failed check or
// download is said in Settings only. gg never restarts by itself.

import { formatClock, formatDate, type DateFormat } from "../format";
import { api, CoreError, onEvent } from "../ipc";
import { isMac } from "../platform";
import type { UpdateStatus } from "../types";
import { app, dismissToast, notify, reportError, toast } from "./app.svelte";
import { whenFirstRowsShown } from "./firstRows";

export const updates = $state({ status: { state: "unavailable" } as UpdateStatus });

/** With no repository to show (the welcome screen), the checks start this long after start, ms. */
const WELCOME_START_MS = 2_000;
/** And at the latest this long after start, whatever is on screen, ms. */
const LATEST_START_MS = 20_000;

/** The notice shown for a version, by the version and kind (`ready 0.4.1`), and its id. */
let notice: { key: string; id: number } | null = null;
/** The notices dismissed or shown this run: one per version and kind. */
const noticed = new Set<string>();
let started = false;
let initialized = false;

/** Listens for the core's update status and reads it; starts the checks once the window has settled. Call once. */
export function initUpdates(): () => void {
  if (initialized) return () => {};
  initialized = true;
  const stopEvents = onEvent<UpdateStatus>("update-changed", (status) => {
    heard++;
    apply(status);
  });
  readStatus(api.updateStatus());
  const stopFirstRows = whenFirstRowsShown(startUpdates);
  const latest = setTimeout(startUpdates, LATEST_START_MS);
  let welcome: ReturnType<typeof setTimeout> | null = null;
  const stopWelcome = $effect.root(() => {
    $effect(() => {
      // Nothing to load: the welcome screen (or git is missing).
      if (app.ready && !app.restoring && (app.tabs.length === 0 || app.gitMissing) && welcome === null) {
        welcome = setTimeout(startUpdates, WELCOME_START_MS);
      }
    });
  });
  return () => {
    stopEvents();
    stopWelcome();
    stopFirstRows();
    clearTimeout(latest);
    if (welcome !== null) clearTimeout(welcome);
  };
}

/** Tells the core it may start checking (once). */
export function startUpdates(): void {
  if (started) return;
  started = true;
  readStatus(api.startUpdateChecks(), () => (started = false));
}

/** The events heard so far: a status read is older than any event heard while it was asked. */
let heard = 0;

/** Applies the status `read` answers, unless an event came meanwhile (it is newer). */
function readStatus(read: Promise<UpdateStatus>, failed: () => void = () => {}): void {
  const before = heard;
  read.then((status) => {
    if (heard === before) apply(status);
  }, failed);
}

/** "Check now". */
export async function checkNow(): Promise<void> {
  try {
    await api.checkForUpdates();
  } catch (e) {
    reportError(e, "Could not check for updates");
  }
}

/** "Restart to update": refused, with the reason, while git works in a repository. */
export async function restartToUpdate(): Promise<void> {
  try {
    await api.restartToUpdate();
  } catch (e) {
    if (e instanceof CoreError && e.code === "busy") {
      toast("info", e.message);
      return;
    }
    // A failed install: the status says so too, in Settings.
    reportError(e);
  }
}

/** The Homebrew command that updates gg, when Homebrew installed it (macOS). */
export function brewHint(): string | null {
  return isMac() && app.cli.link === "homebrew" ? "brew upgrade --cask gg" : null;
}

function apply(status: UpdateStatus): void {
  updates.status = status;
  const key = status.state === "ready" || status.state === "available" ? `${status.state} ${status.version}` : null;
  // A notice whose version is no longer what the status says goes (installing keeps it).
  if (notice !== null && notice.key !== key && status.state !== "installing") {
    dismissToast(notice.id);
    notice = null;
  }
  if (key === null || noticed.has(key)) return;
  noticed.add(key);
  if (status.state === "ready") {
    notice = {
      key,
      id: notify({
        kind: "info",
        message: `gg ${status.version} is ready.`,
        persistent: true,
        actions: [{ label: "Restart to update", keepOpen: true, run: restartToUpdate }],
      }),
    };
  } else if (status.state === "available") {
    const brew = brewHint();
    notice = {
      key,
      id: notify({
        kind: "info",
        message: `gg ${status.version} is available.`,
        note: brew ? `${status.reason} Or update it with Homebrew: ${brew}` : status.reason,
        persistent: true,
        actions: [{ label: "Download page", run: () => openDownloadPage() }],
      }),
    };
  }
}

/** Opens gg's download page (gg.shakirul.com) in the browser. */
export async function openDownloadPage(): Promise<void> {
  try {
    const { note } = await api.openAppLink("home");
    if (note) toast("info", note);
  } catch (e) {
    reportError(e);
  }
}

/** When a check worked, for "Checked …": today's time, else the date, in the user's format. */
export function checkedWhen(ms: number, format: DateFormat, now: number = Date.now()): string {
  if (format === "relative") return formatClock(ms, format, now);
  const sameDay = new Date(ms).toDateString() === new Date(now).toDateString();
  return sameDay ? `at ${formatClock(ms, format, now)}` : formatDate(Math.floor(ms / 1000), format, now);
}

/** What Settings → About says of `status`, and how it looks (`ok`, `busy`, `warn`, `off`). */
export function statusLine(
  status: UpdateStatus,
  automatic: boolean,
  format: DateFormat,
  now: number = Date.now(),
): { text: string; tone: "ok" | "busy" | "warn" | "off" } {
  switch (status.state) {
    case "unavailable":
      return { text: "This build of gg does not update itself.", tone: "off" };
    case "idle":
      return automatic ? { text: "Not checked yet.", tone: "off" } : { text: "Automatic checks are off.", tone: "off" };
    case "checking":
      return { text: "Checking for updates…", tone: "busy" };
    case "upToDate":
      return { text: `gg is up to date. Checked ${checkedWhen(status.checkedAtMs, format, now)}.`, tone: "ok" };
    case "downloading": {
      const percent = status.total ? Math.min(100, Math.floor((status.received / status.total) * 100)) : null;
      return { text: `Downloading gg ${status.version}…${percent === null ? "" : ` ${percent}%`}`, tone: "busy" };
    }
    case "ready":
      return { text: `gg ${status.version} is ready. Restart to update.`, tone: "ok" };
    case "installing":
      return { text: `Installing gg ${status.version}…`, tone: "busy" };
    case "available":
      return { text: `gg ${status.version} is available.`, tone: "warn" };
    case "failed":
      return { text: status.message, tone: "warn" };
  }
}
