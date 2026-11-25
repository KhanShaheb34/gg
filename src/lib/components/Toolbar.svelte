<!--
  The graph's toolbar, in the repository header: which branches the graph shows, the View
  button (remote branches, tags and stashes), then the running operation, search, fetch,
  refresh, the command log and the side panel (with a count of the files to commit). Every
  control changes at once when used; the work it starts shows as it runs (spinning icons, a
  loading line along the header's bottom edge). An operation shows once it has run for
  BUSY_DELAY_MS, so one that ends at once does not flicker; requests queued behind it show as
  a count. Once it has run for a second, git's latest progress line (or the step it is at)
  shows beside its title, cut in the middle. One that has run for STOP_AFTER_MS offers Stop: a
  hook that hangs, or git waiting on something that never comes, must not hold the repository.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { relativeTime, visibleText } from "../format";
  import { isMac } from "../platform";
  import { api } from "../ipc";
  import { reportError } from "../state/app.svelte";
  import { logPrefs, setLogOpen } from "../state/commandLog.svelte";
  import { ops } from "../state/ops.svelte";
  import { clearProgress, progressDue, progressLine, PROGRESS_AFTER_MS, watchOpProgress } from "../state/opProgress.svelte";
  import { NARROW_W, panelShown, setPanelShown, sidePanel } from "../state/worktrees.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import type { RepoSnapshot } from "../types";
  import BranchFilter from "./BranchFilter.svelte";
  import MiddleText from "./MiddleText.svelte";
  import SearchBox from "./SearchBox.svelte";
  import ViewOptions from "./ViewOptions.svelte";

  let { view }: { view: RepoView } = $props();

  const mac = isMac();
  const FETCH_KEYS = mac ? "⌘⇧F" : "Ctrl+Shift+F";
  const REFRESH_KEYS = mac ? "⌘R" : "Ctrl+R";
  const LOG_KEYS = mac ? "⌘⇧L" : "Ctrl+Shift+L";
  const PANEL_KEYS = mac ? "⌘B" : "Ctrl+B";
  const shown = $derived(panelShown(view.repoId));
  /** The active worktree's uncommitted files, as its row in the worktree list counts them. */
  const changes = $derived(view.snapshot.status.changed + view.snapshot.status.untracked);
  const panelTitle = $derived.by(() => {
    const s = view.snapshot.status;
    const parts = [];
    if (s.changed > 0) parts.push(`${s.changed} changed`);
    if (s.untracked > 0) parts.push(`${s.untracked} new`);
    if (s.conflicted > 0) parts.push(`${s.conflicted} with conflicts`);
    return `${shown ? "Hide" : "Show"} the side panel (${PANEL_KEYS})` + (parts.length > 0 ? `\nUncommitted: ${parts.join(", ")}` : "");
  });
  /** A narrow window: View shows as its icon (its name in its tooltip), so nothing is cut off. */
  const compact = $derived(sidePanel.windowW < NARROW_W);

  function toggleLog() {
    setLogOpen(!logPrefs.open);
  }

  /** The time tooltips are written for; set as the pointer (or focus) arrives. */
  let now = $state(Date.now());

  /** A repository without commits: filtering and searching have nothing to work on. */
  const noCommits = $derived(!view.snapshot.loading && view.snapshot.totalRows === 0);
  const fetchInfo = $derived(view.snapshot.fetch);
  const noRemotes = $derived(view.snapshot.remotes.length === 0);
  const fetchState = $derived(view.fetching ? "running" : fetchInfo.lastError ? "error" : "idle");
  const fetchTitle = $derived.by(() => {
    if (noRemotes) return "No remotes to fetch from";
    const lines = [view.fetching ? "Fetching from all remotes…" : `Fetch from all remotes (${FETCH_KEYS})`];
    if (fetchInfo.lastError) lines.push(`Last fetch failed: ${fetchInfo.lastError}`);
    else if (fetchInfo.lastAtMs !== null) lines.push(`Last fetched ${relativeTime(fetchInfo.lastAtMs, now)}`);
    return lines.join("\n");
  });

  const BUSY_DELAY_MS = 150;
  const running = $derived(ops.busy(view.repoId));
  /** The running operation, once it has run for BUSY_DELAY_MS; then until none runs. */
  let busy = $state<RepoSnapshot["op"]>(null);
  /** When an operation was first seen running, while it is not shown yet. */
  let seenAt: number | null = null;
  $effect(() => {
    const op = running;
    if (op === null) {
      busy = null;
      seenAt = null;
      return;
    }
    if (untrack(() => busy) !== null) {
      busy = op; // the next one in the queue, or a new count
      return;
    }
    seenAt ??= performance.now();
    const timer = setTimeout(() => (busy = op), Math.max(0, BUSY_DELAY_MS - (performance.now() - seenAt)));
    return () => clearTimeout(timer);
  });
  watchOpProgress();
  /** The operation shown has run for PROGRESS_AFTER_MS: its progress line shows. */
  let progressShown = $state(false);
  $effect(() => {
    const op = busy;
    progressShown = false;
    if (op === null) {
      clearProgress(view.repoId);
      return;
    }
    if (progressDue(op.startedAtMs, Date.now())) {
      progressShown = true;
      return;
    }
    const timer = setTimeout(() => (progressShown = true), PROGRESS_AFTER_MS - (Date.now() - op.startedAtMs));
    return () => clearTimeout(timer);
  });
  const progress = $derived(busy !== null && progressShown ? progressLine(view.repoId, busy.id) : null);

  const STOP_AFTER_MS = 5_000;
  /** The operation shown has run for STOP_AFTER_MS: Stop is offered. */
  let stoppable = $state(false);
  /** Stop was asked for this operation. */
  let stopping = $state<number | null>(null);
  $effect(() => {
    const op = busy;
    stoppable = false;
    if (op === null) return;
    const left = STOP_AFTER_MS - (Date.now() - op.startedAtMs);
    if (left <= 0) {
      stoppable = true;
      return;
    }
    const timer = setTimeout(() => (stoppable = true), left);
    return () => clearTimeout(timer);
  });

  async function stop() {
    const op = busy;
    if (op === null || stopping === op.id) return;
    stopping = op.id;
    try {
      await api.stopOp(view.repoId, op.id);
    } catch (e) {
      reportError(e);
      stopping = null;
    }
  }

  const busyTitle = $derived(
    busy === null
      ? ""
      : visibleText(busy.title) +
          (progress ? `\n${progress}` : "") +
          (busy.queued > 0 ? `\n${busy.queued} more waiting to run after it` : ""),
  );

  function fetch() {
    if (noRemotes) return;
    void view.fetch();
  }
</script>

<div class="toolbar" class:compact>
  <span class="divider" aria-hidden="true"></span>
  <BranchFilter {view} disabled={noCommits} />

  <ViewOptions {view} disabled={noCommits} {compact} />

  <span class="spacer"></span>

  {#if busy}
    <div class="busy" class:with-progress={progress !== null} role="status" data-testid="busy" title={busyTitle}>
      <svg class="spinner" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.22" stroke-width="1.8" />
        <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
      </svg>
      <span class="busy-title">{visibleText(busy.title)}</span>
      {#if progress !== null}
        <MiddleText class="busy-progress" text={progress} keep={14} endFirst title={null} />
      {/if}
      {#if busy.queued > 0}
        <span class="queued" aria-label="{busy.queued} more queued">+{busy.queued}</span>
      {/if}
    </div>
    {#if stoppable}
      <button
        class="button stop"
        data-testid="stop-op"
        disabled={stopping === busy.id}
        title="End {visibleText(busy.title)} and what git started for it (a hook, say)"
        onclick={() => void stop()}>{stopping === busy.id ? "Stopping…" : "Stop"}</button
      >
    {/if}
  {/if}

  <SearchBox {view} disabled={noCommits} />

  <button
    class="icon-btn tool"
    aria-label="Fetch"
    aria-disabled={noRemotes}
    aria-busy={view.fetching}
    data-state={fetchState}
    title={fetchTitle}
    onpointerenter={() => (now = Date.now())}
    onfocus={() => (now = Date.now())}
    onclick={fetch}
  >
    {#if view.fetching}
      <svg class="spinner" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.22" stroke-width="1.6" />
        <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
      </svg>
    {:else}
      <!-- A cloud with an arrow coming down out of it: fetch from the remotes. -->
      <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
        <path
          d="M5.4 10.2H4.6a2.9 2.9 0 0 1-.3-5.78 3.9 3.9 0 0 1 7.5-.3 2.95 2.95 0 0 1-.3 6.08h-.9M8 6.6v7.6M5.9 12.1 8 14.2l2.1-2.1"
          fill="none"
          stroke="currentColor"
          stroke-width="1.45"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
    {/if}
    {#if fetchState === "error"}
      <span class="warn-dot" aria-hidden="true"></span>
    {/if}
  </button>

  <button
    class="icon-btn tool"
    aria-label="Refresh"
    aria-busy={view.refreshing}
    title="Refresh ({REFRESH_KEYS})"
    onclick={() => view.refresh()}
  >
    <svg class:spinning={view.refreshing} width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M13 8a5 5 0 1 1-1.46-3.54M13 2.8v2.9h-2.9"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  </button>

  <button
    class="icon-btn tool"
    aria-label="Command log"
    aria-pressed={logPrefs.open}
    title="{logPrefs.open ? 'Hide' : 'Show'} the git commands the app ran ({LOG_KEYS})"
    data-testid="log-toggle"
    onclick={toggleLog}
  >
    <!-- A terminal window: a prompt and a line of output. -->
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <rect x="2" y="3" width="12" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.4" />
      <path d="M4.8 6.4 6.6 8l-1.8 1.6M8.2 9.8h3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  </button>

  <!-- The side panel, on the tab last used in this repository (as ⌘B shows it). -->
  <button
    class="icon-btn tool"
    aria-label="Side panel"
    aria-pressed={shown}
    title={panelTitle}
    data-testid="side-toggle"
    onclick={() => setPanelShown(view.repoId)}
  >
    <!-- A window with a column on its right: the side panel. -->
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <rect x="2" y="3" width="12" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.4" />
      <path d="M9.9 3.4v9.2" stroke="currentColor" stroke-width="1.4" />
      <path d="M11.6 6h.7M11.6 8h.7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
    </svg>
    {#if changes > 0}
      <!-- How many files there are to commit; over the button's corner, so nothing moves when it comes. -->
      <span class="badge" aria-hidden="true" data-testid="commit-badge">{changes > 99 ? "99+" : changes}</span>
    {/if}
  </button>

  {#if view.snapshot.loading}
    <!-- Along the bottom edge of the header (the nearest positioned ancestor). -->
    <div class="loading" role="progressbar" aria-label="Loading commits"></div>
  {/if}
</div>

<style>
  /* Never squeezed or cut off: what gives way in a narrow window is the repository's name
     and path beside it (and a long operation title, which is contained). */
  .toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 1 1 auto;
    min-width: min-content;
    height: 100%;
  }

  .compact {
    gap: 2px;
  }

  /* Separates the repository's name and path from the controls. */
  .divider {
    flex: none;
    width: 1px;
    height: 18px;
    margin: 0 10px 0 2px;
    background: var(--border-strong);
  }

  .spacer {
    flex: 1 1 0;
    min-width: 8px;
  }

  .tool {
    position: relative;
    flex: none;
  }

  /* The running operation: a quiet accent pill, its title cut short if it must be. As wide
     from its start as with git's progress beside the title: nothing moves when that comes. */
  .busy {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    flex: 0 1 360px;
    min-width: 0;
    max-width: 360px;
    /* Its title does not widen the toolbar: it takes the room there is. */
    contain: inline-size;
    height: 24px;
    margin-right: 6px;
    padding: 0 9px 0 8px;
    border-radius: 12px;
    background: color-mix(in srgb, var(--accent) 11%, transparent);
    color: var(--accent);
    font-size: var(--fs-sm);
    font-weight: 500;
    white-space: nowrap;
    animation: busy-in 140ms var(--ease);
  }

  .busy .spinner {
    flex: none;
  }

  .busy-title {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text);
  }

  /* With git's progress beside the title: the progress gives way first (its start). */
  .busy.with-progress .busy-title {
    flex: none;
    max-width: 55%;
  }

  .busy :global(.busy-progress) {
    flex: 1 1 0;
    min-width: 3em;
    color: var(--text-muted);
    /* No font-variant here: MiddleText measures with the computed font shorthand, which
       Chrome leaves empty when a variant it cannot write is set. */
    font-weight: 400;
  }

  .busy-title:has(+ :global(.busy-progress))::after {
    content: " ·";
    color: var(--text-faint);
  }

  .queued {
    flex: none;
    padding: 0 5px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    line-height: 16px;
  }

  .stop {
    flex: none;
    height: 24px;
    margin-right: 6px;
    padding: 0 10px;
    font-size: var(--fs-sm);
  }

  @keyframes busy-in {
    from {
      opacity: 0;
      transform: translateX(4px);
    }
  }

  .tool[aria-pressed="true"] {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
  }

  .tool[aria-disabled="true"] {
    color: var(--text-faint);
    background: transparent;
  }

  .tool[data-state="error"] {
    color: var(--text);
  }

  .spinner,
  .spinning {
    animation: spin 0.8s linear infinite;
  }

  .warn-dot {
    position: absolute;
    top: 4px;
    right: 4px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--warning);
    box-shadow: 0 0 0 2px var(--bg);
  }

  /* A quiet count, ringed in the header's colour so it stands clear of the icon under it. */
  .badge {
    position: absolute;
    top: -2px;
    right: -3px;
    min-width: 13px;
    height: 13px;
    padding: 0 3.5px;
    border-radius: 6.5px;
    background: var(--text-faint);
    box-shadow: 0 0 0 1.5px var(--bg);
    color: var(--bg);
    font-size: 9.5px;
    font-variant-numeric: tabular-nums;
    font-weight: 650;
    line-height: 13px;
    text-align: center;
    pointer-events: none;
  }

  .tool[aria-pressed="true"] .badge {
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .loading {
    position: absolute;
    left: 0;
    right: 0;
    bottom: -1px;
    height: 2px;
    overflow: hidden;
    pointer-events: none;
  }

  .loading::after {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    width: 30%;
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    animation: sweep 1.1s var(--ease) infinite;
  }

  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
