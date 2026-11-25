// The UI's only way to the core. Inside the Tauri app it uses Tauri's IPC; in a plain
// browser it talks HTTP to gg-devserver. Every failure becomes a CoreError.

import { isTauri } from "./platform";
import type {
  AppState,
  BodyHits,
  CliState,
  CommandRecord,
  CommitDetails,
  Confirmed,
  CoreErrorDto,
  InputField,
  OpOutcome,
  OpenedRepo,
  OpRequest,
  PrDetails,
  PrSuggestion,
  Recent,
  RepoPrefs,
  RepoSnapshot,
  RowsPage,
  ExternalRequest,
  Settings,
  ShellLineAdded,
  ShellLineRemoved,
  UpdateStatus,
  ViewOptions,
  WorktreeFiles,
  WorktreePath,
} from "./types";

export type EventName =
  | "repo-changed"
  | "repo-error"
  | "paths-opened"
  | "op-progress"
  | "cli-changed"
  | "update-changed"
  | "menu-install-cli"
  | "menu-settings"
  | "menu-shortcuts";

/** Files or folders dragged onto the window (Tauri only). `drop` carries their paths. */
export type FolderDragEvent = { kind: "enter" } | { kind: "leave" } | { kind: "drop"; paths: string[] };

/** An error from the core (or from reaching it, as code "io"). */
export class CoreError extends Error {
  code: CoreErrorDto["code"];
  detail: string | null;
  /** The form field it is about, when the core says. */
  field: InputField | null;

  constructor(dto: CoreErrorDto) {
    super(dto.message);
    this.name = "CoreError";
    this.code = dto.code;
    this.detail = dto.detail;
    this.field = dto.field ?? null;
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

/**
 * This page load's name for the numbers below: a reload, or another window, numbers
 * from 1 again under a new one, so the core never waits for numbers it will not get.
 */
const clientId =
  typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;

/** The number of the last operation sent to each repository; a new id starts at 1. */
const clientSeqs = new Map<number, number>();

function nextClientSeq(repoId: number): number {
  const n = (clientSeqs.get(repoId) ?? 0) + 1;
  clientSeqs.set(repoId, n);
  return n;
}

export const api = {
  getAppState: () => call<AppState>("get_app_state"),
  async saveSession(tabs: string[], active: string | null): Promise<void> {
    await call("save_session", { tabs, active });
  },
  updateSettings: (patch: Partial<Settings>) => call<Settings>("update_settings", { patch }),
  removeRecent: (path: string) => call<Recent[]>("remove_recent", { path }),
  /** Puts every setting back to its default ("Reset to defaults"); the settings after. */
  restoreDefaultSettings: () => call<Settings>("restore_default_settings"),
  /** Opens gg's home page or source code in the browser, or shows the settings file in the file manager. */
  openAppLink: (link: "home" | "source" | "settingsFile") => call<{ note: string | null }>("open_app_link", { link }),
  /** Moves an unreadable settings file aside, so settings are saved again. */
  async resetSettings(): Promise<void> {
    await call("reset_settings");
  },
  /**
   * Opens the repository holding `path`. A worktree's folder makes that worktree active (the main
   * one's too); `restore` (a tab of the last session) keeps the worktree remembered as active.
   */
  openRepo: (path: string, restore = false) =>
    call<OpenedRepo>("open_repo", restore ? { path, restore } : { path }),
  async closeRepo(repoId: number): Promise<void> {
    await call("close_repo", { repoId });
  },
  getSnapshot: (repoId: number) => call<RepoSnapshot>("get_snapshot", { repoId }),
  getRows: (repoId: number, generation: number, start: number, count: number) =>
    call<RowsPage>("get_rows", { repoId, generation, start, count }),
  findRow: (repoId: number, hash: string) => call<number | null>("find_row", { repoId, hash }),
  /** Folders the operating system asked the app to open since the last call (Finder, the Dock). */
  takeOpenedPaths: () => call<string[]>("take_opened_paths", {}),
  searchCommits: (repoId: number, query: string) => call<number[]>("search_commits", { repoId, query }),
  /** Rows whose whole message matches `query` (git searches the bodies: slower). */
  searchBodies: (repoId: number, query: string) => call<BodyHits>("search_bodies", { repoId, query }),
  commitDetails: (repoId: number, hash: string) => call<CommitDetails>("commit_details", { repoId, hash }),
  setViewOptions: (repoId: number, options: ViewOptions) =>
    call<RepoSnapshot>("set_view_options", { repoId, options }),
  /** Starts a fetch; progress arrives as `repo-changed` events. */
  async fetch(repoId: number): Promise<void> {
    await call("fetch", { repoId });
  },
  /** Re-reads the repository; `prs`: the user asked, so the pull requests load too (at most once a minute). */
  refresh: (repoId: number, prs = false) => call<RepoSnapshot>("refresh", prs ? { repoId, prs } : { repoId }),
  prSuggest: (repoId: number, branch: string) => call<PrSuggestion>("pr_suggest", { repoId, branch }),
  /** Pull request `number`'s merge status, checks and comments (gh is asked at most once every 10 s per pull request). */
  prDetails: (repoId: number, number: number) => call<PrDetails>("pr_details", { repoId, number }),
  async setWindowFocused(focused: boolean): Promise<void> {
    await call("set_window_focused", { focused });
  },
  async perfMark(name: string, ms: number): Promise<void> {
    await call("perf_mark", { name, ms });
  },
  /**
   * Git commands run after the one with id `sinceId` (0: all kept), oldest first; only those of
   * repository `repoId`, or only writes, if asked.
   */
  commandLog: (sinceId: number, options: { repoId?: number; writesOnly?: boolean } = {}) =>
    call<CommandRecord[]>("command_log", { sinceId, ...options }),
  /**
   * Runs an operation once the ones before it on that repository are done. Each is numbered
   * in the order it is sent, per repository, so that the core queues them in that order
   * even when two reach it in the other.
   */
  runOp: (repoId: number, request: OpRequest, confirmed: Confirmed[] = []) =>
    call<OpOutcome>("run_op", { repoId, request, confirmed, clientId, clientSeq: nextClientSeq(repoId) }),
  /**
   * Stops operation `opId` of repository `repoId` if it still runs: the git command it
   * runs is ended with what it started, and its `runOp` resolves as failed, "Stopped.".
   * Resolves to whether it was running.
   */
  stopOp: (repoId: number, opId: number) => call<boolean>("stop_op", { repoId, opId }),
  getRepoPrefs: (repoId: number) => call<RepoPrefs>("get_repo_prefs", { repoId }),
  setRepoPrefs: (repoId: number, patch: Partial<Pick<RepoPrefs, "mergeNoFf" | "sidePanel">>) =>
    call<RepoPrefs>("set_repo_prefs", { repoId, patch }),
  /** Makes the worktree at `path` the active one (refused while an operation runs or waits there). */
  setActiveWorktree: (repoId: number, path: string) => call<RepoSnapshot>("set_active_worktree", { repoId, path }),
  /** The changed files of the worktree at `path` (any of the repository's, not only the active one). */
  worktreeFiles: (repoId: number, path: string) => call<WorktreeFiles>("worktree_files", { repoId, path }),
  /** Where a new worktree for `branch` goes by default (the `worktreePattern` setting). */
  worktreeDefaultPath: (repoId: number, branch: string) =>
    call<WorktreePath>("worktree_default_path", { repoId, branch }),
  /**
   * Opens `path` (relative to the worktree), or the worktree folder when null, in the editor.
   * Resolves to a note to show when something else was opened (no editor command is set:
   * the worktree folder instead of the file), else null.
   */
  openInEditor: (repoId: number, path: string | null) => call<string | null>("open_in_editor", { repoId, path }),
  /**
   * Opens worktree `path` of repository `repoId` (any of its worktrees; the active one when
   * omitted) in the editor, a terminal or the file manager, or (`url`) the `https://` page
   * `url` in the browser. Resolves to a note to show when something else was opened, else null.
   */
  openExternal: (repoId: number, request: ExternalRequest) =>
    call<{ note: string | null }>("open_external", { repoId, ...request }),
  /**
   * Writes the `gg` launcher into `dir` (one of `CliState.candidates`), replacing only a
   * launcher the app wrote; with `turnOff` (the user ticked the box), then adds gg's line to
   * the shell's start-up file to turn off its own `gg`. Resolves to the command's state after,
   * and what adding the line did (null when not asked, or nothing to turn off).
   */
  installCli: (dir: string, turnOff = false) =>
    call<CliState & { shellLineAdded: ShellLineAdded | null }>("install_cli", { dir, turnOff }),
  /**
   * Removes the launchers the app wrote, and gg's line in a shell file if any; `removed`: the
   * files that went (none: nothing to remove).
   */
  uninstallCli: () => call<CliState & ShellLineRemoved & { removed: string[] }>("uninstall_cli", {}),
  /** Adds gg's line to the shell's start-up file to turn off its own `gg` (Settings). */
  turnOffShellGg: () => call<CliState & { shellLineAdded: ShellLineAdded }>("turn_off_shell_gg", {}),
  /** Takes gg's line out of the shell's start-up files (Settings). */
  removeShellLine: () => call<CliState & ShellLineRemoved>("remove_shell_line", {}),
  /** Where gg updating itself stands. */
  updateStatus: () => call<UpdateStatus>("update_status"),
  /** Starts the update checks (once the first rows are shown; later calls do nothing). */
  startUpdateChecks: () => call<UpdateStatus>("start_update_checks"),
  /** Checks for a new version now ("Check now"); the outcome arrives as `update-changed`. */
  async checkForUpdates(): Promise<void> {
    await call("check_for_updates");
  },
  /**
   * Puts the downloaded version in place and restarts gg. Refused (`busy`) while git works in
   * an open repository; `io` when the install failed (the status says so too).
   */
  async restartToUpdate(): Promise<void> {
    await call("restart_to_update");
  },
  /** Closes the app's window (Cmd/Ctrl+W with no tab open); nothing in a browser. */
  closeWindow: (): Promise<void> => transport().closeWindow(),
  /** Asks the user for a repository folder; null if cancelled. */
  pickFolder: (): Promise<string | null> => transport().pickFolder(),
};
