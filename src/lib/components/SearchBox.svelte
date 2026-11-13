<!--
  Commit search in the toolbar: a button that opens an inline field (also Cmd/Ctrl+F). Typing
  searches after a short pause; the field shows "3 of 41" for the selected hit. Enter and
  Shift+Enter (or the arrows beside it) move to the next and previous hit, wrapping; Esc
  closes the field, clears the search and gives the focus back.
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import { isMac } from "../platform";
  import type { RepoView } from "../state/repo.svelte";

  let { view }: { view: RepoView } = $props();

  /** Pause after typing before the search runs, ms. */
  const DEBOUNCE_MS = 150;
  const KEYS = isMac() ? "⌘F" : "Ctrl+F";

  // The search of this repository may be open already (the tab was in the background).
  let text = $state(untrack(() => view.searchQuery));
  let input: HTMLInputElement | undefined = $state();
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** Where the focus was before the field took it. */
  let returnTo: HTMLElement | null = null;
  let seenFocus = untrack(() => view.searchFocus);

  const pending = $derived(text.trim() !== view.searchQuery.trim() || view.searchPending);
  const hits = $derived(view.liveHits);
  const counted = $derived.by(() => {
    if (view.searchQuery.trim() === "") return "";
    if (hits.length === 0) return "No results";
    const at = view.searchIndex;
    if (at >= 0) return `${at + 1} of ${hits.length}`;
    return hits.length === 1 ? "1 result" : `${hits.length} results`;
  });
  /** The count shown: the last one known while a search is underway, so it does not flicker. */
  let shown = $state("");
  $effect(() => {
    if (text.trim() === "") shown = "";
    else if (!pending) shown = counted;
  });

  // Takes the focus whenever asked to (the button, Cmd/Ctrl+F).
  $effect(() => {
    const ask = view.searchFocus;
    if (ask === seenFocus) return;
    seenFocus = ask;
    const from = document.activeElement as HTMLElement | null;
    void tick().then(() => {
      if (!input) return;
      if (from && from !== input && !from.closest(".search")) returnTo = from;
      input.focus();
      input.select();
    });
  });

  $effect(() => () => {
    if (timer !== null) clearTimeout(timer);
  });

  function run() {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    void view.runSearch(text);
  }

  function onInput() {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    if (text.trim() === "") run();
    else timer = setTimeout(run, DEBOUNCE_MS);
  }

  function step(dir: 1 | -1) {
    if (timer !== null || text.trim() !== view.searchQuery.trim()) {
      run(); // Enter before the pause ends: search now, which selects the first hit
      return;
    }
    void (dir === 1 ? view.nextHit() : view.prevHit());
  }

  function close() {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    text = "";
    view.closeSearch();
    const target =
      returnTo?.isConnected === true ? returnTo : document.querySelector<HTMLElement>('#repo-panel [role="grid"]');
    returnTo = null;
    target?.focus();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") step(e.shiftKey ? -1 : 1);
    else if (e.key === "Escape") close();
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
</script>

{#if view.searchOpen}
  <div class="search">
    <svg class="glass" width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="7" cy="7" r="4.6" fill="none" stroke="currentColor" stroke-width="1.6" />
      <path d="m10.5 10.5 3.2 3.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
    </svg>
    <input
      type="search"
      aria-label="Search commits"
      placeholder="Subject, author or hash"
      spellcheck="false"
      autocomplete="off"
      bind:this={input}
      bind:value={text}
      oninput={onInput}
      onkeydown={onKeydown}
    />
    <span class="count" class:none={shown === "No results"} data-testid="search-count" aria-live="polite">{shown}</span>
    <span class="divider" aria-hidden="true"></span>
    <button
      class="step"
      aria-label="Previous match"
      title="Previous match (Shift+Enter)"
      disabled={hits.length === 0}
      onclick={() => step(-1)}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
        <path d="M3 7.5 6 4.5l3 3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
    <button
      class="step"
      aria-label="Next match"
      title="Next match (Enter)"
      disabled={hits.length === 0}
      onclick={() => step(1)}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
        <path d="M3 4.5 6 7.5l3-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
    <button class="step" aria-label="Close search" title="Close search (Esc)" onclick={close}>
      <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
        <path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
      </svg>
    </button>
  </div>
{:else}
  <button class="icon-btn" aria-label="Search commits" title="Search commits ({KEYS})" onclick={() => view.openSearch()}>
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="7" cy="7" r="4.6" fill="none" stroke="currentColor" stroke-width="1.5" />
      <path d="m10.5 10.5 3.2 3.2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
    </svg>
  </button>
{/if}

<style>
  .search {
    display: flex;
    align-items: center;
    gap: 2px;
    width: 300px;
    min-width: 200px;
    flex: 0 1 300px;
    height: 28px;
    padding: 0 3px 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--field-bg);
    animation: grow 140ms var(--ease);
  }

  .search:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .glass {
    flex: none;
    margin-right: 5px;
    color: var(--text-faint);
  }

  input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: var(--fs-sm);
  }

  input::placeholder {
    color: var(--text-faint);
  }

  input::-webkit-search-cancel-button {
    display: none;
  }

  .count {
    flex: none;
    padding: 0 4px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .count.none {
    color: var(--warning);
  }

  .divider {
    flex: none;
    width: 1px;
    height: 14px;
    margin: 0 2px;
    background: var(--border);
  }

  .step {
    display: grid;
    place-items: center;
    flex: none;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
  }

  .step:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text);
  }

  .step:active:not(:disabled) {
    background: var(--bg-pressed);
  }

  .step:disabled {
    color: var(--text-faint);
    opacity: 0.6;
  }

  .step svg {
    display: block;
  }

  @keyframes grow {
    from {
      opacity: 0;
      width: 120px;
    }
  }
</style>
