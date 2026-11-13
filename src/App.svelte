<script lang="ts">
  import { onMount } from "svelte";
  import Mark from "./lib/components/Mark.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import Toast from "./lib/components/Toast.svelte";
  import Welcome from "./lib/components/Welcome.svelte";
  import { colorIndex } from "./lib/format";
  import { api, onFolderDrop } from "./lib/ipc";
  import { isMac } from "./lib/platform";
  import { shortcutFor } from "./lib/shortcuts";
  import { activateIndex, app, closeTab, cycleTab, init, openPath, openViaPicker } from "./lib/state/app.svelte";
  import { trackWindowFocus } from "./lib/windowFocus";

  const active = $derived(app.tabs.find((t) => t.repoId === app.activeRepoId) ?? null);
  const mac = isMac();

  /** Files are being dragged over the window (app only). */
  let dragging = $state(false);

  onMount(() => {
    void init();
    const stopFocus = trackWindowFocus(window, document, (focused) => {
      api.setWindowFocused(focused).catch(() => {
        // Focus only tunes background work; a missed report is harmless.
      });
    });
    const stopDrop = onFolderDrop(async (event) => {
      dragging = event.kind === "enter";
      if (event.kind === "drop") for (const path of event.paths) await openPath(path);
    });
    // The webview's own context menu (Reload, Inspect…) is not the app's; text fields keep theirs.
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
    if (!shortcut || app.gitMissing) return;
    e.preventDefault();
    switch (shortcut.kind) {
      case "open":
        void openViaPicker();
        break;
      case "close":
        if (app.activeRepoId !== null) void closeTab(app.activeRepoId);
        break;
      case "cycle":
        cycleTab(shortcut.delta);
        break;
      case "jump":
        activateIndex(shortcut.index);
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
        <header class="repo-head">
          <span class="dot" style:background="var(--lane-{colorIndex(active.path)})" aria-hidden="true"></span>
          <h1 class="repo-name">{active.name}</h1>
          <span class="repo-path selectable">{active.path}</span>
        </header>
        <div class="repo-body">
          <p class="placeholder">The commit graph will appear here.</p>
        </div>
      </div>
    {:else if app.ready}
      <Welcome />
    {/if}
  </main>

  {#if dragging}
    <div class="drop" aria-hidden="true">
      <p>Drop a folder to open it</p>
    </div>
  {/if}

  <Toast />
</div>

<style>
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
    display: flex;
    align-items: baseline;
    gap: 8px;
    height: 34px;
    padding: 0 14px;
    line-height: 34px;
    border-bottom: 1px solid var(--border);
    flex: none;
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
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .repo-body {
    display: grid;
    place-items: center;
    flex: 1;
    min-height: 0;
  }

  .placeholder {
    color: var(--text-faint);
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
