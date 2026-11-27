<!--
  The Pull requests page's list: the pinned ones first, then the groups the user chose (by role,
  repository or status), the ones not updated for a month apart and folded; each pull request
  one row: its state (open, a draft muted, merged or closed for a pinned one gone from the lists)
  as an icon in a column of its own, with a dot on it when something happened since it was last
  opened in gg (its title then bolder); its title, its repository and number, who opened it and
  when it changed, its checks and its review decision, and a pin on hover, focus or selection.
  Rows are fixed in height and only those in view (and a few screens around) are drawn, so
  hundreds scroll as smoothly as ten. The list scrolls the page's whole width: its scroll bar
  is at the window's edge; the rows are centred within it.

  One focus stop: ↑ ↓, Page Up/Down, Home and End move the selection (aria-activedescendant)
  through the pull requests and the rows that fold and unfold; Enter or a click opens the one
  selected (or folds); P pins it; Shift+F10, the menu key or a right-click opens its menu.
  The selection and the scroll position are kept while a pull request's page shows.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import { relativeTime } from "../format";
  import { involvedWhy, itemKey, lookOf, moveTo, reviewOf, ROW_H, visibleRange, HEAD_H, type InboxRow, type Item, type Toggle } from "../inbox";
  import { clock, copyLink } from "../state/prs.svelte";
  import { inbox, openInBrowser } from "../state/inbox.svelte";
  import { openMenu } from "../state/overlays.svelte";
  import type { InboxPr } from "../types";
  import PinIcon from "./PinIcon.svelte";
  import PrIcon from "./PrIcon.svelte";

  type Props = {
    layout: { rows: InboxRow[]; items: Item[]; height: number };
    onopen: (pr: InboxPr) => void;
    ontoggle: (t: Toggle) => void;
    onpin: (pr: InboxPr, pinned: boolean) => void;
    /** Grouped by repository: each row leaves out the repository its heading names. */
    byRepo?: boolean;
  };

  let { layout, onopen, ontoggle, onpin, byRepo = false }: Props = $props();

  /** The list has the focus (in it, a row's included). */
  let focused = $state(false);
  /** Escape, or a click on no row, put the highlight away (the place is kept for the keys). */
  let cleared = $state(false);
  /** The selection shows: only while the list has the focus, and until it is put away. */
  const lit = $derived(focused && !cleared);

  let box: HTMLDivElement | undefined = $state();
  let scrollTop = $state(inbox.scrollTop);
  let viewH = $state(600);
  const range = $derived(visibleRange(layout.rows, scrollTop, viewH));
  /** The selected item's place, as the cursor's key finds it in the list now. */
  const at = $derived.by(() => {
    const i = layout.items.findIndex((it) => itemKey(it) === inbox.cursor);
    return i < 0 ? null : i;
  });

  /**
   * The rows drawn: those in view, and the selected one wherever it is, so the list's
   * aria-activedescendant always names a row that is there.
   */
  const shown = $derived.by(() => {
    const rows = layout.rows.slice(range[0], range[1]);
    if (at === null) return rows;
    const active = layout.items[at];
    return lit && active && !rows.includes(active) ? [...rows, active] : rows;
  });

  onMount(() => {
    if (!box) return;
    box.scrollTop = inbox.scrollTop;
    // Read back: a shorter list than before clamps it.
    onScroll();
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

  const heightOf = (r: InboxRow) => (r.kind === "pr" ? ROW_H : r.kind === "head" ? HEAD_H : 36);

  /** Brings item `index` into view, as little as it takes (with its section's heading, for the first of one). */
  function reveal(index: number) {
    const r = layout.items[index];
    if (!r || !box) return;
    const before = layout.rows[layout.rows.indexOf(r) - 1];
    const top = before && before.kind === "head" && before.toggle === null ? before.top : r.top;
    if (top < box.scrollTop) box.scrollTop = top;
    else if (r.top + heightOf(r) > box.scrollTop + box.clientHeight) box.scrollTop = r.top + heightOf(r) - box.clientHeight;
  }

  function activate(item: Item) {
    const key = itemKey(item);
    inbox.cursor = key;
    cleared = false;
    if (item.kind === "pr") {
      onopen(item.pr);
      return;
    }
    ontoggle(item.toggle);
    // The fold stays selected and in sight, what it opened below it.
    void tick().then(() => {
      const i = layout.items.findIndex((it) => itemKey(it) === key);
      if (i >= 0) reveal(i);
    });
  }

  /** The menu of pull request `pr`, at `point`. */
  function menu(pr: InboxPr, point: { x: number; y: number }, keyboard: boolean) {
    inbox.cursor = pr.key;
    openMenu(
      point,
      [
        { id: "open", label: "Open" },
        { id: "pin", label: pr.pinned ? "Unpin" : "Pin to top" },
        { separator: true },
        { id: "browser", label: "Open in browser" },
        { id: "copy", label: "Copy link" },
      ],
      (id) => {
        if (id === "open") onopen(pr);
        else if (id === "pin") onpin(pr, !pr.pinned);
        else if (id === "browser") openInBrowser(pr.url);
        else if (id === "copy") void copyLink(pr.url);
      },
      { keyboard },
    );
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const item = at === null ? null : layout.items[at];
    if (e.key === "Escape") {
      if (lit && item) {
        e.preventDefault();
        cleared = true;
      }
      return;
    }
    if (e.key === "Enter" || e.key === " ") {
      if (item && lit) {
        e.preventDefault();
        activate(item);
      }
      return;
    }
    if ((e.key === "p" || e.key === "P") && !e.shiftKey && item?.kind === "pr" && lit) {
      e.preventDefault();
      onpin(item.pr, !item.pr.pinned);
      return;
    }
    if ((e.key === "o" || e.key === "O") && !e.shiftKey && item?.kind === "pr" && lit) {
      e.preventDefault();
      openInBrowser(item.pr.url);
      return;
    }
    if ((e.key === "F10" && e.shiftKey) || e.key === "ContextMenu") {
      if (item?.kind === "pr" && box && lit) {
        e.preventDefault();
        const el = box.querySelector<HTMLElement>(`#inbox-item-${item.index}`);
        const r = (el ?? box).getBoundingClientRect();
        menu(item.pr, { x: r.left + 40, y: r.bottom - 6 }, true);
      }
      return;
    }
    const page = Math.max(1, Math.floor((box?.clientHeight ?? ROW_H) / ROW_H) - 1);
    // From where it was, the highlight put away or not.
    const next = moveTo(at, e.key, layout.items.length, page);
    if (next === null) return;
    e.preventDefault();
    cleared = false;
    inbox.cursor = itemKey(layout.items[next]);
    void tick().then(() => reveal(next));
  }

  function onFocus() {
    focused = true;
    cleared = false;
    // Focus with nothing selected selects the first.
    if (at === null && layout.items.length > 0) inbox.cursor = itemKey(layout.items[0]);
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
  aria-activedescendant={at === null || !lit ? undefined : `inbox-item-${at}`}
  data-testid="inbox-list"
  data-inbox-focus
  bind:this={box}
  onscroll={onScroll}
  onkeydown={onKeydown}
  onfocus={onFocus}
  onfocusin={() => (focused = true)}
  onfocusout={(e) => {
    if (!box?.contains(e.relatedTarget as Node | null)) focused = false;
  }}
  onclick={(e) => {
    // On no row: the highlight goes.
    const t = e.target as HTMLElement;
    if (t === box || t.classList.contains("space") || (t.classList.contains("head") && !t.classList.contains("fold"))) cleared = true;
  }}
>
  <div class="space" style:height="{layout.height}px">
    {#each shown as row (row.kind === "pr" ? row.pr.key : `${row.kind}:${row.id}`)}
      {#if row.kind === "head" && row.toggle === null}
        <div class="head" class:repo={row.id.startsWith("repo:")} role="presentation" style:top="{row.top}px" data-testid="inbox-group" data-group={row.id}>
          <h2>{row.title}</h2>
          <span class="count">{row.count}</span>
        </div>
      {:else if row.kind === "head" && row.toggle !== null}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="head fold"
          class:open={row.open}
          class:selected={lit && row.index === at}
          role="option"
          id="inbox-item-{row.index}"
          tabindex="-1"
          aria-selected={lit && row.index === at}
          aria-label="{row.title}, {row.count}, {row.open ? 'shown' : 'hidden'}"
          style:top="{row.top}px"
          data-testid="inbox-group"
          data-group={row.id}
          data-open={row.open}
          onclick={() => activate(row)}
        >
          <span class="chevron" aria-hidden="true"></span>
          <h2>{row.title}</h2>
          <span class="count">{row.count}</span>
        </div>
      {:else if row.kind === "more"}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="more"
          class:selected={lit && row.index === at}
          role="option"
          id="inbox-item-{row.index}"
          tabindex="-1"
          aria-selected={lit && row.index === at}
          style:top="{row.top}px"
          data-testid="inbox-more"
          data-id={row.id}
          onclick={() => activate(row)}
        >
          <span class="more-label">{row.label}</span>
        </div>
      {:else if row.kind === "pr"}
        {@const pr = row.pr}
        {@const look = lookOf(pr)}
        {@const review = reviewOf(pr.review)}
        {@const why = involvedWhy(pr)}
        {@const changed = when(pr.updatedAtMs)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row"
          class:selected={lit && row.index === at}
          class:fresh={pr.activity !== "none"}
          class:pinned={pr.pinned}
          role="option"
          id="inbox-item-{row.index}"
          tabindex="-1"
          aria-selected={lit && row.index === at}
          aria-label="{pr.title}, {pr.owner}/{pr.name} #{pr.number}, {look.label}{review && pr.review !== 'changes_requested' ? `, ${review.text}` : ''}{pr.activity === 'new'
            ? ', new'
            : pr.activity === 'updated'
              ? ', new activity'
              : ''}{pr.pinned ? ', pinned' : ''}"
          style:top="{row.top}px"
          data-testid="inbox-pr"
          data-key={pr.key}
          data-activity={pr.activity}
          onclick={() => activate(row)}
          oncontextmenu={(e) => {
            e.preventDefault();
            menu(pr, { x: e.clientX, y: e.clientY }, false);
          }}
        >
          <!-- Its state, in a column of its own so every title starts at one edge: the glyph (open,
               draft, merged, closed) in the colour of what it needs (red: something to fix; amber:
               checks running; green: checks passing; grey: nothing known), and its tooltip says why.
               Something new since it was last opened: a dot on it (and the title bolder). -->
          <span
            class="state t-{look.tone}"
            title={pr.activity === "new"
              ? `${look.label} · new since you last looked`
              : pr.activity === "updated"
                ? `${look.label} · new comments, reviews or commits since you last looked`
                : look.label}
            data-testid="inbox-state-icon"
            data-state={look.glyph}
            data-tone={look.tone}
          >
            <PrIcon kind={look.glyph} />
            {#if pr.activity !== "none"}<span class="unread" aria-hidden="true" data-testid="inbox-unread"></span>{/if}
          </span>
          <div class="main">
            <span class="title" title={pr.title}>{pr.title}</span>
            <span class="meta">
              <!-- Grouped by repository, its heading names it (not a pinned one's: Pinned is no repository). -->
              {#if !byRepo || pr.pinned}<span class="repo">{pr.owner}/{pr.name}</span>{/if}
              <span class="number">#{pr.number}</span>
              {#if pr.author && pr.group !== "mine"}<span class="sep" aria-hidden="true">·</span><span class="by">{pr.author}</span>{/if}
              {#if changed}<span class="sep" aria-hidden="true">·</span><span class="when">{changed}</span>{/if}
              {#if why}<span class="sep" aria-hidden="true">·</span><span class="why">{why}</span>{/if}
            </span>
          </div>
          <div class="side">
            {#if review}
              <span class="review {review.tone}" title={review.text} data-testid="inbox-review" data-review={pr.review}>
                <span class="long">{review.text}</span><span class="short">{review.short}</span>
              </span>
            {/if}
            <!-- Open in browser, then the pin: on hover or selection (the pin always on a pinned one);
                 their room kept either way, so nothing beside them moves. Mouse only (O and P from
                 the keyboard): a click keeps the list's focus and selection. -->
            <button
              class="hover-btn"
              type="button"
              tabindex="-1"
              aria-label="Open in browser"
              title="Open in browser (O)"
              data-testid="inbox-open-browser"
              onmousedown={(e) => e.preventDefault()}
              onclick={(e) => {
                e.stopPropagation();
                openInBrowser(pr.url);
              }}
            >
              <svg width="13" height="13" viewBox="0 0 12 12" aria-hidden="true">
                <path d="M5 2.2H3.1A1.1 1.1 0 0 0 2 3.3v5.6A1.1 1.1 0 0 0 3.1 10h5.6A1.1 1.1 0 0 0 9.8 8.9V7" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" />
                <path d="M6.6 2h3.4v3.4M10 2 5.6 6.4" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </button>
            <button
              class="hover-btn pin"
              class:on={pr.pinned}
              type="button"
              tabindex="-1"
              aria-label={pr.pinned ? "Unpin" : "Pin to top"}
              aria-pressed={pr.pinned}
              title={pr.pinned ? "Unpin (P)" : "Pin to top (P)"}
              data-testid="inbox-pin"
              onmousedown={(e) => e.preventDefault()}
              onclick={(e) => {
                e.stopPropagation();
                onpin(pr, !pr.pinned);
              }}
            >
              <PinIcon on={pr.pinned} />
            </button>
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

  /* The rows, centred in the list's whole width (its scroll bar at the window's edge). */
  .space {
    position: relative;
    width: 100%;
    max-width: 960px;
    margin: 0 auto;
  }

  .head,
  .row,
  .more {
    position: absolute;
    left: 0;
    right: 0;
  }

  /* A heading: its words on the middle of a 28 px box (the room above it is margin, outside
     the box), so a folding one's highlight sits evenly around its chevron, title and count. */
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    margin-top: 6px;
    padding: 0 16px;
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
    padding: 0 14px 0 10px;
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

  /* Its state: open in the open colour, a draft muted; the unread dot on its corner, ringed in
     the row's colour so it reads on any background. */
  .state {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
    width: 20px;
    height: 20px;
  }

  /* Its colour is what it needs; its tooltip and name say why. */
  .state.t-danger {
    color: var(--danger);
  }

  .state.t-warning {
    color: var(--warning);
  }

  .state.t-success {
    color: var(--success);
  }

  .state.t-neutral {
    color: var(--text-muted);
  }

  .state.t-muted {
    color: var(--text-faint);
  }

  .state.t-merged {
    color: var(--info);
  }

  .state.t-closed {
    color: var(--danger);
  }

  .unread {
    position: absolute;
    top: -3px;
    right: -4px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 0 2px var(--bg);
  }

  .row:hover .unread {
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--text) 5.5%, var(--bg));
  }

  .row.selected .unread {
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 11%, var(--bg));
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

  .sep {
    color: var(--text-faint);
  }

  .side {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
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

  /* A repository's name as it is written (case matters): not the groups' small capitals. */
  .head.repo h2 {
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 650;
    letter-spacing: 0;
    text-transform: none;
  }

  /* A heading that folds what is under it: its title where the others' are, a chevron in the
     gutter before it, turning down when open. */
  .head.fold {
    border-radius: var(--radius);
    cursor: default;
  }

  .head.fold:hover {
    background: var(--bg-hover);
  }

  .head.fold.selected {
    background: var(--bg-selected);
  }

  .list:focus-visible .head.fold.selected,
  .list:focus-visible .more.selected {
    box-shadow: inset 0 0 0 2px var(--focus-ring);
  }

  .chevron {
    position: absolute;
    top: 50%;
    left: 5px;
    width: 5px;
    height: 5px;
    margin-top: -3px;
    border-right: 1.3px solid var(--text-faint);
    border-bottom: 1.3px solid var(--text-faint);
    transform: rotate(-45deg);
    transition: transform 80ms var(--ease);
  }

  .head.fold.open .chevron {
    margin-top: -4px;
    transform: rotate(45deg);
  }

  /* "Show N older": a quiet line under its repository, aligned with the titles. */
  .more {
    display: flex;
    align-items: center;
    height: 36px;
    padding: 0 14px 0 40px;
    border-radius: var(--radius);
    color: var(--accent);
    font-size: var(--fs-sm);
    font-weight: 550;
    cursor: default;
  }

  .more:hover .more-label {
    text-decoration: underline;
  }

  .more.selected {
    background: var(--bg-selected);
  }

  .hover-btn {
    display: grid;
    place-items: center;
    flex: none;
    width: 24px;
    height: 24px;
    padding: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--text-faint);
    opacity: 0;
    transition: opacity 80ms linear;
  }

  .row:hover .hover-btn,
  .row.selected .hover-btn,
  .pin.on {
    opacity: 1;
  }

  .pin.on {
    color: var(--accent);
  }

  .hover-btn:hover {
    background: var(--bg-hover);
    color: var(--text);
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
