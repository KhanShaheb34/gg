<!--
  The Pull requests page's list: a heading per group (yours, waiting for your review,
  mentioned or assigned), then its pull requests, each one row: a dot for activity since it
  was last opened in gg, its title, its repository and number, draft, who opened it and when it
  changed, its checks and its review decision. Rows are fixed in height and only those in view
  (and a few screens around) are drawn, so hundreds scroll as smoothly as ten.

  One focus stop: ↑ ↓, Page Up/Down, Home and End move the selection (aria-activedescendant),
  Enter or a click opens the pull request. The selection and the scroll position are kept while
  a pull request's page shows.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import { relativeTime } from "../format";
  import { checksOf, GROUP_TITLES, HEAD_H, involvedWhy, moveTo, reviewOf, ROW_H, rowOfIndex, rowsOf, visibleRange } from "../inbox";
  import { clock } from "../state/prs.svelte";
  import { inbox } from "../state/inbox.svelte";
  import type { InboxPr } from "../types";

  type Props = {
    prs: InboxPr[];
    onopen: (pr: InboxPr) => void;
  };

  let { prs, onopen }: Props = $props();

  const layout = $derived(rowsOf(prs));
  let box: HTMLDivElement | undefined = $state();
  let scrollTop = $state(inbox.scrollTop);
  let viewH = $state(600);
  const range = $derived(visibleRange(layout.rows, scrollTop, viewH));
  const shown = $derived(layout.rows.slice(range[0], range[1]));
  /** The selected pull request's place, as the cursor's key finds it in the list now. */
  const at = $derived.by(() => {
    const i = prs.findIndex((p) => p.key === inbox.cursor);
    return i < 0 ? null : i;
  });

  onMount(() => {
    if (!box) return;
    box.scrollTop = inbox.scrollTop;
    const observer = new ResizeObserver(() => {
      if (box) viewH = box.clientHeight;
    });
    observer.observe(box);
    viewH = box.clientHeight;
    return () => observer.disconnect();
  });

  function onScroll() {
    if (!box) return;
    scrollTop = box.scrollTop;
    inbox.scrollTop = box.scrollTop;
  }

  /** Brings row `index` into view, as little as it takes. */
  function reveal(index: number) {
    const r = rowOfIndex(layout.rows, index);
    if (!r || !box) return;
    // The group's heading too, for the first of a group.
    const top = r.index === 0 || prs[r.index - 1]?.group !== r.pr.group ? r.top - HEAD_H : r.top;
    if (top < box.scrollTop) box.scrollTop = top;
    else if (r.top + ROW_H > box.scrollTop + box.clientHeight) box.scrollTop = r.top + ROW_H - box.clientHeight;
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "Enter" || e.key === " ") {
      if (at !== null) {
        e.preventDefault();
        onopen(prs[at]);
      }
      return;
    }
    const page = Math.max(1, Math.floor((box?.clientHeight ?? ROW_H) / ROW_H) - 1);
    const next = moveTo(at, e.key, prs.length, page);
    if (next === null) return;
    e.preventDefault();
    inbox.cursor = prs[next].key;
    void tick().then(() => reveal(next));
  }

  function onFocus() {
    // Focus with nothing selected selects the first.
    if (at === null && prs.length > 0) inbox.cursor = prs[0].key;
    if (at !== null) reveal(at);
  }

  /** "2 h ago", from the minute clock. */
  const when = (ms: number | null) => (ms === null ? null : relativeTime(ms, clock.now));
</script>

<div
  class="list"
  role="listbox"
  tabindex="0"
  aria-label="Pull requests"
  aria-activedescendant={at === null ? undefined : `inbox-pr-${at}`}
  data-testid="inbox-list"
  data-inbox-focus
  bind:this={box}
  onscroll={onScroll}
  onkeydown={onKeydown}
  onfocus={onFocus}
>
  <div class="space" style:height="{layout.height}px">
    {#each shown as row (row.kind === "head" ? `h:${row.group}` : row.pr.key)}
      {#if row.kind === "head"}
        <div class="head" role="presentation" style:top="{row.top}px" data-testid="inbox-group" data-group={row.group}>
          <h2>{GROUP_TITLES[row.group]}</h2>
          <span class="count">{row.count}</span>
        </div>
      {:else}
        {@const pr = row.pr}
        {@const checks = checksOf(pr.checks)}
        {@const review = reviewOf(pr.review)}
        {@const why = involvedWhy(pr)}
        {@const changed = when(pr.updatedAtMs)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row"
          class:selected={row.index === at}
          class:fresh={pr.activity !== "none"}
          role="option"
          id="inbox-pr-{row.index}"
          tabindex="-1"
          aria-selected={row.index === at}
          aria-label="{pr.title}, {pr.owner}/{pr.name} #{pr.number}{pr.draft ? ', draft' : ''}{checks ? `, ${checks.text}` : ''}{review ? `, ${review.text}` : ''}{pr.activity === 'new'
            ? ', new'
            : pr.activity === 'updated'
              ? ', new activity'
              : ''}"
          style:top="{row.top}px"
          data-testid="inbox-pr"
          data-key={pr.key}
          data-activity={pr.activity}
          onclick={() => {
            inbox.cursor = pr.key;
            onopen(pr);
          }}
        >
          <span
            class="mark"
            class:new={pr.activity === "new"}
            class:updated={pr.activity === "updated"}
            title={pr.activity === "new" ? "New since you last looked" : pr.activity === "updated" ? "New comments, reviews or commits since you last looked" : undefined}
            aria-hidden="true"
          ></span>
          <div class="main">
            <span class="title" title={pr.title}>{pr.title}</span>
            <span class="meta">
              <span class="repo">{pr.owner}/{pr.name}</span>
              <span class="number">#{pr.number}</span>
              {#if pr.draft}<span class="draft">Draft</span>{/if}
              {#if pr.author && pr.group !== "mine"}<span class="sep" aria-hidden="true">·</span><span class="by">{pr.author}</span>{/if}
              {#if changed}<span class="sep" aria-hidden="true">·</span><span class="when">{changed}</span>{/if}
              {#if why}<span class="sep" aria-hidden="true">·</span><span class="why">{why}</span>{/if}
            </span>
          </div>
          <div class="side">
            {#if checks}
              <span class="checks {checks.tone}" title={checks.text} data-testid="inbox-checks" data-checks={pr.checks}>
                {#if pr.checks === "passing"}
                  <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true"><path d="M3.4 7.3 5.9 9.8l4.7-5.1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
                {:else if pr.checks === "failing"}
                  <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true"><path d="M4.2 4.2l5.6 5.6M9.8 4.2l-5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
                {:else}
                  <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true"><circle cx="7" cy="7" r="3" fill="currentColor" /></svg>
                {/if}
              </span>
            {/if}
            {#if review}
              <span class="review {review.tone}" title={review.text} data-testid="inbox-review" data-review={pr.review}>
                <span class="long">{review.text}</span><span class="short">{review.short}</span>
              </span>
            {/if}
          </div>
        </div>
      {/if}
    {/each}
  </div>
</div>

<style>
  .list {
    /* Its width decides what each row keeps (below). */
    container: inbox-list / inline-size;
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    outline: none;
  }

  .list:focus-visible .row.selected {
    box-shadow: inset 0 0 0 2px var(--focus-ring);
  }

  .space {
    position: relative;
    width: 100%;
  }

  .head,
  .row {
    position: absolute;
    left: 0;
    right: 0;
  }

  .head {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    height: 34px;
    padding: 0 16px 7px;
  }

  .head h2 {
    margin: 0;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-weight: 650;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .count {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 56px;
    padding: 0 14px 0 8px;
    border-radius: var(--radius);
    cursor: default;
    transition: background-color 80ms linear;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .row.selected {
    background: var(--bg-selected);
  }

  /* Activity since it was last opened: a dot before it, and its title a little heavier. */
  .mark {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .mark.new,
  .mark.updated {
    background: var(--accent);
  }

  .mark.updated {
    background: transparent;
    box-shadow: inset 0 0 0 2px var(--accent);
  }

  .main {
    display: flex;
    flex-direction: column;
    gap: 3px;
    flex: 1 1 auto;
    min-width: 0;
  }

  .title {
    overflow: hidden;
    color: var(--text);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .fresh .title {
    font-weight: 650;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }

  .meta > * {
    flex: none;
  }

  .repo {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .number {
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }

  .draft {
    height: 16px;
    padding: 0 5px;
    border: 1px dashed var(--border-strong);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 14px;
  }

  .sep {
    color: var(--text-faint);
  }

  .side {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }

  .checks {
    --tone: var(--text-faint);
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--tone) 14%, transparent);
    color: var(--tone);
  }

  .checks svg {
    display: block;
  }

  .review {
    --tone: var(--text-faint);
    display: inline-flex;
    align-items: center;
    height: 20px;
    padding: 0 8px;
    border: 1px solid color-mix(in srgb, var(--tone) 32%, transparent);
    border-radius: 10px;
    background: color-mix(in srgb, var(--tone) 8%, var(--bg));
    color: var(--text);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }

  .review.neutral {
    border-color: var(--border-strong);
    background: transparent;
    color: var(--text-muted);
  }

  .success {
    --tone: var(--success);
  }

  .danger {
    --tone: var(--danger);
  }

  .warning {
    --tone: var(--warning);
  }

  .short {
    display: none;
  }

  /* Narrow: the review says less, what is after the repository goes first. */
  @container inbox-list (width < 560px) {
    .long {
      display: none;
    }

    .short {
      display: inline;
    }

    .why,
    .by,
    .sep:has(+ .by),
    .sep:has(+ .why) {
      display: none;
    }
  }

  @container inbox-list (width < 400px) {
    .review {
      display: none;
    }

    .when,
    .sep:has(+ .when) {
      display: none;
    }
  }
</style>
