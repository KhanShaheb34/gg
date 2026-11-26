<!--
  The status bar along the window's foot, under the graph and the side panel: information,
  where the header above holds actions. On its left the active worktree's folder (the home
  folder as ~, cut in the middle when it does not fit, in full in its tooltip): a click, or the
  menu key, opens a small menu to copy the path or show the folder in the file manager. On
  its right when the repository was last fetched, by gg or anything else ("Fetched 2 min
  ago"; the moment itself in its tooltip), kept current by one timer set for when the wording
  next changes; nothing while that is unknown. View › Status bar hides it.
-->
<script lang="ts">
  import { formatDateLong, shortenPath } from "../format";
  import { item } from "../menus/common";
  import { revealLabel } from "../platform";
  import { app, toast } from "../state/app.svelte";
  import { openExternal } from "../state/editor";
  import { openMenu, overlays } from "../state/overlays.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import { fetchedText, nextChangeIn } from "../statusBar";
  import MiddleText from "./MiddleText.svelte";

  let { view, path }: { view: RepoView; path: string } = $props();

  const shown = $derived(shortenPath(path, app.homeDir));
  const fetchedAt = $derived(view.snapshot.fetch.fetchedAtMs);

  /** Moves the wording on: when it next changes, and when the window shows again. */
  let tick = $state(0);
  /** The time the wording is written for: read afresh with every tick and every new fetch time (another tab's, say). */
  const now = $derived.by(() => {
    void tick;
    void fetchedAt;
    return Date.now();
  });
  $effect(() => {
    if (fetchedAt === null) return;
    const timer = setTimeout(() => tick++, nextChangeIn(fetchedAt, now) + 50);
    return () => clearTimeout(timer);
  });
  $effect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") tick++;
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  });

  const fetched = $derived(fetchedText(fetchedAt, now));
  const fetchedTitle = $derived.by(() => {
    if (fetchedAt === null) return "";
    const format = app.settings.dateFormat === "relative" ? "standard" : app.settings.dateFormat;
    return `Last fetched ${formatDateLong(Math.floor(fetchedAt / 1000), format)}`;
  });

  /** The menu this bar opened, while it is the one open. */
  let menuId = $state<number | null>(null);
  const menuOpen = $derived(menuId !== null && overlays.menu?.id === menuId);

  function showMenu(button: HTMLElement, keyboard: boolean) {
    const r = button.getBoundingClientRect();
    const target = path;
    openMenu(
      { x: r.left, y: r.top - 4 },
      [item("copy", "Copy path"), item("reveal", revealLabel())],
      (id) => {
        if (id === "copy") {
          navigator.clipboard.writeText(target).then(
            () => toast("info", "Copied the path."),
            () => toast("error", "Could not copy the path."),
          );
        } else if (id === "reveal") {
          void openExternal(view.repoId, { target: "reveal", path: target });
        }
      },
      { keyboard, above: true },
    );
    menuId = overlays.menu?.id ?? null;
  }
</script>

<footer class="status-bar" data-testid="status-bar" aria-label="Status">
  <button
    class="path"
    data-testid="status-path"
    aria-haspopup="menu"
    aria-expanded={menuOpen}
    aria-label="Active worktree {shown}: copy its path or show it"
    title={path}
    onclick={(e) => showMenu(e.currentTarget, e.detail === 0)}
    oncontextmenu={(e) => {
      e.preventDefault();
      showMenu(e.currentTarget, false);
    }}
  >
    <!-- A folder. -->
    <svg class="glyph" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <path
        d="M1.5 3.2c0-.5.4-.9.9-.9h2.2l1.1 1.2h3.9c.5 0 .9.4.9.9v4.4c0 .5-.4.9-.9.9H2.4c-.5 0-.9-.4-.9-.9z"
        fill="none"
        stroke="currentColor"
        stroke-width="1.1"
        stroke-linejoin="round"
      />
    </svg>
    <MiddleText class="path-text" text={shown} keep={28} title={null} />
  </button>
  <span class="spacer"></span>
  {#if fetched !== null}
    <span class="fetched" data-testid="status-fetched" title={fetchedTitle}>{fetched}</span>
  {/if}
</footer>

<style>
  /* Quiet: the chrome's own tone, a hairline above, small muted text. */
  .status-bar {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 22px;
    padding: 0 10px 0 8px;
    border-top: 1px solid var(--border);
    background: var(--bg-chrome);
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 16px;
    flex: none;
    white-space: nowrap;
  }

  .path {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: 0 1 auto;
    min-width: 0;
    height: 18px;
    padding: 0 6px 0 8px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: inherit;
    font-size: inherit;
    transition: background-color 80ms linear, color 80ms linear;
  }

  .path:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .path:active,
  .path[aria-expanded="true"] {
    background: var(--bg-pressed);
  }

  .glyph {
    flex: none;
    opacity: 0.85;
  }

  .path :global(.path-text) {
    flex: 0 1 auto;
    min-width: 0;
  }

  .spacer {
    flex: 1 1 0;
    min-width: 0;
  }

  .fetched {
    flex: none;
    font-variant-numeric: tabular-nums;
  }
</style>
