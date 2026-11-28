<script lang="ts">
  import { onMount } from "svelte";
  import CommandLog from "./lib/components/CommandLog.svelte";
  import CommitDetails from "./lib/components/CommitDetails.svelte";
  import ConflictBanner from "./lib/components/ConflictBanner.svelte";
  import ContextMenu from "./lib/components/ContextMenu.svelte";
  import AppIcon from "./lib/components/AppIcon.svelte";
  import SidePanel from "./lib/components/SidePanel.svelte";
  import WorktreePanel from "./lib/components/WorktreePanel.svelte";
  import CommitPanel from "./lib/components/CommitPanel.svelte";
  import PullRequestPanel from "./lib/components/PullRequestPanel.svelte";
  import StatusBar from "./lib/components/StatusBar.svelte";
  import Toolbar from "./lib/components/Toolbar.svelte";
  import TabBar from "./lib/components/TabBar.svelte";
  import Toast from "./lib/components/Toast.svelte";
  import Welcome from "./lib/components/Welcome.svelte";
  import InboxPage from "./lib/components/InboxPage.svelte";
  import { backToList, closeInbox, forwardToPr, refreshInbox, toggleInbox } from "./lib/state/inbox.svelte";
  import { detailsRoom, panelRoom } from "./lib/detailsLayout";
  import { shortHash, shortenPath } from "./lib/format";
  import MiddleText from "./lib/components/MiddleText.svelte";
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
    windowState,
  } from "./lib/state/app.svelte";
  import { openBranchPicker } from "./lib/state/picker";
  import { openCliDialog } from "./lib/state/cli.svelte";
  import { openClone, openNewTab } from "./lib/state/clone.svelte";
  import { openSettings, openShortcuts } from "./lib/state/settings.svelte";
  import { initUpdates } from "./lib/state/updates.svelte";
  import { logPrefs, setLogOpen } from "./lib/state/commandLog.svelte";
  import { fromAppMenu, overlays } from "./lib/state/overlays.svelte";
  import { onUserPush } from "./lib/state/ops.svelte";
  import { afterPush, forgetSuggestions, openPullRequestTab, prTabTitle, subjectOf } from "./lib/state/pullRequest.svelte";
  import { detailsBusy, prChecksOf } from "./lib/state/prDetails.svelte";
  import { repoView } from "./lib/state/repo.svelte";
  import { statusBar } from "./lib/state/statusBar.svelte";
  import {
    activateWorktree,
    addWorktree,
    commitPath,
    committingIn,
    holdCommit,
    listedWorktrees,
    docked,
    openCommit,
    panelState,
    openPanelMenu,
    panelShown,
    setPanelShown,
    showSideTab,
    sidePanel,
  } from "./lib/state/worktrees.svelte";
  import { trackWindowFocus } from "./lib/windowFocus";

  const active = $derived(app.tabs.find((t) => t.repoId === app.activeRepoId) ?? null);
  const mac = isMac();
  const PANEL_KEYS = mac ? "⌘B" : "Ctrl+B";
  const PICKER_KEYS = mac ? "⌘P" : "Ctrl+P";
  const COMMIT_KEYS = mac ? "⌘K" : "Ctrl+K";
  const PR_KEYS = mac ? "⌘⇧P" : "Ctrl+Shift+P";
  const REFRESH_KEYS = mac ? "⌘R" : "Ctrl+R";

  // A commit panel whose worktree is no longer listed is closed for good (back to the worktree
  // list): a worktree made again at that path later is another one, and does not bring the
  // panel back.
  $effect(() => {
    if (!active) return;
    const panel = panelState(active.repoId);
    const listed = repoView(active.repoId).snapshot.worktrees;
    if (panel.commitFor !== null && listed.length > 0 && !listed.some((w) => w.path === panel.commitFor)) {
      panel.commitFor = null;
      panel.tab = "worktrees";
    }
  });

  // The Pull request tab lets go of a branch it was asked for once that is the current one
  // again, or is no longer a local branch: it follows the active worktree's branch.
  $effect(() => {
    if (!active) return;
    const panel = panelState(active.repoId);
    const snap = repoView(active.repoId).snapshot;
    const pinned = panel.prFor;
    if (pinned === null || snap.generation < 0) return;
    if (pinned === snap.head.branch || !snap.branches.includes(pinned)) panel.prFor = null;
  });

  // The Pull requests page asks gh again when the window comes back to the front (past a
  // floor: see refreshInbox), only while it shows.
  $effect(() => {
    if (windowState.focused && windowState.visible && app.inboxOpen) void refreshInbox("focus");
  });

  // The Commit tab, shown, holds the worktree it shows (see holdCommit).
  $effect(() => {
    if (active) holdCommit(repoView(active.repoId));
  });

  /** The active tab's active worktree, as its snapshot has it. */
  const activeWorktree = $derived.by(() => {
    if (!active) return null;
    const snap = repoView(active.repoId).snapshot;
    const fullPath = snap.activeWorktree || active.path;
    const name = snap.head.branch ?? (snap.head.hash ? `${shortHash(snap.head.hash)} (detached)` : "no commits");
    return {
      /** Its folder in full, for the status bar. */
      fullPath,
      /** Its folder, the home folder as ~. */
      path: shortenPath(fullPath, app.homeDir),
      name,
      count: snap.worktrees.length,
    };
  });

  /**
   * F6 (Shift+F6 back) moves between the window's regions: the graph, the newest notice, the
   * side panel. From outside them it goes to the newest notice first, else the graph.
   */
  function cycleRegion(delta: 1 | -1) {
    const graph = document.querySelector<HTMLElement>('#repo-panel [role="grid"]');
    const notices = document.querySelectorAll<HTMLElement>("[data-notices] .toast");
    const notice = notices[notices.length - 1]?.querySelector<HTMLElement>("button") ?? null;
    const panel = document.querySelector<HTMLElement>(
      '[data-region="side-panel"] [data-testid="worktree-list"], [data-region="side-panel"] [data-testid="commit-message"], [data-region="side-panel"] [data-pr-focus]',
    );
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

  /** Shows the side panel's Worktrees tab with the focus in its list (the header's worktree count). */
  function showWorktrees() {
    if (active) showSideTab(repoView(active.repoId), "worktrees", true);
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
    // Looking for a new version of gg: once the first rows are on screen.
    const stopUpdates = initUpdates();
    const stopFocus = trackWindowFocus(window, document, windowFocusChanged);
    const stopDrop = onFolderDrop(async (event) => {
      dragging = event.kind === "enter";
      if (event.kind === "drop") for (const path of event.paths) await openPath(path);
    });
    // The app menu's "Install ‘gg’ Command…", "Settings…" and Help's "Keyboard Shortcuts"
    // (macOS's menu bar; the window's menu on Windows and Linux): not while a dialog is open.
    const stopCliMenu = onEvent("menu-install-cli", () => fromAppMenu(() => void openCliDialog()));
    const stopSettingsMenu = onEvent("menu-settings", () => fromAppMenu(openSettings));
    const stopShortcutsMenu = onEvent("menu-shortcuts", () => fromAppMenu(openShortcuts));
    // The File menu's Open Repository…, Clone Repository… and New Tab, as their keys do.
    const stopOpenMenu = onEvent("menu-open", () => fromAppMenu(() => !app.gitMissing && void openViaPicker()));
    const stopCloneMenu = onEvent("menu-clone", () => fromAppMenu(() => !app.gitMissing && openClone()));
    const stopNewTabMenu = onEvent("menu-new-tab", () => fromAppMenu(() => !app.gitMissing && openNewTab()));
    // A push the user asked for may show the side panel's Pull request tab (pullRequest.svelte.ts).
    onUserPush(afterPush);
    // The webview's own context menu (Reload, Inspect…) is not the app's; text fields keep
    // theirs. The graph opens the app's own menus on its rows (GraphView), before this.
    const onContextMenu = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      if (!target?.closest("input, textarea, [contenteditable]")) e.preventDefault();
    };
    document.addEventListener("contextmenu", onContextMenu);
    // Hidden (minimised, another space), the Pull request tab stops asking about running checks.
    const onVisibility = () => (windowState.visible = document.visibilityState === "visible");
    document.addEventListener("visibilitychange", onVisibility);
    onVisibility();
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      stopUpdates();
      stopFocus();
      stopDrop();
      stopCliMenu();
      stopSettingsMenu();
      stopShortcutsMenu();
      stopOpenMenu();
      stopCloneMenu();
      stopNewTabMenu();
      onUserPush(null);
      document.removeEventListener("contextmenu", onContextMenu);
    };
  });

  /** The focus is in a field that takes text: plain keys (?) are its own. */
  function typing(): boolean {
    const el = document.activeElement;
    return el instanceof HTMLElement && (el.isContentEditable || !!el.closest("input, textarea, select"));
  }

  /**
   * Makes worktree `index` (0-based, as the side panel lists them) of the active tab the active
   * one; says why not when there is no such worktree or its folder is gone.
   */
  function switchWorktree(index: number) {
    if (!active) return;
    const view = repoView(active.repoId);
    const wt = listedWorktrees(view.snapshot)[index];
    if (!wt) {
      const n = view.snapshot.worktrees.length;
      toast("info", n <= 1 ? "This repository has one worktree." : `This repository has ${n} worktrees.`);
      return;
    }
    activateWorktree(view, wt);
  }

  function onKeydown(e: KeyboardEvent) {
    const shortcut = shortcutFor(e, mac, typing());
    if (!shortcut) return;
    // Always: Cmd/Ctrl+R must never reload the page, nor Cmd/Ctrl+F open the browser's find.
    e.preventDefault();
    // Holding Cmd+W must not close one tab after another.
    if (e.repeat && !repeatable(shortcut)) return;
    // A dialog is modal: nothing behind it changes while it is open.
    if (overlays.dialog) return;
    if (app.gitMissing && !["close", "settings", "help"].includes(shortcut.kind)) return;
    // On the Pull requests page, a repository's own shortcuts do nothing (its tab is behind
    // the page); Refresh loads the page again and Close puts it away.
    const view = active && !app.inboxOpen ? repoView(active.repoId) : null;
    if (app.inboxOpen && shortcut.kind === "refresh") {
      void refreshInbox("user");
      return;
    }
    if (app.inboxOpen && shortcut.kind === "close") {
      closeInbox();
      return;
    }
    switch (shortcut.kind) {
      case "open":
        void openViaPicker();
        break;
      case "newTab":
        openNewTab();
        break;
      case "clone":
        openClone();
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
      case "commit":
        if (view) openCommit(view);
        break;
      case "pullRequest":
        if (view) openPullRequestTab(view, null, true);
        break;
      case "inbox":
        toggleInbox();
        break;
      case "back":
        // The Pull requests page's history: nothing elsewhere.
        if (app.inboxOpen) backToList();
        break;
      case "forward":
        if (app.inboxOpen) forwardToPr();
        break;
      case "branches":
        if (view) openBranchPicker(view.repoId);
        break;
      case "sidePanel":
        if (view && active) {
          // Hidden from inside it (its header has no Hide button): the focus goes to the graph,
          // not to the page.
          const inside = document.activeElement?.closest('[data-region="side-panel"]') != null;
          setPanelShown(active.repoId);
          if (inside && !panelShown(active.repoId)) document.querySelector<HTMLElement>('#repo-panel [role="grid"]')?.focus();
        }
        break;
      case "log":
        if (view) setLogOpen(!logPrefs.open, true);
        break;
      case "settings":
        openSettings();
        break;
      case "help":
        openShortcuts();
        break;
      case "worktree":
        if (view) switchWorktree(shortcut.index);
        break;
      default: {
        const unhandled: never = shortcut;
        void unhandled;
      }
    }
  }
</script>

<svelte:window onkeydown={onKeydown} bind:innerWidth={sidePanel.windowW} />

<div class="shell" data-testid="app-root">
  <!-- Behind a modal dialog or sheet, the window takes no input: Tab never reaches it. -->
  <div class="behind" inert={overlays.dialogs.length > 0}>
  <TabBar />

  <main class="content">
    {#if app.gitMissing}
      <section class="notice" role="alert">
        <AppIcon size={44} />
        <p class="notice-text">Git was not found. Install git and restart.</p>
      </section>
    {:else if app.inboxOpen}
      <InboxPage />
    {:else if active}
      <div class="repo" id="repo-panel" role="tabpanel" aria-labelledby="tab-{active.repoId}" data-testid="repo-view">
        {#key active.repoId}
          <!-- Actions only: the repository's name is its tab's, its folder is in the status bar. -->
          <header class="repo-head">
            <div class="where">
              {#if activeWorktree}
                {@const hidden = !panelShown(active.repoId) && activeWorktree.count > 1}
                <!-- Where every action runs: the active worktree's branch, always in sight; it
                     opens the branch picker. -->
                <button
                  class="worktree"
                  data-testid="active-worktree"
                  aria-haspopup="dialog"
                  aria-label="Switch branch: {activeWorktree.name}, in the active worktree {activeWorktree.path}{activeWorktree.count > 1
                    ? `, one of ${activeWorktree.count} worktrees`
                    : ''}"
                  title="Switch branch ({PICKER_KEYS}). Active worktree: {activeWorktree.path}{activeWorktree.count > 1
                    ? ` (one of ${activeWorktree.count})`
                    : ''}"
                  onclick={() => openBranchPicker(active.repoId)}
                >
                  <svg class="ring" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
                    <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.7" />
                  </svg>
                  <MiddleText class="wt-branch" text={activeWorktree.name} keep={8} title={null} />
                  <svg class="chevron" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
                    <path d="M2.5 3.8 5 6.3l2.5-2.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
                  </svg>
                </button>
                {#if hidden}
                  <!-- How many worktrees there are while the panel is hidden: shows them. "worktrees"
                       wraps out of sight when there is no room for it. -->
                  <button
                    class="wt-count"
                    data-testid="worktree-count"
                    aria-label="Show the {activeWorktree.count} worktrees"
                    title="Show the worktrees ({PANEL_KEYS})"
                    onclick={showWorktrees}><span>· {activeWorktree.count}</span><span>&nbsp;worktrees</span></button
                  >
                {/if}
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
              <!-- Over the graph in a narrow window, the side panel goes when the graph is clicked. -->
              <div
                class="graph-area"
                onpointerdowncapture={() => {
                  if (!docked() && sidePanel.overlayOpen) setPanelShown(active.repoId, false);
                }}
              >
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
              {@const panel = panelState(active.repoId)}
              <!-- The worktree the Commit tab holds (or would show): its files to commit. -->
              {@const status = commitPath(view) === view.snapshot.activeWorktree ? view.snapshot.status : (committingIn(view)?.status ?? null)}
              {@const changes = status ? status.changed + status.untracked : 0}
              {@const worktrees = view.snapshot.worktrees.length}
              {@const prChecks = prChecksOf(view)}
              <SidePanel
                tabs={[
                  {
                    id: "commit",
                    label: "Commit",
                    title: `Commit${changes > 0 ? ` · ${changes} changed file${changes === 1 ? "" : "s"}` : ""} (${COMMIT_KEYS})`,
                    dot: changes > 0 ? "accent" : false,
                  },
                  {
                    id: "pr",
                    label: "Pull request",
                    title: prTabTitle(view.snapshot, subjectOf(view), PR_KEYS, prChecks?.phrase ?? null),
                    dot: prChecks?.tone ?? false,
                  },
                  { id: "worktrees", label: "Worktrees", title: `Worktrees${worktrees > 1 ? ` · ${worktrees}` : ""}` },
                ]}
                tab={panel.tab}
                onselect={(tab) => showSideTab(view, tab)}
                overlay={!docked()}
                top={bannerH}
                onhide={() => {
                  setPanelShown(active.repoId, false);
                  document.querySelector<HTMLElement>('#repo-panel [role="grid"]')?.focus();
                }}
              >
                {#snippet actions()}
                  {#if panel.tab === "pr"}
                    {@const busy = view.refreshing || view.snapshot.prs.status === "loading" || detailsBusy(view.snapshot.rootPath)}
                    <button
                      class="icon-btn small"
                      aria-label="Refresh the pull requests"
                      title="Refresh the pull requests ({REFRESH_KEYS})"
                      aria-busy={busy}
                      data-testid="pr-refresh"
                      onclick={() => {
                        forgetSuggestions(view.snapshot.rootPath);
                        panel.prRefresh++;
                        void view.refresh();
                      }}
                    >
                      <!-- The toolbar's Refresh, turning while the list loads. -->
                      <svg class:spinning={busy} width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
                        <path
                          d="M13 8a5 5 0 1 1-1.46-3.54M13 2.8v2.9h-2.9"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="1.6"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        />
                      </svg>
                    </button>
                  {/if}
                  {#if panel.tab === "worktrees"}
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
                  {/if}
                {/snippet}
                {#if panel.tab === "commit"}
                  {@const path = commitPath(view)}
                  {#if path}
                    {#key path}
                      <CommitPanel {view} {path} />
                    {/key}
                  {/if}
                {:else if panel.tab === "pr"}
                  <PullRequestPanel {view} branch={subjectOf(view)} />
                {:else}
                  <WorktreePanel {view} />
                {/if}
              </SidePanel>
            {/if}
          </div>
        {/key}
      </div>
    {:else if app.ready && !app.restoring}
      <Welcome />
    {/if}
  </main>
  {#if active && activeWorktree && statusBar.shown && !app.gitMissing && !app.inboxOpen}
    <StatusBar view={repoView(active.repoId)} path={activeWorktree.fullPath} />
  {/if}
  </div>

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
  .dialog-layer,
  .behind {
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
    padding: 0 8px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }

  /* The branch (and how many worktrees): gives way before the toolbar does (the toolbar is
     never cut off). */
  .where {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
  }

  /* The active worktree: its branch with the HEAD ring; opens the branch picker. It keeps its
     width: the branch up to 180 px, cut in the middle beyond. As tall as the toolbar's pop-up
     buttons beside it. */
  .worktree {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: 0 1000 auto;
    min-width: min-content;
    height: 28px;
    padding: 0 8px 0 8px;
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

  .worktree :global(.wt-branch) {
    flex: none;
    max-width: 180px;
    color: var(--text);
    font-weight: 600;
  }

  .chevron {
    flex: none;
    margin-left: -1px;
    color: var(--text-muted);
  }

  .wt-count {
    display: flex;
    flex-wrap: wrap;
    flex: 0 1 auto;
    min-width: 2.4em;
    height: 22px;
    margin-left: -6px;
    padding: 2px 6px;
    overflow: hidden;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 18px;
    white-space: nowrap;
    transition: background-color 80ms linear;
  }

  .wt-count:hover {
    background: var(--bg-hover);
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

  .spinning {
    animation: spin 0.8s linear infinite;
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
