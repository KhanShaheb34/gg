// The command log panel's data: the git commands the core ran for one repository, fetched
// a little at a time (only what is new since the last id), grouped under the operation they
// were part of. It asks the core only while the panel is open (`start`): when the repository
// changes, at most once a second; closed, it holds what it has and does nothing.
//
// Also the panel's preferences (open, height, background reads shown), remembered in local
// storage like the details panel's. Every window of the app shares that storage: a change in
// one is taken over by the others (the storage event).

import { api, onEvent } from "../ipc";
import type { CommandRecord, RepoSnapshot } from "../types";

/** Most commands kept; the oldest entries go first, an operation's commands together. */
export const MAX_KEPT = 1000;
/** Least time between two asks while the repository keeps changing, ms. */
export const LOG_REFRESH_MS = 1000;

/** An operation and its commands, oldest first. `title` once the core has given it one. */
export type OpEntry = { kind: "op"; key: string; opId: number; title: string | null; records: CommandRecord[] };
/** A command that was part of no operation (a background read, a scheduled fetch). */
export type CmdEntry = { kind: "cmd"; key: string; record: CommandRecord };
export type LogEntry = OpEntry | CmdEntry;

export class CommandLogStore {
  /** The entries, oldest first: each operation where its first command came. */
  items = $state<LogEntry[]>([]);
  /** Commands kept. */
  count = $state(0);
  /** The id of the newest command fetched (the next ask is for those after it). */
  lastId = 0;
  /** The repository shown and whether only writes are. */
  repoId: number | null = null;
  writesOnly = true;
  /** The first ask has been answered: an empty list means there is nothing. */
  loaded = $state(false);
  /** Why the first ask failed, until an ask works (the panel offers to try again). */
  error = $state<string | null>(null);

  private ops = new Map<number, OpEntry>();
  /** Bumped when starting over: an answer to an earlier ask is dropped. */
  private epoch = 0;
  private inFlight: Promise<void> | null = null;
  private again = false;
  private lastAsk = -Infinity;
  private timer: ReturnType<typeof setTimeout> | null = null;

  /** Shows repository `repoId`'s commands (writes only, or reads too), starting over if that is new. */
  show(repoId: number, writesOnly: boolean): void {
    if (repoId === this.repoId && writesOnly === this.writesOnly) return;
    this.repoId = repoId;
    this.writesOnly = writesOnly;
    this.epoch++;
    this.items = [];
    this.ops.clear();
    this.count = 0;
    this.lastId = 0;
    this.loaded = false;
    this.error = null;
    this.again = false;
  }

  /**
   * Asks for what is new now. A call while an ask is out runs once more after it; resolves
   * when what was asked is in.
   */
  fetchNow(): Promise<void> {
    if (this.inFlight) {
      this.again = true;
      return this.inFlight;
    }
    this.inFlight = this.ask().finally(() => {
      this.inFlight = null;
      if (this.again) {
        this.again = false;
        void this.fetchNow();
      }
    });
    return this.inFlight;
  }

  private async ask(): Promise<void> {
    const repoId = this.repoId;
    if (repoId === null) return;
    const epoch = this.epoch;
    this.lastAsk = performance.now();
    let records: CommandRecord[];
    try {
      records = await api.commandLog(this.lastId, { repoId, writesOnly: this.writesOnly });
    } catch (e) {
      // Asked again with the next change; with nothing shown yet, the panel says why.
      if (epoch === this.epoch && !this.loaded) this.error = e instanceof Error ? e.message : String(e);
      return;
    }
    if (epoch !== this.epoch) return;
    this.add(records);
    this.loaded = true;
    this.error = null;
  }

  /** Asks soon: now, unless the last ask was under LOG_REFRESH_MS ago (then once it is not). */
  private soon(): void {
    if (this.timer !== null) return;
    const wait = Math.max(0, LOG_REFRESH_MS - (performance.now() - this.lastAsk));
    if (wait === 0) {
      void this.fetchNow();
      return;
    }
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.fetchNow();
    }, wait);
  }

  /**
   * Asks now, then whenever the repository shown changes (at most once a second), until the
   * returned function is called. Nothing is listened to or timed otherwise.
   */
  start(): () => void {
    void this.fetchNow();
    const stopEvents = onEvent<RepoSnapshot>("repo-changed", (s) => {
      if (s.repoId === this.repoId) this.soon();
    });
    return () => {
      stopEvents();
      if (this.timer !== null) clearTimeout(this.timer);
      this.timer = null;
    };
  }

  /** Adds `records` (oldest first, newer than any kept), then drops the oldest past MAX_KEPT. */
  add(records: CommandRecord[]): void {
    for (const record of records) {
      if (record.id <= this.lastId) continue;
      this.lastId = record.id;
      this.count++;
      if (record.opId === null) {
        this.items.push({ kind: "cmd", key: `c${record.id}`, record });
        continue;
      }
      const op = this.ops.get(record.opId);
      if (op) {
        op.records.push(record);
        if (op.title === null && record.opTitle !== null) op.title = record.opTitle;
      } else {
        const entry: OpEntry = { kind: "op", key: `o${record.opId}`, opId: record.opId, title: record.opTitle, records: [record] };
        this.items.push(entry);
        // The proxy the list holds, so later changes show.
        this.ops.set(record.opId, this.items[this.items.length - 1] as OpEntry);
      }
    }
    let drop = 0;
    while (this.count > MAX_KEPT && drop < this.items.length) {
      const gone = this.items[drop++];
      this.count -= gone.kind === "op" ? gone.records.length : 1;
      if (gone.kind === "op") this.ops.delete(gone.opId);
    }
    if (drop > 0) this.items.splice(0, drop);
  }

  /** The entry that holds command `id`, if it is kept. */
  find(id: number): LogEntry | null {
    for (const item of this.items) {
      if (item.kind === "cmd" ? item.record.id === id : item.records.some((r) => r.id === id)) return item;
    }
    return null;
  }
}

/** The panel's data, one per window. */
export const commandLog = new CommandLogStore();

// --- Preferences --------------------------------------------------------------------------

const PREFS_KEY = "gg.log.v1";

export type LogPrefs = {
  open: boolean;
  /** The height the user chose, px; null until they size it. */
  height: number | null;
  /** Background reads are shown too. */
  reads: boolean;
};

/** The remembered preferences, or the defaults (closed, default height, operations only). */
export function readLogPrefs(): LogPrefs {
  const out: LogPrefs = { open: false, height: null, reads: false };
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(PREFS_KEY) ?? "{}");
    if (raw && typeof raw === "object") {
      const o = raw as Record<string, unknown>;
      if (typeof o.open === "boolean") out.open = o.open;
      if (typeof o.height === "number" && Number.isFinite(o.height) && o.height > 0) out.height = Math.round(o.height);
      if (typeof o.reads === "boolean") out.reads = o.reads;
    }
  } catch {
    // Defaults, then.
  }
  return out;
}

export const logPrefs = $state<LogPrefs>(readLogPrefs());

/** Takes over the preferences another window saved, when storage key `key` changed. */
export function syncLogPrefs(key: string | null): void {
  if (key !== PREFS_KEY && key !== null) return;
  Object.assign(logPrefs, readLogPrefs());
}

if (typeof window !== "undefined") window.addEventListener("storage", (e) => syncLogPrefs(e.key));

export function saveLogPrefs(): void {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(logPrefs));
  } catch {
    // Kept until the window closes.
  }
}

// --- Opening and closing -------------------------------------------------------------------

/** The panel's element, for where the focus is. */
const PANEL = '[data-testid="command-log"]';
/** The next panel that mounts takes the focus (opened from the keyboard); taken once. */
let autofocusNext = false;
/** Where the focus was before it went into the panel: it goes back there when the panel closes. */
let returnTo: HTMLElement | null = null;

function focusInPanel(): boolean {
  if (typeof document === "undefined") return false;
  const active = document.activeElement as Element | null;
  return typeof active?.closest === "function" && active.closest(PANEL) !== null;
}

/**
 * Opens or closes the command log: every way in and out (the shortcut, the toolbar button,
 * "Show log", Escape, its close button) goes through here. Opened with `focus` (from the
 * keyboard), the panel takes the focus when it mounts, once. Closed while the focus is inside
 * it, the focus goes back where it came from (else to the graph).
 */
export function setLogOpen(open: boolean, focus = false): void {
  if (open) {
    autofocusNext = focus;
    if (focus && typeof document !== "undefined") {
      const active = document.activeElement;
      returnTo = active && active !== document.body ? (active as HTMLElement) : null;
    }
  } else {
    autofocusNext = false;
    if (focusInPanel()) {
      const back = returnTo;
      if (back?.isConnected) back.focus();
      else document.querySelector<HTMLElement>('[role="grid"]')?.focus({ preventScroll: true });
    }
    returnTo = null;
  }
  if (logPrefs.open !== open) {
    logPrefs.open = open;
    saveLogPrefs();
  }
}

/** Whether the panel mounting now takes the focus; true once per keyboard opening. */
export function takeLogAutofocus(): boolean {
  const focus = autofocusNext;
  autofocusNext = false;
  return focus;
}

/** The focus came into the panel from `el` (outside it): closing puts it back there. */
export function focusCameFrom(el: HTMLElement): void {
  returnTo = el;
}
