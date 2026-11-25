<!--
  The worktrees of a repository, as git lists them (so ones made in a terminal, by an agent
  or by another tool show too), the main one first. Each row: on its first line the branch
  (or the detached commit), uncommitted changes and, in one place at the right, how it stands
  with its upstream (↑ahead ↓behind, "no upstream", or how many commits are on no remote); on
  its second line its folder (by name: the folder most of them are in is said once, in the
  panel's header; see worktreeFolders) and what is unusual: locked, folder missing, a merge or
  rebase stopped there. Long names are cut in the middle. The active worktree, where every
  action runs, has the accent bar and the ring the graph draws on its HEAD.

  A grid of rows: a click selects a row and shows its HEAD in the graph; a double click, Enter
  or the row's "Make active" button makes it the active worktree (the row shows the switch at
  once; one that cannot be made active says why). The grid takes arrow keys, Home and End;
  the menu key (or Shift+F10) or a right click opens a row's menu. With only the main
  worktree, one sentence says what worktrees are for.
-->
<script lang="ts">
  import { tick } from "svelte";
  import { shortHash, worktreeFolders } from "../format";
  import { commits } from "../menus/common";
  import { makeActiveReason } from "../menus/worktreeMenu";
  import { app, toast } from "../state/app.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import {
    addWorktree,
    listedWorktrees,
    makeActive,
    openCommit,
    openWorktreeMenu,
    panelState,
    revealWorktree,
  } from "../state/worktrees.svelte";
  import { clock, openPullRequest, staleSince } from "../state/prs.svelte";
  import type { WorktreeInfo } from "../types";
  import MiddleText from "./MiddleText.svelte";
  import PrChip from "./PrChip.svelte";

  let { view }: { view: RepoView } = $props();

  const panel = $derived(panelState(view.repoId));
  const snap = $derived(view.snapshot);
  /** The main worktree first, then the others in git's order. */
  const rows = $derived(listedWorktrees(snap));
  const folders = $derived(worktreeFolders(rows.map((w) => w.path), rows.find((w) => w.isMain)?.path ?? null, app.homeDir));
  const loading = $derived(snap.generation < 0 && rows.length === 0);
  const selectedIndex = $derived(rows.findIndex((w) => w.path === panel.selected));
  /** The pull-request markers' "as of" when they may be out of date. */
  const prStale = $derived(staleSince(snap.prs, clock.now, app.settings.dateFormat));

  let list: HTMLElement | undefined = $state();
  const uid = Math.random().toString(36).slice(2, 8);
  const rowId = (i: number) => `wt-${uid}-${i}`;
  /** The row's first cell: the grid's active descendant, named for the worktree. */
  const cellId = (i: number) => `wt-${uid}-${i}-cell`;

  // The header's worktree count (the Worktrees tab, the focus in its list) asks for the focus here.
  $effect(() => {
    const request = panel.focusRequest;
    if (request === panel.focusDone) return;
    panel.focusDone = request;
    void tick().then(() => {
      if (panel.selected === null) panel.selected = rows.find((w) => w.active)?.path ?? null;
      list?.focus();
    });
  });

  const IN_PROGRESS = { merge: "merging", rebase: "rebasing", "cherry-pick": "cherry-picking", revert: "reverting" } as const;

  /** What the row says of its uncommitted changes, for its tooltip and accessible name. */
  function changesText(wt: WorktreeInfo): string {
    const s = wt.status;
    if (s === null) return wt.prunable ? "" : "Changes not read yet";
    const parts = [];
    if (s.changed > 0) parts.push(`${s.changed} changed`);
    if (s.untracked > 0) parts.push(`${s.untracked} new`);
    if (s.conflicted > 0) parts.push(`${s.conflicted} with conflicts`);
    return parts.length === 0 ? "No uncommitted changes" : `Uncommitted: ${parts.join(", ")}`;
  }

  function changeCount(wt: WorktreeInfo): number {
    return wt.status === null ? 0 : wt.status.changed + wt.status.untracked;
  }

  function aheadBehind(wt: WorktreeInfo): string {
    const u = wt.upstream;
    if (u === null) return "";
    const ahead = u.ahead > 0 ? `↑${u.ahead}` : "";
    const behind = u.behind > 0 ? `↓${u.behind}` : "";
    return ahead && behind ? `${ahead} ${behind}` : ahead + behind;
  }

  function upstreamText(wt: WorktreeInfo): string {
    const u = wt.upstream;
    if (u === null) return "";
    if (u.ahead === 0 && u.behind === 0) return `Up to date with ${u.name}`;
    return `${u.ahead} ahead, ${u.behind} behind ${u.name}`;
  }

  /** "no upstream", or the commits on no remote when there are some; "" with an upstream. */
  function remoteNote(wt: WorktreeInfo): string {
    // Without any remote, nothing is pushed anywhere: not worth saying on every row.
    if (wt.upstream !== null || wt.prunable || snap.remotes.length === 0) return "";
    if (wt.unpushed !== null && wt.unpushed > 0) return `${wt.unpushed} unpushed`;
    return wt.branch === null ? "" : "no upstream";
  }

  function remoteNoteTitle(wt: WorktreeInfo): string {
    if (wt.unpushed !== null && wt.unpushed > 0) {
      return `${commits(wt.unpushed)} on no remote${wt.branch === null ? "" : `; ${wt.branch} has no upstream`}`;
    }
    return `${wt.branch} has no upstream branch`;
  }

  /** The accessible name of a row. */
  function label(wt: WorktreeInfo): string {
    const name = wt.branch ?? `detached at ${wt.head ? shortHash(wt.head) : "nothing"}`;
    const parts = [name, folders.labels.get(wt.path) ?? wt.path];
    if (wt.active) parts.push("active");
    if (wt.isMain) parts.push("main worktree");
    const changes = changesText(wt);
    if (changes) parts.push(changes);
    if (wt.upstream) parts.push(upstreamText(wt));
    else if (remoteNote(wt)) parts.push(remoteNote(wt));
    if (wt.locked) parts.push(wt.lockReason ? `locked: ${wt.lockReason}` : "locked");
    if (wt.prunable) parts.push("folder missing");
    if (wt.inProgress) parts.push(`${wt.inProgress} in progress`);
    if (wt.pr) parts.push(`${wt.pr.state === "draft" ? "draft " : ""}pull request #${wt.pr.number}`);
    return parts.join(", ");
  }

  function select(i: number) {
    const wt = rows[i];
    if (wt) void revealWorktree(view, wt);
  }

  /** Makes `wt` active, or says why it cannot be. */
  function activate(wt: WorktreeInfo) {
    const why = makeActiveReason(wt, snap);
    if (why === undefined) void makeActive(view.repoId, wt.path, view);
    else if (!wt.active) toast("info", `${wt.branch ?? "That worktree"} cannot be made active: ${why.charAt(0).toLowerCase()}${why.slice(1)}.`);
  }

  function rowAt(e: Event): number {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-row]");
    return el ? Number(el.dataset.row) : -1;
  }

  function onClick(e: MouseEvent) {
    const i = rowAt(e);
    if (i >= 0) select(i);
  }

  function onDblClick(e: MouseEvent) {
    if ((e.target as HTMLElement).closest("button")) return;
    const i = rowAt(e);
    if (i >= 0) activate(rows[i]);
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const i = rowAt(e);
    if (i < 0) return;
    panel.selected = rows[i].path;
    list?.focus({ preventScroll: true });
    openWorktreeMenu(view, rows[i], { x: e.clientX, y: e.clientY }, false);
  }

  function openMenuOf(i: number, anchor: HTMLElement) {
    const r = anchor.getBoundingClientRect();
    openWorktreeMenu(view, rows[i], { x: r.left + 8, y: r.bottom }, true);
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.target !== list || e.metaKey || e.ctrlKey || e.altKey) return;
    const at = selectedIndex;
    const last = rows.length - 1;
    let next = -1;
    switch (e.key) {
      case "ArrowDown":
        next = at < 0 ? 0 : Math.min(last, at + 1);
        break;
      case "ArrowUp":
        next = at < 0 ? last : Math.max(0, at - 1);
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = last;
        break;
      case "Enter":
        if (at >= 0) {
          e.preventDefault();
          activate(rows[at]);
        }
        return;
      case "ContextMenu":
      case "F10":
        if (e.key === "F10" && !e.shiftKey) return;
        e.preventDefault();
        if (at >= 0) {
          const el = list.querySelector<HTMLElement>(`#${rowId(at)}`);
          if (el) openMenuOf(at, el.querySelector<HTMLElement>(".name") ?? el);
        }
        return;
      default:
        return;
    }
    e.preventDefault();
    if (next >= 0 && next !== at) {
      select(next);
      void tick().then(() => list?.querySelector(`#${rowId(next)}`)?.scrollIntoView({ block: "nearest" }));
    }
  }
</script>

<div class="panel">
  {#if loading}
    <p class="state">Reading worktrees…</p>
  {:else}
    {#if folders.parent !== null}
      <!-- The folder most worktrees are in, said once: the rows name their folders only. -->
      <p class="where" data-testid="worktrees-folder">
        <span class="in">in</span><MiddleText text={folders.parent} keep={18} title={folders.parentPath} />
      </p>
    {/if}
    {#if snap.prs.partial}
      <!-- 1000 were listed: some branches' markers may be missing. -->
      <p class="where partial" data-testid="prs-partial" title="Branches with a pull request beyond those show none">
        Showing the first 1000 open pull requests
      </p>
    {/if}
    <!-- Rows take pointer events here; the grid itself takes the keys (aria-activedescendant). -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <div
      class="list"
      role="grid"
      aria-label="Worktrees"
      aria-rowcount={rows.length}
      tabindex="0"
      aria-activedescendant={selectedIndex >= 0 ? cellId(selectedIndex) : undefined}
      bind:this={list}
      onkeydown={onKeydown}
      onclick={onClick}
      ondblclick={onDblClick}
      oncontextmenu={onContextMenu}
      data-testid="worktree-list"
    >
      {#each rows as wt, i (wt.path)}
        {@const n = changeCount(wt)}
        {@const ab = aheadBehind(wt)}
        {@const note = remoteNote(wt)}
        {@const switching = panel.switching === wt.path}
        {@const canActivate = makeActiveReason(wt, snap) === undefined}
        <div
          id={rowId(i)}
          class="row"
          class:active={wt.active}
          class:missing={wt.prunable}
          role="row"
          aria-rowindex={i + 1}
          aria-selected={wt.path === panel.selected}
          aria-label={label(wt)}
          data-row={i}
          data-path={wt.path}
          data-active={wt.active}
        >
          <div class="cell" role="gridcell" id={cellId(i)} aria-label={label(wt)}>
            <span class="mark" aria-hidden="true">
              {#if switching}
                <svg class="spin" width="14" height="14" viewBox="0 0 16 16">
                  <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.25" stroke-width="1.8" />
                  <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
                </svg>
              {:else if wt.active}
                <!-- The ring the graph draws on the active HEAD. -->
                <svg width="14" height="14" viewBox="0 0 14 14">
                  <circle cx="7" cy="7" r="4.1" fill="none" stroke="currentColor" stroke-width="2" />
                </svg>
              {:else}
                <svg width="14" height="14" viewBox="0 0 14 14">
                  <path
                    d="M1.8 3.6c0-.6.5-1.1 1.1-1.1h2.8l1.2 1.3h4.2c.6 0 1.1.5 1.1 1.1v5.3c0 .6-.5 1.1-1.1 1.1H2.9c-.6 0-1.1-.5-1.1-1.1z"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.15"
                    stroke-linejoin="round"
                  />
                  {#if wt.prunable}
                    <path d="M2.2 12.2 11.8 2.2" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" />
                  {/if}
                </svg>
              {/if}
            </span>

            <span class="line one">
              {#if wt.branch !== null}
                <MiddleText class="name" text={wt.branch} />
                {#if wt.pr}
                  <PrChip pr={wt.pr} stale={prStale} onOpen={(pr) => openPullRequest(view.repoId, pr)} />
                {/if}
              {:else}
                <span class="name detached" title="Detached HEAD"
                  ><span class="hash">{wt.head ? shortHash(wt.head) : "—"}</span><span class="what">detached</span></span
                >
              {/if}
              <span class="facts">
                {#if wt.status === null && !wt.prunable}
                  <span class="pending" title="Not read yet">…</span>
                {:else if n > 0 || (wt.status?.conflicted ?? 0) > 0}
                  <!-- The changes, a button: their commit panel. -->
                  <button
                    class="changes"
                    class:conflicts={(wt.status?.conflicted ?? 0) > 0}
                    tabindex="-1"
                    title="{changesText(wt)}. Commit…"
                    aria-label="Commit the changes of {wt.branch ?? 'this worktree'}"
                    onclick={(e) => {
                      e.stopPropagation();
                      openCommit(view, wt.path);
                    }}
                    ondblclick={(e) => e.stopPropagation()}><span class="dot"></span>{n}</button
                  >
                {/if}
                <!-- How it stands with its upstream, in one place. -->
                {#if ab}
                  <span class="upstream ab" title={upstreamText(wt)}>{ab}</span>
                {:else if note}
                  <span class="upstream note" title={remoteNoteTitle(wt)}>{note}</span>
                {/if}
              </span>
            </span>

            <span class="line two">
              <MiddleText class="path" text={folders.labels.get(wt.path) ?? wt.path} title={wt.path} />
              {#if wt.inProgress || wt.prunable || wt.locked}
                <span class="badges">
                  {#if wt.inProgress}
                    <span class="badge warn" title="A {wt.inProgress} stopped there">{IN_PROGRESS[wt.inProgress]}</span>
                  {/if}
                  {#if wt.prunable}
                    <span class="badge warn" title="Its folder is gone: prune stale entries to forget it">folder missing</span>
                  {/if}
                  {#if wt.locked}
                    <span class="badge" title={wt.lockReason ? `Locked: ${wt.lockReason}` : "Locked"}
                      ><svg width="9" height="10" viewBox="0 0 9 10" aria-hidden="true"
                        ><rect x="1" y="4.4" width="7" height="4.9" rx="1" fill="currentColor" /><path
                          d="M2.6 4.6V3.2a1.9 1.9 0 0 1 3.8 0v1.4"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="1.1"
                        /></svg
                      >locked</span
                    >
                  {/if}
                </span>
              {/if}
            </span>
          </div>

          <div class="tools" role="gridcell">
            {#if !wt.active && (canActivate || switching)}
              <button
                class="activate"
                tabindex="-1"
                disabled={switching}
                title="Make this the active worktree: actions run there"
                onclick={(e) => {
                  e.stopPropagation();
                  activate(wt);
                }}>{switching ? "Switching…" : "Make active"}</button
              >
            {/if}
            <button
              class="more"
              tabindex="-1"
              aria-label="More for this worktree"
              title="More"
              onclick={(e) => {
                e.stopPropagation();
                panel.selected = wt.path;
                openMenuOf(i, e.currentTarget);
              }}
            >
              <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="3.2" cy="7" r="1.1" fill="currentColor" />
                <circle cx="7" cy="7" r="1.1" fill="currentColor" />
                <circle cx="10.8" cy="7" r="1.1" fill="currentColor" />
              </svg>
            </button>
          </div>
        </div>
      {/each}
    </div>

    {#if rows.length === 1}
      <div class="intro" data-testid="worktrees-intro">
        <p>A worktree is a second folder of this repository with another branch checked out, so you can work on both at once.</p>
        <button class="add" onclick={() => addWorktree(view.snapshot)} disabled={snap.head.hash === null}>Add worktree</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .panel {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .state {
    padding: 14px 12px;
    color: var(--text-faint);
    font-size: var(--fs-sm);
  }

  .where {
    display: flex;
    gap: 4px;
    min-width: 0;
    margin: 0;
    padding: 7px 12px 1px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .in {
    flex: none;
  }

  .partial {
    padding-top: 2px;
  }

  .list {
    margin: 0;
    padding: 4px 0;
    list-style: none;
    outline: none;
  }

  .list:focus-visible {
    box-shadow: inset 0 0 0 2px var(--focus-ring);
  }

  .row {
    position: relative;
    transition: background-color 60ms linear;
  }

  /* Two lines: what is checked out and how it stands; where it is and what is unusual. */
  .cell {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr);
    grid-template-rows: 18px 16px;
    column-gap: 8px;
    row-gap: 2px;
    padding: 6px 10px 6px 12px;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .row[aria-selected="true"] {
    background: var(--bg-selected);
  }

  .list:focus-visible .row[aria-selected="true"] {
    box-shadow: inset 0 0 0 1px var(--focus-ring);
  }

  /* The active worktree: the accent bar along its left edge. */
  .row.active::before {
    content: "";
    position: absolute;
    top: 5px;
    bottom: 5px;
    left: 0;
    width: 3px;
    border-radius: 0 2px 2px 0;
    background: var(--accent);
  }

  .mark {
    display: grid;
    place-items: center;
    grid-row: 1;
    height: 18px;
    color: var(--text-faint);
  }

  .active .mark {
    color: var(--accent);
  }

  .missing .mark {
    color: var(--warning);
  }

  .mark svg {
    display: block;
  }

  .spin {
    color: var(--accent);
    animation: spin 0.8s linear infinite;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    grid-column: 2;
  }

  .one {
    grid-row: 1;
  }

  .two {
    grid-row: 2;
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }

  .line :global(.name) {
    min-width: 0;
    font-weight: 500;
  }

  .active .line :global(.name) {
    font-weight: 650;
  }

  .missing .line :global(.name) {
    color: var(--text-muted);
  }

  .detached {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
  }

  .hash {
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .what {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-weight: 500;
  }

  .facts {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    margin-left: auto;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }

  .changes {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 18px;
    margin: 0 -5px;
    padding: 0 5px;
    border: 0;
    border-radius: 9px;
    background: transparent;
    color: var(--text);
    font: inherit;
    transition: background-color 80ms linear;
  }

  .changes:hover {
    background: var(--bg-pressed);
  }

  .changes .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warning);
  }

  .changes.conflicts {
    color: var(--danger);
  }

  .changes.conflicts .dot {
    background: var(--danger);
  }

  .pending {
    color: var(--text-faint);
    font-weight: 500;
  }

  .ab {
    letter-spacing: -0.01em;
  }

  .line :global(.path) {
    flex: 0 1 auto;
    min-width: 0;
  }

  .badges {
    display: flex;
    align-items: center;
    gap: 5px;
    flex: none;
    margin-left: auto;
  }

  .upstream {
    white-space: nowrap;
  }

  .note {
    color: var(--text-faint);
    font-weight: 500;
  }

  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 15px;
    padding: 0 5px;
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: 10.5px;
    font-weight: 550;
    line-height: 13px;
    white-space: nowrap;
  }

  .badge.warn {
    border-color: color-mix(in srgb, var(--warning) 45%, transparent);
    background: color-mix(in srgb, var(--warning) 9%, transparent);
    color: var(--warning);
  }

  /* Make active and the menu, over the right end of the row's second line while the pointer
     is on it (the first line's changes stay in reach; the keys have Enter and the menu key; a
     selected row keeps its facts in sight). */
  .tools {
    position: absolute;
    right: 0;
    bottom: 3px;
    height: 24px;
    display: none;
    align-items: center;
    gap: 2px;
    padding: 0 6px 0 18px;
    background: linear-gradient(90deg, transparent, var(--row-bg) 16px);
  }

  .row:hover .tools {
    display: flex;
  }

  .row:hover {
    --row-bg: color-mix(in srgb, var(--text) 5.5%, var(--bg));
  }

  .row[aria-selected="true"] {
    --row-bg: color-mix(in srgb, var(--accent) 11%, var(--bg));
  }

  .row[aria-selected="true"]:hover {
    background: color-mix(in srgb, var(--accent) 15%, var(--bg));
    --row-bg: color-mix(in srgb, var(--accent) 15%, var(--bg));
  }

  .activate {
    height: 22px;
    padding: 0 8px;
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    background: var(--bg-elevated);
    color: var(--text);
    font-size: var(--fs-xs);
    font-weight: 550;
    white-space: nowrap;
  }

  .activate:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .activate:disabled {
    color: var(--text-faint);
  }

  .more {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted);
  }

  .more:hover {
    background: var(--bg-pressed);
    color: var(--text);
  }

  .intro {
    margin: 6px 12px 12px;
    padding: 12px;
    border: 1px dashed var(--border-strong);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }

  .add {
    height: 26px;
    margin-top: 10px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
  }

  .add:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .add:disabled {
    color: var(--text-faint);
  }
</style>
