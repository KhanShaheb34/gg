<!--
  The tab bar's + (⌘T, Ctrl+T): a menu under it to open another repository: Open folder… (as
  ⌘O does), the recent repositories (filtered as you type; one already in a tab says so, and
  choosing it shows that tab), and Clone, from a link (a link typed or pasted here becomes it)
  or one of your repositories. The field keeps the focus: ↑ ↓ choose, Enter opens, Esc or a
  click outside closes, and the focus goes back where it was.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import { newTabRows, type NewTabRow } from "../clone";
  import { colorIndex, displayFolder, shortenPath, visibleText } from "../format";
  import { moveTo } from "../inbox";
  import { isMac } from "../platform";
  import { app, openPath, openViaPicker } from "../state/app.svelte";
  import { afterMenu, openClone } from "../state/clone.svelte";

  let { close }: { close: () => void } = $props();

  const uid = `new-tab-${Math.random().toString(36).slice(2, 8)}`;
  const mac = isMac();
  const OPEN_KEYS = mac ? "⌘O" : "Ctrl+O";
  const CLONE_KEYS = mac ? "⇧⌘O" : "Ctrl+Shift+O";

  let query = $state("");
  let active = $state(0);
  let listEl = $state<HTMLElement>();
  let input = $state<HTMLInputElement>();
  const openPaths = $derived(new Set(app.tabs.map((t) => t.path)));
  const rows = $derived(newTabRows(app.recents, openPaths, query));
  $effect(() => {
    void query;
    active = 0;
  });

  /** Where the menu sits: under the + button, inside the window. */
  let at = $state({ left: 8, top: 44 });
  onMount(() => {
    input?.focus();
    const anchor = document.querySelector<HTMLElement>('[data-testid="new-tab"]');
    if (!anchor) return;
    const r = anchor.getBoundingClientRect();
    const width = Math.min(420, window.innerWidth - 16);
    at = { left: Math.max(8, Math.min(r.left - 6, window.innerWidth - width - 8)), top: r.bottom + 4 };
  });

  function choose(row: NewTabRow | undefined) {
    if (!row) return;
    void afterMenu(close, () => {
      if (row.kind === "open") return openViaPicker();
      if (row.kind === "recent") return openPath(row.recent.path);
      if (row.kind === "mine") return openClone({ way: "mine" });
      openClone(row.link ? { way: "link", link: row.link } : { way: "link" });
    });
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    if (e.key === "Tab") {
      // The field keeps the focus; the rows are its options.
      e.preventDefault();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (!e.repeat) choose(rows[active]);
      return;
    }
    const next = moveTo(active, e.key, rows.length, 6);
    if (next === null || e.key === "Home" || e.key === "End") return;
    e.preventDefault();
    active = next;
    void tick().then(() => listEl?.querySelector(`#${uid}-${next}`)?.scrollIntoView({ block: "nearest" }));
  }

  const rowId = (row: NewTabRow) => (row.kind === "recent" ? `recent:${row.recent.path}` : row.kind);
</script>

{#snippet folderIcon()}
  <svg class="icon" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M1.75 4.25c0-.83.67-1.5 1.5-1.5h3l1.5 1.75h5c.83 0 1.5.67 1.5 1.5v5.75c0 .83-.67 1.5-1.5 1.5h-9.5c-.83 0-1.5-.67-1.5-1.5z"
      fill="none"
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linejoin="round"
    />
  </svg>
{/snippet}

{#snippet cloneIcon()}
  <!-- A copy coming down into a tray: clone. -->
  <svg class="icon" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
    <path d="M8 2v7.2M5.2 6.6 8 9.4l2.8-2.8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
    <path d="M2.5 10.2v1.6c0 .8.6 1.4 1.4 1.4h8.2c.8 0 1.4-.6 1.4-1.4v-1.6" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
  </svg>
{/snippet}

<!-- Over the window, which takes no input while it is open; a press outside closes it. -->
<div
  class="backdrop"
  role="presentation"
  onpointerdown={(e) => {
    if (e.target === e.currentTarget) close();
  }}
>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="menu"
    role="dialog"
    aria-modal="true"
    aria-label="Open a repository"
    tabindex="-1"
    style:left="{at.left}px"
    style:top="{at.top}px"
    data-testid="new-tab-menu"
    onkeydown={onKeydown}
  >
    <div class="query">
      <svg width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="7" cy="7" r="4.6" fill="none" stroke="currentColor" stroke-width="1.5" />
        <path d="M10.5 10.5 14 14" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
      </svg>
      <input
        type="text"
        role="combobox"
        aria-label="Filter the recent repositories, or paste a link to clone"
        aria-expanded="true"
        aria-controls="{uid}-list"
        aria-autocomplete="list"
        aria-activedescendant={rows.length > 0 ? `${uid}-${active}` : undefined}
        placeholder="Recent repositories, or a link to clone"
        autocomplete="off"
        spellcheck="false"
        bind:this={input}
        bind:value={query}
        data-testid="new-tab-filter"
      />
    </div>
    <div class="list" id="{uid}-list" role="listbox" aria-label="Open a repository" tabindex="-1" bind:this={listEl}>
      {#each rows as row, i (rowId(row))}
        {@const firstRecent = row.kind === "recent" && rows[i - 1]?.kind !== "recent"}
        {@const firstClone = row.kind === "clone"}
        {#if firstRecent}<div class="section" role="presentation">Recent</div>{/if}
        {#if firstClone && i > 0}<div class="rule" role="presentation"></div>{/if}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="option"
          id="{uid}-{i}"
          role="option"
          tabindex="-1"
          aria-selected={i === active}
          data-active={i === active}
          data-kind={row.kind}
          data-testid="new-tab-{row.kind}"
          onpointerdown={(e) => e.preventDefault()}
          onpointermove={() => (active = i)}
          onclick={() => choose(row)}
        >
          {#if row.kind === "open"}
            {@render folderIcon()}
            <span class="label">Open folder…</span>
            <kbd>{OPEN_KEYS}</kbd>
          {:else if row.kind === "recent"}
            <span class="dot" style:background="var(--lane-{colorIndex(row.recent.path)})" aria-hidden="true"></span>
            <span class="label name">{visibleText(row.recent.name)}</span>
            <span class="folder path-tail" title={shortenPath(row.recent.path, app.homeDir)}><bdi>{displayFolder(row.recent.path, app.homeDir)}</bdi></span>
            {#if row.open}<span class="chip">In a tab</span>{/if}
          {:else if row.kind === "clone"}
            {@render cloneIcon()}
            {#if row.link}
              <span class="label">Clone <code>{visibleText(row.link)}</code></span>
            {:else}
              <span class="label">Clone from a link…</span>
              <kbd>{CLONE_KEYS}</kbd>
            {/if}
          {:else}
            {@render cloneIcon()}
            <span class="label">Clone one of your repositories…</span>
          {/if}
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
  }

  .menu {
    position: fixed;
    display: flex;
    flex-direction: column;
    width: min(420px, calc(100vw - 16px));
    max-height: calc(100vh - var(--titlebar-h) - 24px);
    padding: 6px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    background: var(--bg-elevated);
    box-shadow: var(--shadow-menu);
    outline: none;
    animation: drop 110ms var(--ease);
  }

  .query {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--field-bg);
    color: var(--text-faint);
  }

  .query:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .query input {
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

  .query input::placeholder {
    color: var(--text-faint);
  }

  .list {
    flex: 0 1 auto;
    min-height: 0;
    max-height: 380px;
    margin-top: 6px;
    overflow-y: auto;
    overscroll-behavior: contain;
    outline: none;
  }

  .section {
    padding: 6px 10px 3px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-weight: 600;
  }

  .rule {
    margin: 5px 6px;
    border-top: 1px solid var(--border);
  }

  .option {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 10px;
    border-radius: var(--radius);
    white-space: nowrap;
  }

  .option[data-active="true"] {
    background: var(--bg-selected);
  }

  .option:active {
    background: var(--bg-pressed);
  }

  .icon {
    flex: none;
    color: var(--text-muted);
  }

  [data-kind="clone"] .icon,
  [data-kind="mine"] .icon {
    color: var(--accent);
  }

  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    margin: 0 3.5px;
    border-radius: 50%;
  }

  .label {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .label code {
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .name {
    font-weight: 550;
  }

  .folder {
    flex: 1 1 0;
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  .chip {
    flex: none;
    padding: 0 6px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 500;
    line-height: 16px;
  }

  kbd {
    flex: none;
    margin-left: auto;
    color: var(--text-faint);
    font-family: var(--font-ui);
    font-size: var(--fs-sm);
  }

  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
</style>
