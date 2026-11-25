<!--
  The side panel on the right of a repository: a header (a strip of icon tabs on the left, the
  selected one underlined; the shown tab's own buttons and Hide on the right) over a body the
  shown tab fills (the commit panel, the branch's pull request, the worktree list). A tab
  says what it is in its tooltip and accessible name; one with something waiting (files to
  commit) has a dot at its corner. The tabs take the arrow keys, Home and End, as the ARIA tabs
  pattern has it; only the selected one is in the Tab order. Its left edge
  resizes it (SIDE_MIN_W to SIDE_MAX_W, remembered; double-click resets). In a wide window it
  sits beside the graph (at its least width below NARROW_W, where it does not resize); in a
  narrow one (`overlay`) it lies over the graph's right side with a shadow, below the conflict
  banner (`top`), and Escape inside hides it. Its header lines up with the graph's column
  header, and is the same at every width: four tabs and three buttons fit at the least one.
-->
<script lang="ts">
  import { tick, type Snippet } from "svelte";
  import { isMac } from "../platform";
  import { tabForKey, type SideTab } from "../state/panelState.svelte";
  import { clampWidth, dockedWidth, NARROW_W, saveWidth, SIDE_DEFAULT_W, sidePanel } from "../state/worktrees.svelte";
  import Splitter from "./Splitter.svelte";

  type Tab = {
    id: SideTab;
    /** Its accessible name ("Commit"). */
    label: string;
    /** Its tooltip ("Commit · 6 changed files (⌘K)"). */
    title: string;
    /**
     * A dot at the icon's corner, in a tone: something waiting there (`accent`: files to
     * commit), or how a pull request's checks stand (`danger`, `warning`, `success`).
     */
    dot?: false | "accent" | "success" | "warning" | "danger";
  };

  type Props = {
    tabs: Tab[];
    /** The tab shown. */
    tab: SideTab;
    onselect: (tab: SideTab) => void;
    /** Over the graph rather than beside it. */
    overlay: boolean;
    /** Over the graph: how far from the top it starts (below the conflict banner), px. */
    top?: number;
    onhide: () => void;
    /** The shown tab's own buttons, before Hide. */
    actions?: Snippet;
    children: Snippet;
  };

  let { tabs, tab, onselect, overlay, top = 0, onhide, actions, children }: Props = $props();

  const HIDE_KEYS = isMac() ? "⌘B" : "Ctrl+B";
  /** Over the graph, the panel leaves the graph at least this much of the window. */
  const OVERLAY_GAP = 120;

  const width = $derived(overlay ? Math.min(sidePanel.width, Math.max(200, sidePanel.windowW - OVERLAY_GAP)) : dockedWidth());
  /** Docked at its least width in a narrow window: nothing to resize. */
  const resizable = $derived(overlay || sidePanel.windowW >= NARROW_W);

  let startW = 0;

  // Docked, notices keep clear of it (Toast reads --side-w), as they do of the command log.
  $effect(() => {
    if (overlay) return;
    const root = document.documentElement;
    root.style.setProperty("--side-w", `${width}px`);
    return () => root.style.removeProperty("--side-w");
  });

  let strip: HTMLElement | undefined = $state();

  function onTabKeydown(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    const next = tabForKey(
      tabs.map((t) => t.id),
      tab,
      e.key,
    );
    if (next === null) return;
    e.preventDefault();
    if (next !== tab) onselect(next);
    void tick().then(() => strip?.querySelector<HTMLElement>(`[data-tab="${next}"]`)?.focus());
  }

  function onKeydown(e: KeyboardEvent) {
    if (overlay && e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      onhide();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<aside
  class="side"
  class:overlay
  style:width="{width}px"
  style:top={overlay ? `${top}px` : undefined}
  aria-label="Side panel"
  data-region="side-panel"
  onkeydown={onKeydown}
>
  {#if resizable}<Splitter
    label="Resize the side panel"
    edge="left"
    value={width}
    onstart={() => (startW = sidePanel.width)}
    onmove={(d) => (sidePanel.width = clampWidth(startW - d))}
    onend={saveWidth}
    onreset={() => {
      sidePanel.width = SIDE_DEFAULT_W;
      saveWidth();
    }}
  />{/if}
  <header class="head">
    <div class="tabs" role="tablist" aria-label="Side panel" bind:this={strip} data-testid="side-tabs">
      {#each tabs as t (t.id)}
        {@const selected = t.id === tab}
        <button
          class="tab"
          role="tab"
          id="side-tab-{t.id}"
          aria-selected={selected}
          aria-controls="side-tabpanel"
          aria-label={t.label}
          tabindex={selected ? 0 : -1}
          title={t.title}
          data-tab={t.id}
          data-testid="side-tab-{t.id}"
          onclick={() => onselect(t.id)}
          onkeydown={onTabKeydown}
        >
          <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
            {#if t.id === "commit"}
              <!-- A commit: a circle on a line. -->
              <circle cx="8" cy="8" r="3.2" fill="none" stroke="currentColor" stroke-width="1.5" />
              <path d="M1.3 8h3.5M11.2 8h3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
            {:else if t.id === "pr"}
              <!-- A pull request: two commits on a line, and a third whose line comes back to
                   the first with an arrow (the markers' mark). -->
              <circle cx="4.2" cy="3.6" r="1.9" fill="none" stroke="currentColor" stroke-width="1.4" />
              <circle cx="4.2" cy="12.4" r="1.9" fill="none" stroke="currentColor" stroke-width="1.4" />
              <circle cx="11.8" cy="12.4" r="1.9" fill="none" stroke="currentColor" stroke-width="1.4" />
              <path
                d="M4.2 5.5v5M11.8 10.5V6.4c0-1.2-.8-2-2-2H7.4"
                fill="none"
                stroke="currentColor"
                stroke-width="1.4"
                stroke-linecap="round"
              />
              <path d="M8.9 2.6 7.1 4.4l1.8 1.8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
            {:else}
              <!-- Two folders, one behind the other: the repository's worktrees. -->
              <path
                d="M2 6.3c0-.6.5-1.1 1.1-1.1h2.2l1.2 1.3h4.4c.6 0 1.1.5 1.1 1.1v4.9c0 .6-.5 1.1-1.1 1.1H3.1c-.6 0-1.1-.5-1.1-1.1zM4.6 5V3.5c0-.6.5-1.1 1.1-1.1h2.1l1.2 1.3h3.9c.6 0 1.1.5 1.1 1.1v4.7c0 .6-.5 1.1-1.1 1.1h-.7"
                fill="none"
                stroke="currentColor"
                stroke-width="1.4"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            {/if}
          </svg>
          {#if t.dot}<span class="dot {t.dot}" aria-hidden="true" data-testid="side-tab-dot" data-tone={t.dot}></span>{/if}
        </button>
      {/each}
    </div>
    <span class="spacer"></span>
    <div class="actions">
      {@render actions?.()}
      <button class="icon-btn small" aria-label="Hide the side panel" title="Hide the side panel ({HIDE_KEYS})" onclick={onhide}>
        <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
          <rect x="1.8" y="2.3" width="10.4" height="9.4" rx="1.8" fill="none" stroke="currentColor" stroke-width="1.25" />
          <path d="M8.6 2.6v8.8" stroke="currentColor" stroke-width="1.25" />
          <path d="M4 7h2.6M5.3 5.6 6.6 7 5.3 8.4" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>
    </div>
  </header>
  <div class="body" id="side-tabpanel" role="tabpanel" aria-labelledby="side-tab-{tab}">
    {@render children()}
  </div>
</aside>

<style>
  .side {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: none;
    min-height: 0;
    border-left: 1px solid var(--border);
    background: var(--bg);
  }

  /* Over the graph (a narrow window): lifted off it. */
  .overlay {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    z-index: 20;
    border-left-color: var(--border-strong);
    box-shadow: var(--shadow-pop);
    animation: slide 140ms var(--ease);
  }

  @keyframes slide {
    from {
      opacity: 0;
      transform: translateX(12px);
    }
  }

  /* As tall as the graph's column header, so the two read as one strip. */
  .head {
    display: flex;
    align-items: stretch;
    flex: none;
    height: 28px;
    padding: 0 3px 0 4px;
    border-bottom: 1px solid var(--border);
  }

  .tabs {
    display: flex;
    flex: none;
    gap: 2px;
    /* Over the header's bottom border: the selected tab's line takes its place. */
    margin-bottom: -1px;
  }

  /* An icon, as the header's buttons draw theirs, over the line that marks the selected tab. */
  .tab {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
    width: 30px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: 0;
    background: transparent;
    color: var(--text-muted);
    transition: color 80ms linear;
  }

  .tab svg {
    display: block;
  }

  .tab:hover,
  .tab[aria-selected="true"] {
    color: var(--text);
  }

  /* The selected tab's line, along the header's bottom edge. */
  .tab::after {
    content: "";
    position: absolute;
    left: 5px;
    right: 5px;
    bottom: 0;
    height: 2px;
    border-radius: 1px 1px 0 0;
    background: transparent;
    transition: background-color 80ms linear;
  }

  .tab:hover::after {
    background: var(--border-strong);
  }

  .tab[aria-selected="true"]::after {
    background: var(--accent);
  }

  .tab:focus-visible {
    outline-offset: -3px;
    border-radius: 6px;
  }

  /* Something waiting there: a dot at the icon's top right, ringed in the header's colour. */
  .dot {
    position: absolute;
    top: 5px;
    right: 5px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 0 1.5px var(--bg);
    pointer-events: none;
  }

  .dot.success {
    background: var(--success);
  }

  .dot.warning {
    background: var(--warning);
  }

  .dot.danger {
    background: var(--danger);
  }

  .spacer {
    flex: 1 1 0;
    min-width: 0;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
  }

  .head :global(.icon-btn.small) {
    width: 22px;
    height: 22px;
    border-radius: 5px;
  }

  .body {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
</style>
