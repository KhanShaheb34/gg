<!--
  The context menu (openMenu in state/overlays): one panel at the pointer, kept inside the
  window, and a panel per open submenu beside its item. It closes on Escape, a click
  elsewhere, a scroll of what it was opened on (another list scrolling by itself, like the
  command log following new commands, leaves it), the window losing focus, or its repository
  changing under it (new rows, an operation starting or ending): its items may no longer
  apply. Focus then goes back
  where it was.

  Keys act on the deepest open panel: arrows, Home and End move; typing jumps to the next
  item starting with what was typed; Enter or Space picks; ArrowRight opens a submenu and
  ArrowLeft (or Escape) closes it. Submenus also open on hover. An item that cannot apply is
  shown faint, with the reason beside it while it is hovered or active.
-->
<script lang="ts">
  import { tick } from "svelte";
  import type { MenuItem } from "../menus/types";
  import { closeMenu, overlays } from "../state/overlays.svelte";
  import { findRepoView } from "../state/repo.svelte";

  type Item = Extract<MenuItem, { id: string }>;
  /** Where a panel goes, worked out once its size is known. */
  type Anchor = { kind: "point"; x: number; y: number } | { kind: "item"; rect: DOMRect };
  type Level = { items: MenuItem[]; anchor: Anchor; active: number; parent: number };

  /** Gap kept between a panel and the window's edges, px. */
  const EDGE = 6;
  /** Hovering an item this long opens (or closes) a submenu. */
  const HOVER_MS = 140;
  /** Typed letters are searched for together within this time. */
  const TYPEAHEAD_MS = 600;
  /**
   * Scrolling closes the menu, but not the scroll that opening it caused: the right-click
   * selects the row, the details panel opens, and the graph keeps the row in view.
   */
  const SETTLE_MS = 300;

  let levels: Level[] = $state([]);
  let panels: HTMLElement[] = $state([]);
  let shownId = 0;
  let openedAt = 0;
  /** The element the menu was opened over (under its point), if any. */
  let openedOver: Element | null = null;
  let hoverTimer: ReturnType<typeof setTimeout> | null = null;
  let typed = "";
  let typedAt = 0;
  /** Where the reason of an active, disabled item shows. */
  let why: { text: string; left: number; top: number } | null = $state(null);

  const isItem = (it: MenuItem | undefined): it is Item => it !== undefined && !("separator" in it);

  // A new menu: start over with its items.
  $effect(() => {
    const menu = overlays.menu;
    if (!menu) {
      levels = [];
      why = null;
      clearHover();
      return;
    }
    if (menu.id === shownId) return;
    shownId = menu.id;
    openedAt = performance.now();
    // Before the panel is drawn: what the menu was opened on.
    openedOver = document.elementFromPoint(menu.x, menu.y);
    typed = "";
    clearHover();
    levels = [
      {
        items: menu.items,
        anchor: { kind: "point", x: menu.x, y: menu.y },
        active: menu.keyboard ? next(menu.items, -1, 1) : -1,
        parent: -1,
      },
    ];
    void tick().then(() => focusActive());
  });

  /** The menu's repository as it bears on the menu: its rows and its running operation. */
  function stateOf(repoId: number | null): { generation: number; op: number } | null {
    const snap = repoId === null ? undefined : findRepoView(repoId)?.snapshot;
    return snap ? { generation: snap.generation, op: snap.op?.id ?? 0 } : null;
  }

  // An operation starting or ending closes the menu; new rows close it if its target went.
  let openedOn: { id: number; state: { generation: number; op: number } | null } | null = null;
  /** The generation of rows the open menu has found its target still in (tests wait for it). */
  let checkedGeneration: number | null = $state(null);
  $effect(() => {
    const menu = overlays.menu;
    if (!menu) return;
    const state = stateOf(menu.repoId);
    if (openedOn?.id !== menu.id) {
      openedOn = { id: menu.id, state };
      checkedGeneration = state?.generation ?? null;
      return;
    }
    const before = openedOn.state;
    openedOn.state = state;
    if (before === null || state === null) {
      if (before !== state) closeMenu();
      return;
    }
    if (state.op !== before.op) closeMenu();
    else if (state.generation !== before.generation) {
      if (!menu.alive) {
        checkedGeneration = state.generation;
        return;
      }
      void menu.alive().then((alive) => {
        if (overlays.menu?.id !== menu.id) return;
        if (alive) checkedGeneration = state.generation;
        else closeMenu();
      });
    }
  });

  // The reason of the deepest active item that cannot apply, beside its panel.
  $effect(() => {
    why = null;
    for (let l = levels.length - 1; l >= 0; l--) {
      const it = levels[l].items[levels[l].active];
      if (!isItem(it)) continue;
      if (!it.disabledReason) break;
      const el = panels[l]?.querySelector<HTMLElement>(`[data-index="${levels[l].active}"]`);
      const panel = panels[l]?.getBoundingClientRect();
      if (!el || !panel) break;
      const r = el.getBoundingClientRect();
      const width = Math.min(280, window.innerWidth - 2 * EDGE);
      const right = panel.right + 6;
      const left = right + width <= window.innerWidth - EDGE ? right : Math.max(EDGE, panel.left - 6 - width);
      why = { text: it.disabledReason, left, top: r.top };
      break;
    }
  });

  /** The index of the next item after `from` in direction `dir` (wrapping), skipping separators. */
  function next(items: MenuItem[], from: number, dir: 1 | -1): number {
    const n = items.length;
    for (let k = 1; k <= n; k++) {
      const i = (((from + dir * k) % n) + n) % n;
      if (isItem(items[i])) return i;
    }
    return -1;
  }

  function deepest(): Level {
    return levels[levels.length - 1];
  }

  function setActive(l: number, index: number) {
    levels[l].active = index;
  }

  function focusActive() {
    const l = levels.length - 1;
    const panel = panels[l];
    if (!panel) return;
    const el = panel.querySelector<HTMLElement>(`[data-index="${levels[l].active}"]`);
    (el ?? panel).focus({ preventScroll: true });
    // A panel taller than the window scrolls: keep the active item in it.
    el?.scrollIntoView({ block: "nearest" });
  }

  function clearHover() {
    if (hoverTimer !== null) clearTimeout(hoverTimer);
    hoverTimer = null;
  }

  function openSubmenu(l: number, index: number, fromKeyboard: boolean) {
    const it = levels[l].items[index];
    if (!isItem(it) || !it.submenu || it.disabledReason) return;
    const el = panels[l]?.querySelector<HTMLElement>(`[data-index="${index}"]`);
    if (!el) return;
    levels = [
      ...levels.slice(0, l + 1),
      {
        items: it.submenu,
        anchor: { kind: "item", rect: el.getBoundingClientRect() },
        active: fromKeyboard ? next(it.submenu, -1, 1) : -1,
        parent: index,
      },
    ];
    if (fromKeyboard) void tick().then(focusActive);
  }

  function closeDeeper(l: number) {
    if (levels.length > l + 1) levels = levels.slice(0, l + 1);
  }

  function pick(l: number, index: number, fromKeyboard: boolean) {
    const it = levels[l]?.items[index];
    if (!isItem(it) || it.disabledReason) return;
    if (it.submenu) {
      openSubmenu(l, index, fromKeyboard);
      return;
    }
    const menu = overlays.menu;
    closeMenu(true);
    menu?.onPick(it.id);
  }

  function onHover(l: number, index: number) {
    if (levels[l].active === index && (levels.length === l + 1 || levels[l + 1].parent === index)) return;
    setActive(l, index);
    clearHover();
    const it = levels[l].items[index];
    const opensSubmenu = isItem(it) && !!it.submenu && !it.disabledReason;
    if (levels.length > l + 1 || opensSubmenu) {
      hoverTimer = setTimeout(() => {
        hoverTimer = null;
        if (opensSubmenu) openSubmenu(l, index, false);
        else closeDeeper(l);
      }, HOVER_MS);
    }
    panels[l]?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.focus({ preventScroll: true });
  }

  function onLeave(l: number) {
    // Off the panel: nothing is active there, unless it holds the open submenu.
    if (levels.length === l + 1) {
      clearHover();
      setActive(l, -1);
      panels[l]?.focus({ preventScroll: true });
    }
  }

  function typeAhead(key: string) {
    const now = performance.now();
    typed = now - typedAt > TYPEAHEAD_MS ? key : typed + key;
    typedAt = now;
    const level = deepest();
    const n = level.items.length;
    // A repeated single letter moves on; more letters search from the active item.
    const start = typed.length === 1 ? level.active + 1 : Math.max(level.active, 0);
    for (let k = 0; k < n; k++) {
      const i = (start + k) % n;
      const it = level.items[i];
      if (isItem(it) && it.label.toLowerCase().startsWith(typed)) {
        setActive(levels.length - 1, i);
        void tick().then(focusActive);
        return;
      }
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (!overlays.menu || levels.length === 0 || overlays.dialog) return;
    if (e.metaKey || e.ctrlKey || e.altKey) {
      // A shortcut: the menu goes, the shortcut does what it does.
      closeMenu(false);
      return;
    }
    e.stopPropagation();
    const l = levels.length - 1;
    const level = deepest();
    // Typed letters search together only while nothing else is pressed in between.
    const typing = (e.key.length === 1 && e.key !== " ") || (e.key === " " && typed !== "");
    if (!typing) typed = "";
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp":
        setActive(l, next(level.items, level.active, e.key === "ArrowDown" ? 1 : -1));
        break;
      case "Home":
        setActive(l, next(level.items, -1, 1));
        break;
      case "End":
        setActive(l, next(level.items, 0, -1));
        break;
      case "ArrowRight":
        openSubmenu(l, level.active, true);
        break;
      case "ArrowLeft":
        if (l > 0) closeDeeper(l - 1);
        break;
      case "Escape":
        if (l > 0) closeDeeper(l - 1);
        else closeMenu(true);
        break;
      case "Enter":
      case " ":
        // A space within typed letters is one of them ("copy h").
        if (e.key === " " && typing && performance.now() - typedAt < TYPEAHEAD_MS) typeAhead(" ");
        else pick(l, level.active, true);
        break;
      case "Tab":
        closeMenu(true);
        break;
      default:
        if (e.key.length === 1) typeAhead(e.key.toLowerCase());
        else return; // not a key of the menu's
    }
    e.preventDefault();
    void tick().then(focusActive);
  }

  /** A click or scroll outside every panel closes the menu. */
  function outside(e: Event): boolean {
    const target = e.target as Node | null;
    return !panels.some((p) => p?.contains(target));
  }

  /**
   * The scroll moved what the menu was opened on (the graph, the page): another list
   * scrolling by itself (the command log following new commands) leaves the menu open.
   */
  function movesWhatItIsOn(e: Event): boolean {
    const target = e.target;
    if (!(target instanceof Element) || openedOver === null) return true;
    return target.contains(openedOver);
  }

  /**
   * Puts a panel where its anchor says, inside the window: at the point (or to its left
   * when there is no room on the right), or beside its item (left of the parent when there
   * is no room on the right). Runs before the panel is painted.
   */
  function place(node: HTMLElement, anchor: Anchor) {
    const { width, height } = node.getBoundingClientRect();
    const maxX = window.innerWidth - EDGE;
    const maxY = window.innerHeight - EDGE;
    let left: number;
    let top: number;
    if (anchor.kind === "point") {
      left = anchor.x + width <= maxX ? anchor.x : anchor.x - width;
      top = anchor.y + height <= maxY ? anchor.y : maxY - height;
    } else {
      const r = anchor.rect;
      left = r.right + 2 + width <= maxX ? r.right + 2 : r.left - 2 - width;
      top = r.top - 5; // the first item level with the parent item
      if (top + height > maxY) top = maxY - height;
    }
    node.style.left = `${Math.round(Math.max(EDGE, Math.min(left, maxX - width)))}px`;
    node.style.top = `${Math.round(Math.max(EDGE, top))}px`;
  }
</script>

<svelte:window
  onkeydowncapture={onKeydown}
  onpointerdowncapture={(e) => overlays.menu && outside(e) && closeMenu(false)}
  onscrollcapture={(e) =>
    overlays.menu && outside(e) && movesWhatItIsOn(e) && performance.now() - openedAt > SETTLE_MS && closeMenu(false)}
  onwheelcapture={(e) => overlays.menu && outside(e) && closeMenu(false)}
  onblur={() => closeMenu(false)}
  onresize={() => closeMenu(false)}
/>

{#each levels as level, l (level)}
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    aria-label={l === 0 ? "Actions" : undefined}
    data-testid="context-menu"
    data-checked-generation={l === 0 ? checkedGeneration : undefined}
    bind:this={panels[l]}
    use:place={level.anchor}
    onpointerleave={() => onLeave(l)}
    oncontextmenu={(e) => e.preventDefault()}
  >
    {#each level.items as it, i (i)}
      {#if isItem(it)}
        {@const open = levels[l + 1]?.parent === i}
        <div
          class="item"
          class:active={level.active === i || open}
          class:danger={it.danger}
          role="menuitem"
          tabindex="-1"
          data-index={i}
          data-id={it.id}
          aria-disabled={it.disabledReason ? "true" : undefined}
          aria-haspopup={it.submenu ? "menu" : undefined}
          aria-expanded={it.submenu ? open : undefined}
          aria-describedby={it.disabledReason && level.active === i ? "menu-why" : undefined}
          onpointermove={() => onHover(l, i)}
          onclick={() => pick(l, i, false)}
          onkeydown={() => {}}
        >
          <span class="label">{it.label}</span>
          {#if it.submenu}
            <svg class="chevron" width="7" height="10" viewBox="0 0 7 10" aria-hidden="true">
              <path d="M1.5 1.5 5 5 1.5 8.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          {/if}
        </div>
      {:else}
        <div class="separator" role="separator"></div>
      {/if}
    {/each}
  </div>
{/each}

{#if why}
  <div class="why" id="menu-why" role="tooltip" style:left="{why.left}px" style:top="{why.top}px">{why.text}</div>
{/if}

<style>
  .menu {
    position: fixed;
    top: 0;
    left: 0;
    z-index: 70;
    min-width: 210px;
    max-width: 360px;
    /* Taller than the window: it scrolls inside (place() keeps it within the edges). */
    max-height: calc(100vh - 12px);
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 5px;
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    background: var(--bg-elevated);
    box-shadow: var(--shadow-menu);
    font-size: var(--fs);
    outline: none;
    animation: appear 70ms linear;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 26px;
    padding: 0 9px 0 10px;
    border-radius: 5px;
    color: var(--text);
    white-space: nowrap;
    outline: none;
  }

  .label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .chevron {
    flex: none;
    color: var(--text-muted);
  }

  .item.active {
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .item.active .chevron {
    color: inherit;
  }

  .danger {
    color: var(--danger);
  }

  .danger.active {
    background: var(--danger-fill);
    color: var(--danger-contrast);
  }

  .item[aria-disabled="true"] {
    color: var(--text-faint);
    opacity: 0.75;
  }

  .item[aria-disabled="true"].active {
    background: var(--bg-hover);
    color: var(--text-faint);
  }

  .separator {
    height: 1px;
    margin: 4px 9px;
    background: var(--border);
  }

  .why {
    position: fixed;
    z-index: 71;
    max-width: 280px;
    padding: 4px 8px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    background: var(--bg-elevated);
    box-shadow: var(--shadow-pop);
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 18px;
    pointer-events: none;
    animation: appear 90ms linear;
  }

  @keyframes appear {
    from {
      opacity: 0;
    }
  }
</style>
