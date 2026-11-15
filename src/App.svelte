<script lang="ts">
  import { onMount } from "svelte";
  import CommandLog from "./lib/components/CommandLog.svelte";
  import CommitDetails from "./lib/components/CommitDetails.svelte";
  import ConflictBanner from "./lib/components/ConflictBanner.svelte";
  import ContextMenu from "./lib/components/ContextMenu.svelte";
  import Mark from "./lib/components/Mark.svelte";
  import SidePanel from "./lib/components/SidePanel.svelte";
  import WorktreePanel from "./lib/components/WorktreePanel.svelte";
  import Toolbar from "./lib/components/Toolbar.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import Toast from "./lib/components/Toast.svelte";
  import Welcome from "./lib/components/Welcome.svelte";
  import { detailsRoom, panelRoom } from "./lib/detailsLayout";
  import { colorIndex, shortHash, shortenPath, visibleText } from "./lib/format";
  import GraphView from "./lib/graph/GraphView.svelte";
  import { api, onEvent, onFolderDrop } from "./lib/ipc";
  import { isMac } from "./lib/platform";
  import { repeatable, shortcutFor } from "./lib/shortcuts";
  import {
    activateIndex,
    app,
    closeActiveTab,
    cycleTab,
    init,
    openPath,
    openViaPicker,
    focusNewestNotice,
    toast,
    windowFocusChanged,
  } from "./lib/state/app.svelte";
  import { openCliDialog } from "./lib/state/cli.svelte";
  import { logPrefs, setLogOpen } from "./lib/state/commandLog.svelte";
  import { overlays } from "./lib/state/overlays.svelte";
  import { repoView } from "./lib/state/repo.svelte";
  import {
    addWorktree,
    docked,
    openPanelMenu,
    panelShown,
    setPanelShown,
    sidePanel,
  } from "./lib/state/worktrees.svelte";
  import { trackWindowFocus } from "./lib/windowFocus";

  const active = $derived(app.tabs.find((t) => t.repoId === app.activeRepoId) ?? null);
  const mac = isMac();
  const PANEL_KEYS = mac ? "⌘B" : "Ctrl+B";

  /** The active tab's active worktree, as its snapshot has it. */
  const activeWorktree = $derived.by(() => {
    if (!active) return null;
    const snap = repoView(active.repoId).snapshot;
    return { path: snap.activeWorktree, branch: snap.head.branch, hash: snap.head.hash, count: snap.worktrees.length };
  });

  /**
   * F6 (Shift+F6 back) moves between the window's regions: the graph, the newest notice, the
   * side panel. From outside them it goes to the newest notice first (R30), else the graph.
   */
  function cycleRegion(delta: 1 | -1) {
    const graph = document.querySelector<HTMLElement>('#repo-panel [role="grid"]');
    const notices = document.querySelectorAll<HTMLElement>("[data-notices] .toast");
    const notice = notices[notices.length - 1]?.querySelector<HTMLElement>("button") ?? null;
    const panel = document.querySelector<HTMLElement>('[data-region="side-panel"] [role="listbox"]');
    const regions = [graph, notice, panel];
    const focused = document.activeElement;
    const at = regions.findIndex(
      (el, i) =>
        el !== null &&
        focused !== null &&
        (i === 1 ? !!focused.closest("[data-notices]") : i === 2 ? !!focused.closest('[data-region="side-panel"]') : el.contains(focused)),
    );
    if (at < 0) {
      if (!focusNewestNotice()) graph?.focus();
      return;
    }
    for (let step = 1; step < regions.length; step++) {
      const next = regions[(at + delta * step + regions.length * 2) % regions.length];
      if (next) {
        next.focus();
        return;
      }
    }
  }

  /** Shows the side panel with the focus in its list (the header's worktree button). */
  function showWorktrees() {
    if (active) setPanelShown(active.repoId, true, true);
  }

  /** Files are being dragged over the window (app only). */
  let dragging = $state(false);

  // The bottom panels (details, command log) share what the graph leaves: it keeps
  // MIN_GRAPH_H under the conflict banner. The details panel goes first; the log takes what
  // is left (its minimum is kept for it while open).
  // Measured to the fraction of a pixel, so the graph's minimum holds exactly.
  let bodyRect = $state<DOMRectReadOnly>();
  let bannerRect = $state<DOMRectReadOnly>();

  /**
   * Calls `set` with the element's content box whenever it changes size, in the next
   * animation frame rather than inside the ResizeObserver's delivery: the panels' sizes
   * follow from it, and changing them while the observers deliver (the banner appearing over
   * the details and the log) is a ResizeObserver loop.
   */
  function measured(node: HTMLElement, set: (rect: DOMRectReadOnly) => void) {
    let frame = 0;
    let latest: DOMRectReadOnly | null = null;
    const observer = new ResizeObserver((entries) => {
      latest = entries[entries.length - 1].contentRect;
      frame ||= requestAnimationFrame(() => {
        frame = 0;
        if (latest) set(latest);
      });
    });
    observer.observe(node);
    set(node.getBoundingClientRect());
    return {
      destroy() {
        observer.disconnect();
        cancelAnimationFrame(frame);
      },
    };
  }
  const bodyH = $derived(bodyRect?.height ?? Infinity);
  const bannerH = $derived(bannerRect?.height ?? 0);
  let detailsShown = $state(0);

  onMount(() => {
    void init();
    const stopFocus = trackWindowFocus(window, document, windowFocusChanged);
    const stopDrop = onFolderDrop(async (event) => {
      dragging = event.kind === "enter";
      if (event.kind === "drop") for (const path of event.paths) await openPath(path);
    });
    // The app menu's "Install ‘gg’ Command…" (macOS).
    const stopCliMenu = onEvent("menu-install-cli", () => void openCliDialog());
    // The webview's own context menu (Reload, Inspect…) is not the app's; text fields keep
    // theirs. The graph opens the app's own menus on its rows (GraphView), before this.
    const onContextMenu = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      if (!target?.closest("input, textarea, [contenteditable]")) e.preventDefault();
    };
    document.addEventListener("contextmenu", onContextMenu);
    return () => {
      stopFocus();
      stopDrop();
      stopCliMenu();
      document.removeEventListener("contextmenu", onContextMenu);
    };
  });

  function onKeydown(e: KeyboardEvent) {
    const shortcut = shortcutFor(e, mac);
    if (!shortcut) return;
    // Always: Cmd/Ctrl+R must never reload the page, nor Cmd/Ctrl+F open the browser's find.
    e.preventDefault();
    // Holding Cmd+W must not close one tab after another.
    if (e.repeat && !repeatable(shortcut)) return;
    // A dialog is modal: nothing behind it changes while it is open.
    if (overlays.dialog) return;
    if (app.gitMissing && shortcut.kind !== "close") return;
    const view = active ? repoView(active.repoId) : null;
    switch (shortcut.kind) {
      case "open":
        void openViaPicker();
        break;
      case "close":
        // With no tab left, Cmd+W closes the window, as in other tabbed apps.
        if (app.activeRepoId !== null) void closeActiveTab();
        else void api.closeWindow().catch(() => toast("error", "Could not close the window."));
        break;
      case "cycle":
        cycleTab(shortcut.delta);
        break;
      case "jump":
        activateIndex(shortcut.index);
        break;
      case "search":
        view?.search.show();
        break;
      case "fetch":
        void view?.fetch();
        break;
      case "refresh":
        void view?.refresh();
        break;
      case "regions":
        cycleRegion(shortcut.delta);
        break;
      case "sidePanel":
        if (active) setPanelShown(active.repoId);
        break;
      case "log":
        if (active) setLogOpen(!logPrefs.open, true);
        break;
    }
  }
</script>

<svelte:window onkeydown={onKeydown} bind:innerWidth={sidePanel.windowW} />

<div class="shell" data-testid="app-root">
  <TabBar />

  <main class="content">
    {#if app.gitMissing}
      <section class="notice" role="alert">
        <Mark size={36} />
        <p class="notice-text">Git was not found. Install git and restart.</p>
      </section>
    {:else if active}
      <div class="repo" id="repo-panel" role="tabpanel" aria-labelledby="tab-{active.repoId}" data-testid="repo-view">
        {#key active.repoId}
          <header class="repo-head">
            <div class="identity">
              <span class="dot" style:background="var(--lane-{colorIndex(active.path)})" aria-hidden="true"></span>
              <h1 class="repo-name">{active.name}</h1>
              {#if activeWorktree}
                <!-- Where every action runs: the active worktree, always in sight. -->
                <button
                  class="worktree"
                  data-testid="active-worktree"
                  title="Active worktree: {shortenPath(activeWorktree.path || active.path, app.homeDir)}{activeWorktree.count > 1
                    ? ` (one of ${activeWorktree.count})`
                    : ''}. Show the worktrees ({PANEL_KEYS})"
                  onclick={showWorktrees}
                >
                  <svg class="ring" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
                    <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.7" />
                  </svg>
                  <bdi class="wt-branch"
                    >{activeWorktree.branch !== null
                      ? visibleText(activeWorktree.branch)
                      : activeWorktree.hash
                        ? `${shortHash(activeWorktree.hash)} (detached)`
                        : "no commits"}</bdi
                  >
                  <span class="repo-path path-tail"
                    ><bdi>{shortenPath(activeWorktree.path || active.path, app.homeDir)}</bdi></span
                  >
                </button>
              {/if}
            </div>
            <Toolbar view={repoView(active.repoId)} />
          </header>
          <div class="repo-body">
            <div class="main-column" use:measured={(r) => (bodyRect = r)}>
            <!-- The banner takes its height from the graph's top (the graph keeps its rows in place). -->
            <div class="graph-column">
              <div class="banner-slot" use:measured={(r) => (bannerRect = r)}>
                <ConflictBanner view={repoView(active.repoId)} compact={detailsShown > 0 || logPrefs.open} />
              </div>
              <div class="graph-area">
                <GraphView view={repoView(active.repoId)} />
              </div>
            </div>
            <CommitDetails
              view={repoView(active.repoId)}
              hostH={bodyH}
              room={detailsRoom(panelRoom(bodyH, bannerH, 0), logPrefs.open)}
              bind:shown={detailsShown}
            />
            {#if logPrefs.open}
              <CommandLog
                repoId={active.repoId}
                hostH={bodyH}
                room={panelRoom(bodyH, bannerH, detailsShown)}
                onclose={() => setLogOpen(false)}
                detailsHidden={repoView(active.repoId).selectedHash !== null &&
                  detailsRoom(panelRoom(bodyH, bannerH, 0), true) === 0}
              />
            {/if}
            </div>
            {#if panelShown(active.repoId)}
              {@const view = repoView(active.repoId)}
              <SidePanel
                title="Worktrees"
                count={view.snapshot.worktrees.length > 1 ? String(view.snapshot.worktrees.length) : null}
                overlay={!docked()}
                onhide={() => {
                  setPanelShown(active.repoId, false);
                  document.querySelector<HTMLElement>('#repo-panel [role="grid"]')?.focus();
                }}
              >
                {#snippet actions()}
                  <button
                    class="icon-btn small"
                    aria-label="Add worktree"
                    title="Add worktree…"
                    disabled={view.snapshot.head.hash === null}
                    onclick={() => addWorktree(view.snapshot)}
                  >
                    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                      <path d="M7 2.6v8.8M2.6 7h8.8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
                    </svg>
                  </button>
                  <button
                    class="icon-btn small"
                    aria-label="Worktree actions"
                    title="More"
                    onclick={(e) => {
                      const r = e.currentTarget.getBoundingClientRect();
                      openPanelMenu(view, { x: r.left, y: r.bottom + 2 }, e.detail === 0);
                    }}
                  >
                    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                      <circle cx="3.2" cy="7" r="1.1" fill="currentColor" />
                      <circle cx="7" cy="7" r="1.1" fill="currentColor" />
                      <circle cx="10.8" cy="7" r="1.1" fill="currentColor" />
                    </svg>
                  </button>
                {/snippet}
                <WorktreePanel {view} />
              </SidePanel>
            {/if}
          </div>
        {/key}
      </div>
    {:else if app.ready && !app.restoring}
      <Welcome />
    {/if}
  </main>

  {#if dragging}
    <div class="drop" aria-hidden="true">
      <p>Drop a folder to open it</p>
    </div>
  {/if}

  <Toast />
  <ContextMenu />
  {#each overlays.dialogs as dialog (dialog.id)}
    <!-- Only the top dialog takes input; those under it are inert. -->
    <div class="dialog-layer" inert={dialog !== overlays.dialog}>
      <dialog.component {...dialog.props} />
    </div>
  {/each}
</div>

<style>
  .dialog-layer {
    display: contents;
  }

  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    /* Nothing scrolls the window sideways, not even focus moving to something cut off. */
    overflow: clip;
  }

  .content {
    flex: 1;
    min-height: 0;
    position: relative;
  }

  .notice {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
    height: 100%;
    padding: 24px;
    text-align: center;
  }

  .notice-text {
    font-size: 15px;
    font-weight: 550;
  }

  .repo {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .repo-head {
    /* The toolbar's loading line runs along this header's bottom edge. */
    position: relative;
    display: flex;
    align-items: center;
    gap: 14px;
    height: 40px;
    padding: 0 8px 0 14px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }

  .identity {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    max-width: 38%;
    flex: 0 1 auto;
  }

  .dot {
    align-self: center;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }

  .repo-name {
    flex: 0 1 auto;
    min-width: 48px;
    overflow: hidden;
    font-size: var(--fs);
    font-weight: 600;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .repo-path {
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  /* The active worktree: its branch with the HEAD ring, then its folder; opens the panel. */
  .worktree {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: 0 1 auto;
    min-width: 0;
    height: 24px;
    margin-left: 2px;
    padding: 0 8px 0 7px;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    font-size: var(--fs-sm);
    transition: background-color 80ms linear;
  }

  .worktree:hover {
    background: var(--bg-hover);
  }

  .worktree:active {
    background: var(--bg-pressed);
  }

  .ring {
    flex: none;
    color: var(--accent);
  }

  .wt-branch {
    flex: 0 1 auto;
    min-width: 40px;
    max-width: 180px;
    overflow: hidden;
    color: var(--text);
    font-weight: 600;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  /* The graph and the panels docked below it on the left, the side panel on the right (or
     over the graph, in a narrow window). */
  .repo-body {
    position: relative;
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* The graph above, the details panel docked below it (when a commit is selected). */
  .main-column {
    display: flex;
    flex-direction: column;
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
  }

  /* The bottom panels are sized so the graph's rows keep MIN_GRAPH_H under the banner
     (detailsLayout.ts); the column clips rather than push them out of the window. */
  .graph-column {
    display: flex;
    flex-direction: column;
    flex: 1 1 0;
    min-height: 0;
    overflow: clip;
  }

  .banner-slot {
    flex: none;
  }

  .graph-area {
    flex: 1 1 0;
    min-height: 0;
  }

  .drop {
    position: fixed;
    inset: calc(var(--titlebar-h) + 10px) 10px 10px;
    z-index: 40;
    display: grid;
    place-items: center;
    border: 2px dashed var(--accent);
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--bg) 82%, transparent);
    color: var(--accent);
    font-size: 15px;
    font-weight: 600;
    pointer-events: none;
  }
</style>
