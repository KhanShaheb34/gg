<!--
  The graph's toolbar, in the repository header: which branches the graph shows, whether it
  shows remote branches, tags and stashes, then the running operation, search, fetch and
  refresh. Every control changes at once when used; the work it starts shows as it runs
  (spinning icons, a loading line along the header's bottom edge). An operation shows once
  it has run for BUSY_DELAY_MS, so one that ends at once does not flicker; requests queued
  behind it show as a count.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { relativeTime, visibleText } from "../format";
  import { isMac } from "../platform";
  import { logPrefs, setLogOpen } from "../state/commandLog.svelte";
  import { ops } from "../state/ops.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import type { RepoSnapshot } from "../types";
  import BranchFilter from "./BranchFilter.svelte";
  import SearchBox from "./SearchBox.svelte";

  let { view }: { view: RepoView } = $props();

  const mac = isMac();
  const FETCH_KEYS = mac ? "⌘⇧F" : "Ctrl+Shift+F";
  const REFRESH_KEYS = mac ? "⌘R" : "Ctrl+R";
  const LOG_KEYS = mac ? "⌘⇧L" : "Ctrl+Shift+L";

  function toggleLog() {
    setLogOpen(!logPrefs.open);
  }

  /** The time tooltips are written for; set as the pointer (or focus) arrives. */
  let now = $state(Date.now());

  /** A repository without commits: filtering and searching have nothing to work on. */
  const noCommits = $derived(!view.snapshot.loading && view.snapshot.totalRows === 0);
  const NO_COMMITS = "Nothing to show yet: this repository has no commits";
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
  const busyTitle = $derived(
    busy === null
      ? ""
      : visibleText(busy.title) + (busy.queued > 0 ? `\n${busy.queued} more waiting to run after it` : ""),
  );

  function fetch() {
    if (noRemotes) return;
    void view.fetch();
  }
</script>

<div class="toolbar">
  <span class="divider" aria-hidden="true"></span>
  <BranchFilter {view} disabled={noCommits} />

  <label class="toggle" title={noCommits ? NO_COMMITS : undefined}>
    <input
      type="checkbox"
      class="checkbox"
      disabled={noCommits}
      checked={view.viewOptions.showRemotes}
      onchange={(e) => view.setViewOptions({ showRemotes: e.currentTarget.checked })}
    />
    <span>Remote branches</span>
  </label>
  <label class="toggle" title={noCommits ? NO_COMMITS : undefined}>
    <input
      type="checkbox"
      class="checkbox"
      disabled={noCommits}
      checked={view.viewOptions.showTags}
      onchange={(e) => view.setViewOptions({ showTags: e.currentTarget.checked })}
    />
    <span>Tags</span>
  </label>
  <!-- With a branch selection, the stashes on its commits show (R21). -->
  <label class="toggle" title={noCommits ? NO_COMMITS : undefined}>
    <input
      type="checkbox"
      class="checkbox"
      disabled={noCommits}
      checked={view.viewOptions.showStashes}
      onchange={(e) => view.setViewOptions({ showStashes: e.currentTarget.checked })}
    />
    <span>Stashes</span>
  </label>

  <span class="spacer"></span>

  {#if busy}
    <div class="busy" role="status" data-testid="busy" title={busyTitle}>
      <svg class="spinner" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.22" stroke-width="1.8" />
        <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
      </svg>
      <span class="busy-title">{visibleText(busy.title)}</span>
      {#if busy.queued > 0}
        <span class="queued" aria-label="{busy.queued} more queued">+{busy.queued}</span>
      {/if}
    </div>
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

  {#if view.snapshot.loading}
    <!-- Along the bottom edge of the header (the nearest positioned ancestor). -->
    <div class="loading" role="progressbar" aria-label="Loading commits"></div>
  {/if}
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 1 1 auto;
    min-width: 0;
    height: 100%;
  }

  /* Separates the repository's name and path from the controls. */
  .divider {
    flex: none;
    width: 1px;
    height: 18px;
    margin: 0 10px 0 2px;
    background: var(--border-strong);
  }

  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 28px;
    padding: 0 9px 0 8px;
    border-radius: var(--radius);
    color: var(--text);
    font-size: var(--fs-sm);
    white-space: nowrap;
    transition: background-color 80ms linear;
  }

  .toggle:hover {
    background: var(--bg-hover);
  }

  .toggle:active {
    background: var(--bg-pressed);
  }

  /* Off (a repository without commits): faint, and no hover. */
  .toggle:has(:disabled) {
    background: transparent;
    color: var(--text-faint);
  }

  .spacer {
    flex: 1 1 0;
    min-width: 8px;
  }

  .tool {
    position: relative;
    flex: none;
  }

  /* The running operation: a quiet accent pill, its title cut short if it must be. */
  .busy {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    flex: 0 1 auto;
    min-width: 0;
    max-width: 280px;
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
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text);
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
