<!--
  The window's top bar: one tab per open repository, a button to open another, and the theme
  toggle. On macOS in the app it doubles as the title bar: empty space drags the window and
  the traffic lights sit in the left inset.
-->
<script lang="ts">
  import { tick } from "svelte";
  import { colorIndex, shortenPath } from "../format";
  import { isMac, isTauri } from "../platform";
  import { activate, app, closeTab, cycleTheme, openViaPicker } from "../state/app.svelte";

  const trafficLights = isTauri() && isMac();
  const openLabel = `Open repository (${isMac() ? "⌘O" : "Ctrl+O"})`;

  const themeName = $derived(
    app.settings.theme === "system" ? "System" : app.settings.theme === "light" ? "Light" : "Dark",
  );

  let list: HTMLElement | undefined = $state();

  async function focusTab(repoId: number) {
    await tick();
    list?.querySelector<HTMLElement>(`[data-repo-id="${repoId}"]`)?.focus();
  }

  function onTabKeydown(e: KeyboardEvent, index: number) {
    const count = app.tabs.length;
    if (e.key === "Delete") {
      // Closes the tab, as the ARIA tabs pattern has it; the focus goes to the one selected next.
      e.preventDefault();
      void closeTab(app.tabs[index].repoId).then(() => {
        if (app.activeRepoId !== null) void focusTab(app.activeRepoId);
      });
      return;
    }
    let target: number;
    if (e.key === "ArrowRight") target = (index + 1) % count;
    else if (e.key === "ArrowLeft") target = (index - 1 + count) % count;
    else if (e.key === "Home") target = 0;
    else if (e.key === "End") target = count - 1;
    else return;
    e.preventDefault();
    const repoId = app.tabs[target].repoId;
    activate(repoId);
    void focusTab(repoId);
  }

  // A tab selected some other way (Ctrl+Tab, Cmd+1) takes the focus along if a tab had it.
  $effect(() => {
    const repoId = app.activeRepoId;
    if (repoId !== null && list?.contains(document.activeElement)) void focusTab(repoId);
  });

  function onAuxClick(e: MouseEvent, repoId: number) {
    // Middle-click closes, as in browsers.
    if (e.button === 1) {
      e.preventDefault();
      void closeTab(repoId);
    }
  }
</script>

<header class="bar" class:traffic-lights={trafficLights} data-tauri-drag-region>
  {#if app.tabs.length > 0}
    <div class="tabs" role="tablist" aria-label="Open repositories" bind:this={list}>
      {#each app.tabs as tab, i (tab.repoId)}
        {@const selected = tab.repoId === app.activeRepoId}
        <!-- The list holds only tabs: the close button is for the mouse (keys: Delete, ⌘W). -->
        <div class="tab" class:selected role="presentation" title={shortenPath(tab.path, app.homeDir)}>
          <button
            class="tab-main"
            role="tab"
            id="tab-{tab.repoId}"
            data-repo-id={tab.repoId}
            aria-selected={selected}
            aria-controls="repo-panel"
            tabindex={selected ? 0 : -1}
            onclick={() => activate(tab.repoId)}
            onkeydown={(e) => onTabKeydown(e, i)}
            onauxclick={(e) => onAuxClick(e, tab.repoId)}
          >
            <span class="dot" style:background="var(--lane-{colorIndex(tab.path)})" aria-hidden="true"></span>
            <span class="name">{tab.name}</span>
          </button>
          <button
            class="close"
            tabindex="-1"
            aria-hidden="true"
            title="Close ({isMac() ? '⌘W' : 'Ctrl+W'})"
            onclick={() => closeTab(tab.repoId)}
          >
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
            </svg>
          </button>
        </div>
      {/each}
    </div>
  {/if}

  <button class="icon-btn" aria-label="Open repository" title={openLabel} onclick={openViaPicker}>
    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <path d="M7 2v10M2 7h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
    </svg>
  </button>

  <div class="drag" data-tauri-drag-region></div>

  <button
    class="icon-btn"
    aria-label="Theme: {themeName}"
    title="Theme: {themeName} (click to change)"
    onclick={cycleTheme}
  >
    {#if app.settings.theme === "light"}
      <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="3" fill="none" stroke="currentColor" stroke-width="1.4" />
        <path
          d="M8 1.5v1.6M8 12.9v1.6M1.5 8h1.6M12.9 8h1.6M3.4 3.4l1.1 1.1M11.5 11.5l1.1 1.1M3.4 12.6l1.1-1.1M11.5 4.5l1.1-1.1"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linecap="round"
        />
      </svg>
    {:else if app.settings.theme === "dark"}
      <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
        <path
          d="M13.2 9.6A5.6 5.6 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linejoin="round"
        />
      </svg>
    {:else}
      <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-width="1.4" />
        <path d="M8 2.4a5.6 5.6 0 0 1 0 11.2z" fill="currentColor" />
      </svg>
    {/if}
  </button>
</header>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: var(--titlebar-h);
    padding: 0 8px;
    background: var(--bg-chrome);
    border-bottom: 1px solid var(--border);
    flex: none;
  }

  .bar.traffic-lights {
    padding-left: 78px;
  }

  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    overflow: hidden;
  }

  .tab {
    position: relative;
    display: flex;
    align-items: center;
    height: 28px;
    min-width: 72px;
    max-width: 220px;
    border-radius: var(--radius);
    color: var(--text-muted);
    border: 1px solid transparent;
    transition: background-color 80ms linear, color 80ms linear;
  }

  .tab:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  /* A hairline between neighbouring tabs that are neither selected nor hovered. */
  .tab + .tab::before {
    content: "";
    position: absolute;
    left: -2px;
    top: 7px;
    bottom: 7px;
    width: 1px;
    background: var(--border-strong);
    opacity: 0.8;
  }

  .tab.selected::before,
  .tab:hover::before,
  .tab.selected + .tab::before,
  .tab:hover + .tab::before {
    opacity: 0;
  }

  .tab.selected {
    background: var(--bg-elevated);
    border-color: var(--tab-border);
    box-shadow: var(--shadow-tab);
    color: var(--text);
  }

  .tab-main {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    height: 100%;
    padding: 0 12px 0 10px;
    border: 0;
    border-radius: inherit;
    background: transparent;
    font-weight: 500;
  }

  .tab-main:active {
    background: var(--bg-pressed);
  }

  .tab-main:focus-visible {
    outline-offset: -1px;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }

  .tab:not(.selected) .dot {
    opacity: 0.75;
  }

  /* On hover the close button takes the dot's place, as in macOS tabs. */
  .tab:hover .dot,
  .tab:focus-within .dot {
    opacity: 0;
  }

  .name {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .close {
    position: absolute;
    left: 4.5px;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    opacity: 0;
    transition: opacity 80ms linear;
  }

  .tab:hover .close,
  .tab:focus-within .close {
    opacity: 1;
  }

  .close:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .close:active {
    background: var(--bg-pressed);
  }

  .drag {
    flex: 1;
    align-self: stretch;
  }
</style>
