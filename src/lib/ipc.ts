// The UI's only way to the core. Inside the Tauri app it uses Tauri's IPC; in a plain
// browser it talks HTTP to gg-devserver. Every failure becomes a CoreError.

import { isTauri } from "./platform";
import type {
  AppState,
  CommandRecord,
  CommitDetails,
  CoreErrorDto,
  Recent,
  RepoSnapshot,
  RowsPage,
  Settings,
  ViewOptions,
} from "./types";

export type EventName = "repo-changed" | "repo-error";

/** Files or folders dragged onto the window (Tauri only). `drop` carries their paths. */
export type FolderDragEvent = { kind: "enter" } | { kind: "leave" } | { kind: "drop"; paths: string[] };

/** An error from the core (or from reaching it, as code "io"). */
export class CoreError extends Error {
  code: CoreErrorDto["code"];
  detail: string | null;

  constructor(dto: CoreErrorDto) {
    super(dto.message);
    this.name = "CoreError";
    this.code = dto.code;
    this.detail = dto.detail;
  }
}

type Handler = (payload: unknown) => void;

interface Transport {
  invoke(cmd: string, args: Record<string, unknown>): Promise<unknown>;
  listen(name: EventName, handler: Handler): () => void;
  pickFolder(): Promise<string | null>;
  closeWindow(): Promise<void>;
  onFolderDrag(handler: (event: FolderDragEvent) => void): () => void;
}

function isCoreErrorDto(value: unknown): value is CoreErrorDto {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as CoreErrorDto).code === "string" &&
    typeof (value as CoreErrorDto).message === "string"
  );
}

function unreachable(message: string, cause: unknown): CoreError {
  return new CoreError({ code: "io", message, detail: String(cause) });
}

// --- Tauri ------------------------------------------------------------------------------
// The @tauri-apps modules are imported on first use, so the browser build never loads them.

const tauriTransport: Transport = {
  async invoke(cmd, args) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      return await invoke("invoke_core", { cmd, args });
    } catch (e) {
      throw isCoreErrorDto(e) ? new CoreError(e) : unreachable("The core did not answer", e);
    }
  },

  listen(name, handler) {
    let unlisten: (() => void) | null = null;
    let stopped = false;
    import("@tauri-apps/api/event")
      .then(({ listen }) => listen(name, (event) => handler(event.payload)))
      .then((off) => {
        if (stopped) off();
        else unlisten = off;
      })
      .catch((e: unknown) => console.error(`could not listen for ${name}`, e));
    return () => {
      stopped = true;
      unlisten?.();
    };
  },

  async pickFolder() {
    let picked;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      picked = await open({ directory: true, multiple: false });
    } catch (e) {
      throw unreachable("Could not show the folder picker", e);
    }
    return typeof picked === "string" ? picked : null;
  },

  async closeWindow() {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().close();
    } catch (e) {
      throw unreachable("Could not close the window", e);
    }
  },

  onFolderDrag(handler) {
    let unlisten: (() => void) | null = null;
    let stopped = false;
    import("@tauri-apps/api/webview")
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent(({ payload }) => {
          if (payload.type === "enter") handler({ kind: "enter" });
          else if (payload.type === "leave") handler({ kind: "leave" });
          else if (payload.type === "drop") handler({ kind: "drop", paths: payload.paths });
        }),
      )
      .then((off) => {
        if (stopped) off();
        else unlisten = off;
      })
      .catch((e: unknown) => console.error("could not listen for dropped folders", e));
    return () => {
      stopped = true;
      unlisten?.();
    };
  },
};

// --- Browser (gg-devserver) -------------------------------------------------------------

/** Delay before reopening an event stream the browser gave up on. */
const RECONNECT_MS = 1000;

declare global {
  interface Window {
    /** Overrides the dev server URL at run time (the e2e tests start one per test). */
    __GG_DEV_URL__?: string;
  }
}

function devUrl(): string {
  const override = typeof window !== "undefined" ? window.__GG_DEV_URL__ : undefined;
  return override ?? import.meta.env.VITE_GG_DEV_URL ?? "http://127.0.0.1:1421";
}

/** Handlers by event name, all fed by one shared EventSource. */
const handlers = new Map<string, Set<Handler>>();
let source: EventSource | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

function openEvents(): EventSource {
  const es = new EventSource(`${devUrl()}/events`);
  es.onmessage = (e: MessageEvent<string>) => {
    let message: { name: string; payload: unknown };
    try {
      message = JSON.parse(e.data);
    } catch {
      return;
    }
    for (const handler of [...(handlers.get(message.name) ?? [])]) handler(message.payload);
  };
  es.onerror = () => {
    // The browser retries a dropped stream by itself; once it gives up (CLOSED), start over.
    if (es.readyState !== EventSource.CLOSED || source !== es) return;
    source = null;
    reconnectTimer ??= setTimeout(() => {
      reconnectTimer = null;
      if (handlers.size > 0 && !source) source = openEvents();
    }, RECONNECT_MS);
  };
  return es;
}

function stopEventsIfUnused(): void {
  if (handlers.size > 0) return;
  source?.close();
  source = null;
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

const httpTransport: Transport = {
  async invoke(cmd, args) {
    let body: { ok?: unknown; err?: unknown };
    try {
      // The dev server takes only JSON, so browsers preflight (and it checks the origin).
      const response = await fetch(`${devUrl()}/invoke`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ cmd, args }),
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      body = await response.json();
    } catch (e) {
      throw unreachable("Could not reach the dev server", e);
    }
    if (body.err !== undefined) {
      throw isCoreErrorDto(body.err)
        ? new CoreError(body.err)
        : unreachable("Malformed error", JSON.stringify(body.err));
    }
    return body.ok;
  },

  listen(name, handler) {
    let set = handlers.get(name);
    if (!set) handlers.set(name, (set = new Set()));
    set.add(handler);
    source ??= openEvents();
    return () => {
      const current = handlers.get(name);
      if (!current?.delete(handler)) return;
      if (current.size === 0) handlers.delete(name);
      stopEventsIfUnused();
    };
  },

  async pickFolder() {
    const path = window.prompt("Repository path")?.trim();
    return path ? path : null;
  },

  // A page cannot close a window it did not open; the browser's own Cmd+W closes the tab.
  async closeWindow() {},

  // A browser page never learns the real path of a dropped folder.
  onFolderDrag() {
    return () => {};
  },
};

// --- API --------------------------------------------------------------------------------

function transport(): Transport {
  return isTauri() ? tauriTransport : httpTransport;
}

function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  return transport().invoke(cmd, args) as Promise<T>;
}

/** Calls `handler` as files are dragged onto the window; inert in a browser. Returns a stop function. */
export function onFolderDrop(handler: (event: FolderDragEvent) => void): () => void {
  return transport().onFolderDrag(handler);
}

/** Calls `handler` with each event `name`; returns a function that stops it. */
export function onEvent<T>(name: EventName, handler: (payload: T) => void): () => void {
  return transport().listen(name, handler as Handler);
}

export const api = {
  getAppState: () => call<AppState>("get_app_state"),
  async saveSession(tabs: string[], active: string | null): Promise<void> {
    await call("save_session", { tabs, active });
  },
  updateSettings: (patch: Partial<Settings>) => call<Settings>("update_settings", { patch }),
  removeRecent: (path: string) => call<Recent[]>("remove_recent", { path }),
  openRepo: (path: string) => call<RepoSnapshot>("open_repo", { path }),
  async closeRepo(repoId: number): Promise<void> {
    await call("close_repo", { repoId });
  },
  getSnapshot: (repoId: number) => call<RepoSnapshot>("get_snapshot", { repoId }),
  getRows: (repoId: number, generation: number, start: number, count: number) =>
    call<RowsPage>("get_rows", { repoId, generation, start, count }),
  findRow: (repoId: number, hash: string) => call<number | null>("find_row", { repoId, hash }),
  searchCommits: (repoId: number, query: string) => call<number[]>("search_commits", { repoId, query }),
  commitDetails: (repoId: number, hash: string) => call<CommitDetails>("commit_details", { repoId, hash }),
  setViewOptions: (repoId: number, options: ViewOptions) =>
    call<RepoSnapshot>("set_view_options", { repoId, options }),
  /** Starts a fetch; progress arrives as `repo-changed` events. */
  async fetch(repoId: number): Promise<void> {
    await call("fetch", { repoId });
  },
  refresh: (repoId: number) => call<RepoSnapshot>("refresh", { repoId }),
  async setWindowFocused(focused: boolean): Promise<void> {
    await call("set_window_focused", { focused });
  },
  async perfMark(name: string, ms: number): Promise<void> {
    await call("perf_mark", { name, ms });
  },
  /** Git commands run after the one with id `sinceId` (0: all kept), oldest first. */
  commandLog: (sinceId: number) => call<CommandRecord[]>("command_log", { sinceId }),
  /** Closes the app's window (Cmd/Ctrl+W with no tab open); nothing in a browser. */
  closeWindow: (): Promise<void> => transport().closeWindow(),
  /** Asks the user for a repository folder; null if cancelled. */
  pickFolder: (): Promise<string | null> => transport().pickFolder(),
};
