<!--
  Which branches the graph shows: a button naming the choice ("Show all", "3 branches") that
  opens a popover with a filter field and a checklist (Show all, local branches, then remote
  branches when they are shown). Each tick applies at once. Only the rows in view are
  rendered, so thousands of branches cost nothing. Keys: arrows, Page Up/Down, Home and End
  move through the list from the field, Enter ticks, Esc or a click outside closes. Each
  section is a labelled group; the field points (aria-activedescendant) only at a rendered
  option.
-->
<script lang="ts">
  import { tick } from "svelte";
  import { branchItems, filterLabel, toggleBranch, type FilterItem } from "../branchFilter";
  import { matchParts, searchPattern } from "../search";
  import type { RepoView } from "../state/repo.svelte";

  let { view, disabled = false }: { view: RepoView; disabled?: boolean } = $props();

  const ITEM_H = 28;
  const LIST_MAX_H = 336;
  const OVERSCAN = 6;
  const PAGE = 10;
  const id = $derived(`branch-filter-${view.repoId}`);

  let open = $state(false);
  let text = $state("");
  /** Index (in `items`) of the row the keys act on. */
  let active = $state(0);
  let listTop = $state(0);
  let root: HTMLElement;
  let button: HTMLButtonElement;
  let input: HTMLInputElement | undefined = $state();
  let list: HTMLElement | undefined = $state();

  /** The whole selection (hidden remote branches included), and what of it the graph shows. */
  const selected = $derived(view.viewOptions.branches);
  const shown = $derived(view.visibleOptions.branches);
  const chosen = $derived(new Set(selected ?? []));
  const items: FilterItem[] = $derived(
    open ? branchItems(view.snapshot.branches, view.snapshot.remoteBranches, view.viewOptions.showRemotes, text) : [],
  );
  const pattern = $derived(searchPattern(text));
  const first = $derived(Math.max(0, Math.floor(listTop / ITEM_H) - OVERSCAN));
  const last = $derived(Math.min(items.length, Math.ceil((listTop + LIST_MAX_H) / ITEM_H) + OVERSCAN));
  const label = $derived(filterLabel(shown));
  const current = $derived(view.snapshot.head.branch);

  const isOption = (item: FilterItem | undefined) => item !== undefined && item.kind !== "heading";

  type Row = { item: FilterItem; i: number };
  /** The rendered rows by section: Show all, then the local and the remote branches. */
  const sections = $derived.by(() => {
    const all: Row[] = [];
    const local: Row[] = [];
    const remote: Row[] = [];
    for (let i = first; i < last; i++) {
      const item = items[i];
      const isRemote = item.kind === "heading" ? item.label === "Remote branches" : item.kind === "branch" && item.remote;
      (item.kind === "all" ? all : isRemote ? remote : local).push({ item, i });
    }
    return { all, local, remote };
  });
  /** The active option's id, while it is rendered (scrolled out of the rows, there is none). */
  const activeId = $derived(active >= first && active < last && isOption(items[active]) ? `${id}-${active}` : undefined);
  const keyOf = (item: FilterItem) =>
    item.kind === "branch" ? `${item.remote ? "r" : "l"}:${item.name}` : item.kind === "heading" ? `h:${item.label}` : "all";

  function checked(item: FilterItem): boolean {
    if (item.kind === "all") return shown === null;
    return item.kind === "branch" && chosen.has(item.name);
  }

  async function show() {
    text = "";
    active = 0;
    listTop = 0;
    open = true;
    await tick();
    input?.focus();
  }

  function hide(returnFocus: boolean) {
    if (!open) return;
    open = false;
    if (returnFocus) button.focus();
  }

  function toggle(item: FilterItem) {
    if (item.kind === "all") void view.setViewOptions({ branches: null });
    else if (item.kind === "branch") void view.setViewOptions({ branches: toggleBranch(selected, item.name) });
  }

  /** The option nearest `from` going `dir` (skipping headings), or `from` if there is none. */
  function step(from: number, dir: 1 | -1, count = 1): number {
    let at = from;
    let moved = 0;
    for (let i = from + dir; i >= 0 && i < items.length && moved < count; i += dir) {
      if (isOption(items[i])) {
        at = i;
        moved++;
      }
    }
    return at;
  }

  function setActive(index: number) {
    active = index;
    if (!list) return;
    const top = index * ITEM_H;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + ITEM_H > list.scrollTop + list.clientHeight) list.scrollTop = top + ITEM_H - list.clientHeight;
    listTop = list.scrollTop;
  }

  function onInput() {
    // The first matching branch becomes the one Enter ticks.
    const firstBranch = items.findIndex((i) => i.kind === "branch");
    active = text.trim() !== "" && firstBranch >= 0 ? firstBranch : 0;
    if (list) list.scrollTop = 0;
    listTop = 0;
  }

  function onKeydown(e: KeyboardEvent) {
    switch (e.key) {
      case "ArrowDown":
        setActive(step(active, 1));
        break;
      case "ArrowUp":
        setActive(step(active, -1));
        break;
      case "PageDown":
        setActive(step(active, 1, PAGE));
        break;
      case "PageUp":
        setActive(step(active, -1, PAGE));
        break;
      case "Home":
        setActive(0);
        break;
      case "End":
        setActive(step(items.length, -1));
        break;
      case "Enter":
        if (isOption(items[active])) toggle(items[active]);
        break;
      case "Escape":
        hide(true);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function onButtonKeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" && !open) {
      e.preventDefault();
      void show();
    }
  }

  // Closes on a press outside, or when the focus leaves for something outside.
  $effect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (!root.contains(e.target as Node)) hide(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  });

  function onFocusOut(e: FocusEvent) {
    const next = e.relatedTarget as Node | null;
    if (next && !root.contains(next)) hide(false);
  }
</script>

{#snippet entry({ item, i }: Row)}
  {#if item.kind === "heading"}
    <!-- The group's own label says this to assistive technology. -->
    <div class="heading" aria-hidden="true" style:transform="translateY({i * ITEM_H}px)">{item.label}</div>
  {:else}
    {@const on = checked(item)}
    <!-- The keys are the field's (aria-activedescendant); options take clicks only. -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      class="option"
      class:all={item.kind === "all"}
      id="{id}-{i}"
      role="option"
      tabindex="-1"
      aria-selected={on}
      data-active={i === active}
      style:transform="translateY({i * ITEM_H}px)"
      onpointerdown={(e) => e.preventDefault()}
      onpointermove={() => (active = i)}
      onclick={() => {
        active = i;
        toggle(item);
      }}
    >
      <span class="checkbox" data-checked={on} aria-hidden="true"></span>
      {#if item.kind === "all"}
        <span class="name">Show all</span>
      {:else}
        <span class="name" class:current={!item.remote && item.name === current}
          >{#each matchParts(item.name, pattern) as part}{#if part.match}<mark>{part.text}</mark
              >{:else}{part.text}{/if}{/each}</span
        >
        {#if !item.remote && item.name === current}
          <svg class="head" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.6" />
          </svg>
        {/if}
      {/if}
    </div>
  {/if}
{/snippet}

<div class="filter" bind:this={root} onfocusout={onFocusOut}>
  <button
    class="trigger"
    class:filtered={shown !== null}
    class:open
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-controls={open ? `${id}-dialog` : undefined}
    title={disabled ? "Nothing to show yet: this repository has no commits" : "Branches the graph shows"}
    {disabled}
    bind:this={button}
    onclick={() => (open ? hide(false) : show())}
    onkeydown={onButtonKeydown}
  >
    <svg class="lead" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <circle cx="4" cy="3" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" />
      <circle cx="4" cy="11" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" />
      <circle cx="10.5" cy="4.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" />
      <path d="M4 4.6v4.8M10.5 6.1c0 2.4-2.6 2.6-5.5 3.6" fill="none" stroke="currentColor" stroke-width="1.3" />
    </svg>
    <span class="sr-only">Branches: </span>
    <span class="label">{label}</span>
    <svg class="chevron" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M2.5 3.8 5 6.3l2.5-2.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  </button>

  {#if open}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="popover" id="{id}-dialog" role="dialog" aria-label="Branch filter" tabindex="-1" onkeydown={onKeydown}>
      <div class="filter-field">
        <svg width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="7" cy="7" r="4.6" fill="none" stroke="currentColor" stroke-width="1.6" />
          <path d="m10.5 10.5 3.2 3.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
        </svg>
        <input
          type="text"
          role="combobox"
          aria-label="Filter branches"
          aria-expanded="true"
          aria-controls="{id}-list"
          aria-autocomplete="list"
          aria-activedescendant={activeId}
          placeholder="Filter branches"
          spellcheck="false"
          autocomplete="off"
          bind:this={input}
          bind:value={text}
          oninput={onInput}
        />
      </div>
      <div
        class="list"
        id="{id}-list"
        role="listbox"
        aria-label="Branches"
        aria-multiselectable="true"
        tabindex="-1"
        style:max-height="{LIST_MAX_H}px"
        bind:this={list}
        onscroll={() => (listTop = list!.scrollTop)}
      >
        <div class="space" style:height="{items.length * ITEM_H}px">
          {#each sections.all as row (row.i)}
            {@render entry(row)}
          {/each}
          {#if sections.local.length > 0}
            <div class="group" role="group" aria-label="Local branches">
              {#each sections.local as row (keyOf(row.item))}
                {@render entry(row)}
              {/each}
            </div>
          {/if}
          {#if sections.remote.length > 0}
            <div class="group" role="group" aria-label="Remote branches">
              {#each sections.remote as row (keyOf(row.item))}
                {@render entry(row)}
              {/each}
            </div>
          {/if}
        </div>
      </div>
      {#if items.length === 1 && text.trim() !== ""}
        <p class="none">No branch matches “{text.trim()}”</p>
      {/if}
    </div>
  {/if}
</div>

<style>
  .filter {
    position: relative;
    flex: none;
  }

  /* A macOS-style recessed pop-up button: no border; filled on hover and while open. */
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    max-width: 220px;
    padding: 0 7px 0 8px;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text);
    font-size: var(--fs-sm);
    transition: background-color 80ms linear, color 80ms linear;
  }

  .trigger:hover {
    background: var(--bg-hover);
  }

  .trigger:active,
  .trigger.open {
    background: var(--bg-pressed);
  }

  .trigger:disabled {
    background: transparent;
    color: var(--text-faint);
  }

  /* Filtered: the choice reads in the accent colour, on a faint accent fill. */
  .trigger.filtered {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 11%, transparent);
  }

  .trigger.filtered:hover {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }

  .trigger.filtered:active,
  .trigger.filtered.open {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .lead,
  .chevron {
    flex: none;
    color: var(--text-muted);
  }

  .filtered .lead,
  .filtered .chevron {
    color: inherit;
  }

  .chevron {
    transition: transform 120ms var(--ease);
  }

  .open .chevron {
    transform: rotate(180deg);
  }

  .label {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-weight: 500;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .popover {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 30;
    width: 320px;
    padding: 6px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    background: var(--bg-elevated);
    box-shadow: var(--shadow-pop);
    animation: pop 120ms var(--ease);
  }

  /* Not named "field": the dialogs' global .field (a label above its input) would stack the icon on the text. */
  .filter-field {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    margin-bottom: 6px;
    padding: 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--field-bg);
    color: var(--text-faint);
  }

  .filter-field:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .filter-field input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
  }

  .filter-field input::placeholder {
    color: var(--text-faint);
  }

  .list {
    position: relative;
    overflow-y: auto;
    overscroll-behavior: contain;
    outline: none;
  }

  .space {
    position: relative;
  }

  /* A section: its rows sit where the list puts them, as if it were not there. */
  .group {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 0;
  }

  .heading,
  .option {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 28px;
  }

  .heading {
    display: flex;
    align-items: flex-end;
    padding: 0 8px 5px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.02em;
  }

  .option {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 0 8px;
    border-radius: var(--radius);
    font-size: var(--fs-sm);
  }

  .option[data-active="true"] {
    background: var(--bg-hover);
  }

  .option:active {
    background: var(--bg-pressed);
  }

  .option .checkbox {
    pointer-events: none;
  }

  .name {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .all .name {
    font-weight: 500;
  }

  .name.current {
    font-weight: 600;
  }

  .head {
    flex: none;
    color: var(--accent);
  }

  mark {
    border-radius: 2px;
    background: var(--hit-mark);
    color: var(--hit-mark-text);
  }

  .none {
    padding: 6px 8px 4px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
</style>
