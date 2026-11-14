<script lang="ts">
  import { onMount } from "svelte";
  import CommandLog from "./lib/components/CommandLog.svelte";
  import CommitDetails from "./lib/components/CommitDetails.svelte";
  import ConflictBanner from "./lib/components/ConflictBanner.svelte";
  import ContextMenu from "./lib/components/ContextMenu.svelte";
  import Mark from "./lib/components/Mark.svelte";
  import Toolbar from "./lib/components/Toolbar.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import Toast from "./lib/components/Toast.svelte";
  import Welcome from "./lib/components/Welcome.svelte";
  import { detailsRoom, panelRoom } from "./lib/detailsLayout";
  import { colorIndex, shortenPath } from "./lib/format";
  import GraphView from "./lib/graph/GraphView.svelte";
  import { api, onFolderDrop } from "./lib/ipc";
  import { isMac } from "./lib/platform";
  import { repeatable, shortcutFor } from "./lib/shortcuts";
  import {
    activateIndex,
    app,
    closeTab,
    cycleTab,
    init,
    openPath,
    openViaPicker,
    focusNewestNotice,
    toast,
    windowFocusChanged,
  } from "./lib/state/app.svelte";
  import { logPrefs, setLogOpen } from "./lib/state/commandLog.svelte";
  import { overlays } from "./lib/state/overlays.svelte";
  import { repoView } from "./lib/state/repo.svelte";
  import { trackWindowFocus } from "./lib/windowFocus";

  const active = $derived(app.tabs.find((t) => t.repoId === app.activeRepoId) ?? null);
  const mac = isMac();

  /** Files are being dragged over the window (app only). */
  let dragging = $state(false);

  // The bottom panels (details, command log) share what the graph leaves: it keeps
  // MIN_GRAPH_H under the conflict banner. The details panel goes first; the log takes what
  // is left (its minimum is kept for it while open).
  // Measured to the fraction of a pixel, so the graph's minimum holds exactly.
  let bodyRect = $state<DOMRectReadOnly>();
  let bannerRect = $state<DOMRectReadOnly>();
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
        if (app.activeRepoId !== null) void closeTab(app.activeRepoId);
        else void api.closeWindow().catch(() => toast("error", "Could not close the window"));
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
      case "notices":
        focusNewestNotice();
        break;
      case "log":
        if (active) setLogOpen(!logPrefs.open, true);
        break;
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

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
              <span class="repo-path selectable path-tail" title={shortenPath(active.path, app.homeDir)}
                ><bdi>{shortenPath(active.path, app.homeDir)}</bdi></span
              >
            </div>
            <Toolbar view={repoView(active.repoId)} />
          </header>
          <div class="repo-body" bind:contentRect={bodyRect}>
            <!-- The banner takes its height from the graph's top (the graph keeps its rows in place). -->
            <div class="graph-column">
              <div class="banner-slot" bind:contentRect={bannerRect}>
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
              />
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
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    max-width: 34%;
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
    font-size: var(--fs);
    font-weight: 600;
    white-space: nowrap;
  }

  .repo-path {
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  /* The graph above, the details panel docked below it (when a commit is selected). */
  .repo-body {
    display: flex;
    flex-direction: column;
    flex: 1;
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
