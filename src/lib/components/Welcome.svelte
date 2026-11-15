<!-- Shown when no repository is open: open one, or pick from the recent ones. -->
<script lang="ts">
  import { onMount } from "svelte";
  import { colorIndex, displayFolder, relativeTime, shortenPath } from "../format";
  import { isMac, isTauri } from "../platform";
  import { app, openPath, openViaPicker, removeRecent } from "../state/app.svelte";
  import { folderOf } from "../cli";
  import { openCliDialog, removeCli } from "../state/cli.svelte";
  import Mark from "./Mark.svelte";

  const shortcut = isMac() ? "⌘O" : "Ctrl+O";
  // Only the app learns the path of a dropped folder; a browser page cannot.
  const canDrop = isTauri();

  let now = $state(Date.now());
  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 60_000);
    return () => clearInterval(timer);
  });
</script>

<section class="welcome" data-testid="welcome" aria-labelledby="welcome-title">
  <div class="column">
    <header class="intro">
      <div class="tile"><Mark size={34} /></div>
      <div>
        <h1 id="welcome-title">gg</h1>
        <p class="lede">Open a repository folder to see its branches and history as a graph.</p>
      </div>
    </header>

    <div class="actions">
      <button class="primary" onclick={openViaPicker}>
        <svg width="15" height="15" viewBox="0 0 16 16" aria-hidden="true">
          <path
            d="M1.75 4.25c0-.83.67-1.5 1.5-1.5h3l1.5 1.75h5c.83 0 1.5.67 1.5 1.5v5.75c0 .83-.67 1.5-1.5 1.5h-9.5c-.83 0-1.5-.67-1.5-1.5z"
            fill="none"
            stroke="currentColor"
            stroke-width="1.4"
            stroke-linejoin="round"
          />
        </svg>
        Open repository…
        <kbd>{shortcut}</kbd>
      </button>
      {#if canDrop}
        <p class="hint">or drop a folder onto this window</p>
      {/if}
    </div>

    <div class="recent">
      <h2 id="recent-title">Recent</h2>
      {#if app.recents.length > 0}
        <ul aria-label="Recent repositories">
          {#each app.recents as recent (recent.path)}
            <li>
              <button
                class="open"
                aria-label="Open {recent.name}"
                title={shortenPath(recent.path, app.homeDir)}
                onclick={() => openPath(recent.path)}
              >
                <span class="dot" style:background="var(--lane-{colorIndex(recent.path)})" aria-hidden="true"></span>
                <span class="name">{recent.name}</span>
                <span class="folder path-tail"><bdi>{displayFolder(recent.path, app.homeDir)}</bdi></span>
                <span class="when">{relativeTime(recent.lastOpenedMs, now)}</span>
              </button>
              <button
                class="remove"
                aria-label="Remove {recent.name} from recents"
                title="Remove from recents"
                onclick={() => removeRecent(recent.path)}
              >
                <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
                  <path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
                </svg>
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="empty">Repositories you open will be listed here.</p>
      {/if}
    </div>

    {#if app.cli.installed === null}
      <p class="cli-hint" data-testid="cli-hint">
        Open a repository from a terminal with <code>gg</code>.
        <button class="link" onclick={() => void openCliDialog()}>Install the command…</button>
      </p>
    {:else}
      <p class="cli-hint" data-testid="cli-hint">
        <code>gg</code> command installed in {shortenPath(folderOf(app.cli.installed), app.homeDir)} ·
        <button class="link" onclick={() => void removeCli()}>Remove</button>
      </p>
    {/if}
  </div>
</section>

<style>
  .welcome {
    height: 100%;
    overflow: auto;
    padding: clamp(40px, 12vh, 120px) 24px 48px;
  }

  .column {
    width: min(560px, 100%);
    margin: 0 auto;
  }

  .intro {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .tile {
    --mark-ground: var(--bg-elevated);
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--bg-elevated);
    box-shadow: var(--shadow-tab);
    flex: none;
  }

  h1 {
    font-size: 22px;
    font-weight: 650;
    letter-spacing: -0.015em;
    line-height: 1.2;
  }

  .lede {
    margin-top: 3px;
    color: var(--text-muted);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-top: 28px;
  }

  .primary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 8px 0 12px;
    border: 1px solid transparent;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--accent-contrast);
    font-weight: 550;
    box-shadow: 0 1px 1px rgba(0, 0, 0, 0.12);
    transition: background-color 80ms linear;
  }

  .primary:hover {
    background: var(--accent-hover);
  }

  .primary:active {
    transform: translateY(0.5px);
    filter: brightness(0.94);
  }

  .primary:focus-visible {
    outline-offset: 2px;
  }

  kbd {
    margin-left: 4px;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.16);
    font-family: var(--font-ui);
    font-size: var(--fs-xs);
    font-weight: 500;
    opacity: 0.9;
  }

  :global(:root[data-theme="dark"]) kbd {
    background: rgba(0, 0, 0, 0.14);
  }

  .hint {
    color: var(--text-faint);
    font-size: var(--fs-sm);
  }

  .recent {
    margin-top: 40px;
  }

  h2 {
    padding: 0 8px 6px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    font-weight: 600;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--border);
  }

  li {
    position: relative;
    border-bottom: 1px solid var(--border);
  }

  .open {
    display: grid;
    grid-template-columns: 7px 168px minmax(0, 1fr) auto;
    align-items: center;
    column-gap: 10px;
    width: 100%;
    height: 32px;
    padding: 0 36px 0 10px;
    border: 0;
    border-radius: 0;
    background: transparent;
    text-align: left;
  }

  .open:hover {
    background: var(--bg-hover);
  }

  .open:active {
    background: var(--bg-pressed);
  }

  .open:focus-visible {
    outline-offset: -2px;
    border-radius: var(--radius);
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }

  .name {
    font-weight: 550;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .folder {
    color: var(--text-muted);
  }

  .when {
    color: var(--text-faint);
    font-size: var(--fs-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .remove {
    position: absolute;
    top: 50%;
    right: 8px;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--text-faint);
    opacity: 0;
    transform: translateY(-50%);
    transition: opacity 80ms linear;
  }

  li:hover .remove,
  .remove:focus-visible {
    opacity: 1;
  }

  .remove:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .remove:active {
    background: var(--bg-pressed);
  }

  .cli-hint {
    margin-top: 28px;
    padding: 0 8px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  .cli-hint code {
    font-family: var(--font-mono);
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
  }

  .link:hover {
    text-decoration: underline;
  }

  .empty {
    padding: 10px 8px;
    border-top: 1px solid var(--border);
    color: var(--text-faint);
  }
</style>
