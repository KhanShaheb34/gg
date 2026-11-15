<!--
  The side panel on the right of a repository: a header (its title, a count, the content's
  own buttons, and Hide) over a body the content fills (the worktree list; the commit panel and
  pull requests later). Its left edge resizes it (SIDE_MIN_W to SIDE_MAX_W, remembered;
  double-click resets). In a wide window it sits beside the graph (at its least width below
  NARROW_W, where it does not resize); in a narrow one (`overlay`) it lies over the graph's
  right side with a shadow, below the conflict banner (`top`), and Escape inside hides it. Its
  header lines up with the graph's column header.
-->
<script lang="ts">
  import type { Snippet } from "svelte";
  import { isMac } from "../platform";
  import { clampWidth, dockedWidth, NARROW_W, saveWidth, SIDE_DEFAULT_W, sidePanel } from "../state/worktrees.svelte";
  import Splitter from "./Splitter.svelte";

  type Props = {
    title: string;
    /** Said beside the title, quieter ("3"). */
    count?: string | null;
    /** Over the graph rather than beside it. */
    overlay: boolean;
    /** Over the graph: how far from the top it starts (below the conflict banner), px. */
    top?: number;
    onhide: () => void;
    actions?: Snippet;
    children: Snippet;
  };

  let { title, count = null, overlay, top = 0, onhide, actions, children }: Props = $props();

  const HIDE_KEYS = isMac() ? "⌘B" : "Ctrl+B";
  /** Over the graph, the panel leaves the graph at least this much of the window. */
  const OVERLAY_GAP = 120;

  const width = $derived(overlay ? Math.min(sidePanel.width, Math.max(200, sidePanel.windowW - OVERLAY_GAP)) : dockedWidth());
  /** Docked at its least width in a narrow window: nothing to resize. */
  const resizable = $derived(overlay || sidePanel.windowW >= NARROW_W);

  let startW = 0;

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
  aria-label={title}
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
    <h2 class="title">{title}</h2>
    {#if count !== null}
      <span class="count">{count}</span>
    {/if}
    <span class="spacer"></span>
    {@render actions?.()}
    <button class="icon-btn small" aria-label="Hide the side panel" title="Hide the side panel ({HIDE_KEYS})" onclick={onhide}>
      <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
        <rect x="1.8" y="2.3" width="10.4" height="9.4" rx="1.8" fill="none" stroke="currentColor" stroke-width="1.25" />
        <path d="M8.6 2.6v8.8" stroke="currentColor" stroke-width="1.25" />
        <path d="M4 7h2.6M5.3 5.6 6.6 7 5.3 8.4" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
  </header>
  <div class="body">
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
    align-items: center;
    gap: 6px;
    flex: none;
    height: 28px;
    padding: 0 3px 0 12px;
    border-bottom: 1px solid var(--border);
  }

  .title {
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }

  .count {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }

  .spacer {
    flex: 1;
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
