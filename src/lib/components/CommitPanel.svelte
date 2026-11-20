<!--
  The commit panel, the side panel's second view: the changed files of one
  worktree, each with a box, all ticked to begin with, and a message; Commit, and Commit &
  Push naming where the push goes. Commits run in the active worktree: shown another,
  the panel lists its files and offers to make it active, one click.

  The list (sorted by path, as the core gives it) draws only the rows in view, so 5,000 files
  cost what thirty do. Each row: its box, its status letter in the app's file colours, its
  path cut in the middle so the file name stays (a rename "old → new"). A conflicted file, or
  one whose name git cannot take (not UTF-8), cannot be ticked: it says why, under it. The
  list takes the keys: arrows move, Space ticks, Cmd+A ticks every file. While a merge is
  concluded, every tracked file is ticked and stays so; new files are up to the user.

  The list follows the worktree as it changes, keeping ticks and the message; the message is
  kept per worktree while the app runs, closed or not. Cmd+Enter in the message commits.
  With no file ticked (and no merge to conclude), "Commit with no changes" under the buttons
  makes both buttons make an empty commit (git commit --allow-empty): the usual way to run a
  branch's CI again. It is off until asked for, so an unticked list never commits by mistake.
  What stops a commit is said under the buttons. A commit made clears the message and the
  ticks (the notice offers the push when it was not pushed); with nothing left, the panel
  goes back to the worktree list. A failure (a hook's output) is a notice with Show log, and
  the message stays.
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import { visibleText } from "../format";
  import { api } from "../ipc";
  import { makeActiveReason } from "../menus/worktreeMenu";
  import { isMac } from "../platform";
  import { runOp } from "../state/ops.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import { closeCommit, commitDrafts, makeActive, panelState } from "../state/worktrees.svelte";
  import type { FileEntry, HeadInfo, OpRequest } from "../types";
  import MiddleText from "./MiddleText.svelte";

  let { view, path }: { view: RepoView; path: string } = $props();

  /** Height of a row, and of one that says why it cannot be ticked, px. */
  const ROW_H = 26;
  const NOTED_H = 42;
  /** Rows drawn beyond each edge of the view. */
  const OVERSCAN = 8;
  /** The soft limit of a commit's first line. */
  const SUBJECT_MAX = 72;
  const COMMIT_KEYS = isMac() ? "⌘↩" : "Ctrl+Enter";

  const panel = $derived(panelState(view.repoId));
  const snap = $derived(view.snapshot);
  const wt = $derived(snap.worktrees.find((w) => w.path === path) ?? null);
  const active = $derived(wt?.active ?? snap.activeWorktree === path);
  /** Stopped there (the active worktree's state). */
  const stopped = $derived(active ? snap.inProgress : null);
  const merging = $derived(stopped?.kind === "merge");

  let files = $state.raw<FileEntry[] | null>(null);
  /** HEAD as the snapshot showed it when the list was asked for: what the ticks belong to. */
  let filesHead = $state.raw<HeadInfo | null>(null);
  let truncated = $state(false);
  /** The remote the core pushes this branch to (git's rule, as pull requests do); null until read. */
  let pushRemote = $state<string | null>(null);
  let readError: string | null = $state(null);
  /** The paths ticked. */
  let ticked = $state.raw<Set<string>>(new Set());
  /** Everything was ticked and nothing unticked since: files that appear come ticked too. */
  let tickNew = true;
  let message = $state(untrack(() => commitDrafts.get(path) ?? ""));
  let pending: "commit" | "push" | null = $state(null);
  /** The user asked for a commit with no changes ("Commit with no changes"). */
  let allowEmpty = $state(false);
  let switching = $state(false);

  /** Why a file cannot be ticked, or null. */
  function blocked(f: FileEntry): string | null {
    if (f.status === "U") return "Resolve its conflicts first.";
    if (!f.committable) return f.reason ?? "git cannot take this name.";
    return null;
  }
  /** A sentence of its own said after a colon: its first letter lower-cased. */
  const midSentence = (text: string) => text.charAt(0).toLowerCase() + text.slice(1);
  /** Ticked and kept so: a tracked file while a merge is concluded. */
  const locked = (f: FileEntry) => merging && f.status !== "?" && blocked(f) === null;
  const isTicked = (f: FileEntry) => blocked(f) === null && (locked(f) || ticked.has(f.path));

  const tickable = $derived((files ?? []).filter((f) => blocked(f) === null));
  const chosen = $derived((files ?? []).filter(isTicked));
  const conflicts = $derived((files ?? []).filter((f) => f.status === "U").length);
  /** Tracked files a merge needs that cannot go in yet, with why (said up front). */
  /** "Commit with no changes" is offered: nothing ticked, nothing stopped here. */
  const emptyOffered = $derived(active && files !== null && stopped === null && chosen.length === 0);
  /** The commit asked for is an empty one. */
  const emptyCommit = $derived(emptyOffered && allowEmpty);
  const mergeBlocked = $derived(merging ? (files ?? []).filter((f) => f.status !== "?" && blocked(f) !== null) : []);

  // ---- Reading the files ----

  let asked = 0;
  /** A commit was made: the next list read decides whether anything is left to commit. */
  let backWhenEmpty = false;
  /** git's message for the merge stopped here, once put in the empty field (not again). */
  let offeredMergeMessage: string | null = null;
  async function load() {
    const n = ++asked;
    // The list read is of the repository as this snapshot shows it: a commit of these
    // ticks expects that HEAD, not one that came later; and only one read while this worktree
    // was the active one (the snapshot's HEAD is the active worktree's).
    const head = untrack(() => (active ? snap.head : null));
    try {
      const got = await api.worktreeFiles(view.repoId, path);
      if (n !== asked) return;
      const before = files;
      files = got.files;
      filesHead = head;
      truncated = got.truncated;
      pushRemote = got.pushRemote;
      readError = null;
      // Ticks follow the list: gone files go; new ones come ticked while everything is.
      const known = new Set((before ?? []).map((f) => f.path));
      const next = new Set<string>();
      for (const f of got.files) {
        if (blocked(f) !== null) continue;
        if (ticked.has(f.path) || (tickNew && (before === null || !known.has(f.path)))) next.add(f.path);
      }
      ticked = next;
      // Concluding a merge: an empty field gets git's own message for it, once.
      if (got.mergeMessage !== null && got.mergeMessage !== offeredMergeMessage && untrack(() => active)) {
        offeredMergeMessage = got.mergeMessage;
        if (untrack(() => message).trim() === "") message = got.mergeMessage;
      }
      // A commit just made left nothing: back to the worktree list.
      if (backWhenEmpty) {
        backWhenEmpty = false;
        if (got.files.length === 0 && panel.commitFor === path) closeCommit(view.repoId);
      }
    } catch (e) {
      if (n !== asked) return;
      readError = e instanceof Error ? e.message : String(e);
    }
  }

  // Read again whenever the repository's state moves (a file written, a commit made), at most
  // every 150 ms. Only those are followed: the read's own result must not ask for another.
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    void snap.seq;
    void wt?.status;
    void path;
    const first = untrack(() => files === null);
    clearTimeout(timer);
    timer = setTimeout(() => void load(), first ? 0 : 150);
    return () => clearTimeout(timer);
  });

  // The message is kept for the worktree while the app runs.
  $effect(() => {
    if (message === "") commitDrafts.delete(path);
    else commitDrafts.set(path, message);
  });

  // ---- Ticking ----

  function toggle(f: FileEntry) {
    if (blocked(f) !== null || locked(f) || pending) return;
    const next = new Set(ticked);
    if (!next.delete(f.path)) next.add(f.path);
    else tickNew = false;
    ticked = next;
  }

  const allState = $derived(
    tickable.length === 0 || chosen.length === 0 ? "false" : chosen.length === tickable.length ? "true" : "mixed",
  );

  function tickAll(on: boolean) {
    if (pending) return;
    ticked = on ? new Set(tickable.map((f) => f.path)) : new Set();
    tickNew = on;
  }

  // ---- The list: rows in view, keys ----

  let scroller: HTMLDivElement | undefined = $state();
  let top = $state(0);
  let height = $state(0);
  /** The row the keys are on. */
  let cursor = $state(0);
  const uid = Math.random().toString(36).slice(2, 8);

  /** Each row's top, and the list's height. */
  const layout = $derived.by(() => {
    const tops: number[] = [];
    let y = 0;
    for (const f of files ?? []) {
      tops.push(y);
      y += blocked(f) === null ? ROW_H : NOTED_H;
    }
    return { tops, total: y };
  });

  /** The first row whose bottom is below `y`. */
  function rowAtY(y: number): number {
    const { tops } = layout;
    let lo = 0;
    let hi = tops.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (tops[mid] <= y) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  const first = $derived(Math.max(0, rowAtY(top) - OVERSCAN));
  const last = $derived(Math.min(files?.length ?? 0, rowAtY(top + height) + 1 + OVERSCAN));
  const shown = $derived((files ?? []).slice(first, last));
  /** The row the keys are on, when it is drawn (the active descendant is never a row that is not). */
  const activeRow = $derived.by(() => {
    if (!files || files.length === 0) return null;
    const i = Math.min(cursor, files.length - 1);
    return i >= first && i < last ? i : null;
  });

  function moveTo(i: number) {
    if (!files || files.length === 0 || !scroller) return;
    cursor = Math.max(0, Math.min(files.length - 1, i));
    const y = layout.tops[cursor];
    const h = blocked(files[cursor]) === null ? ROW_H : NOTED_H;
    if (y < scroller.scrollTop) scroller.scrollTop = y;
    else if (y + h > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = y + h - scroller.clientHeight;
    top = scroller.scrollTop;
  }

  function onListKeydown(e: KeyboardEvent) {
    if (!files || files.length === 0) return;
    const page = Math.max(1, Math.floor((scroller?.clientHeight ?? 0) / ROW_H) - 1);
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a" && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      tickAll(true);
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    switch (e.key) {
      case "ArrowDown":
        moveTo(cursor + 1);
        break;
      case "ArrowUp":
        moveTo(cursor - 1);
        break;
      case "PageDown":
        moveTo(cursor + page);
        break;
      case "PageUp":
        moveTo(cursor - page);
        break;
      case "Home":
        moveTo(0);
        break;
      case "End":
        moveTo(files.length - 1);
        break;
      case " ":
        toggle(files[Math.min(cursor, files.length - 1)]);
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  function onRowClick(i: number) {
    cursor = i;
    if (files) toggle(files[i]);
  }

  // ---- Committing ----

  /**
   * Where Commit & Push sends the branch (its namesake), or why it cannot say: the remote
   * the core named with the file list (git's push rule, the one Create pull request uses too).
   */
  const pushTo = $derived.by((): { remote: string; branch: string } | { why: string } | null => {
    const branch = snap.head.branch;
    if (!active || branch === null || snap.remotes.length === 0 || files === null) return null;
    if (pushRemote === null) return { why: `It is not clear which remote to push ${branch} to: commit, then push from the branch's menu.` };
    return { remote: pushRemote, branch };
  });

  const subject = $derived(message.split("\n", 1)[0]);
  const KIND = { merge: "merge", rebase: "rebase", "cherry-pick": "cherry-pick", revert: "revert" } as const;

  /** Why Commit cannot be pressed now, or null. */
  const reason = $derived.by((): string | null => {
    if (!active) return "Make this worktree active to commit.";
    if (readError !== null) return readError;
    if (files === null || filesHead === null) return "Reading the changed files…";
    if (stopped && !merging)
      return `A ${KIND[stopped.kind]} is stopped here: Continue, in the banner, commits its changes (or abort it).`;
    if (merging && conflicts > 0) return "Resolve the conflicts in your editor, then continue: commit here, or Continue in the banner.";
    if (merging && truncated) return "More than 5,000 files changed: conclude this merge in a terminal.";
    if (!emptyCommit) {
      if (files.length === 0) return "Nothing to commit: no file has changed. To commit anyway, tick Commit with no changes.";
      if (chosen.length === 0) return "Tick the files to commit.";
    }
    if (message.trim() === "") return "Write a commit message.";
    return null;
  });
  const pushReason = $derived(pushTo !== null && "why" in pushTo ? pushTo.why : null);

  let field: HTMLTextAreaElement | undefined = $state();

  async function commit(push: boolean) {
    // Only the worktree shown commits, and only while it is the active one: the core refuses
    // a request whose worktree is not active by its turn.
    if (!active || filesHead === null) return;
    if (reason !== null || pending || (push && (pushTo === null || "why" in pushTo))) return;
    const request: OpRequest = {
      op: "commit",
      paths: emptyCommit ? [] : chosen.map((f) => f.path),
      message,
      push,
      empty: emptyCommit,
      expectHead: filesHead,
      expectWorktree: path,
    };
    pending = push ? "push" : "commit";
    try {
      const outcome = await runOp(view.repoId, request);
      // Made, whatever followed (a push that failed, a Stop after it): the form starts afresh.
      // (The outcome's snapshot may still show HEAD as it was until the graph has reloaded.)
      if (outcome.committed !== null) {
        message = "";
        allowEmpty = false;
        ticked = new Set();
        tickNew = false;
        backWhenEmpty = true;
        void load();
      }
    } catch {
      // Reported by runOp; the message stays.
    } finally {
      pending = null;
    }
  }

  function onMessageKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && (isMac() ? e.metaKey : e.ctrlKey) && !e.isComposing) {
      e.preventDefault();
      void commit(false);
    }
  }

  async function activate() {
    switching = true;
    try {
      await makeActive(view.repoId, path, view);
    } finally {
      switching = false;
    }
  }

  // Asked to (Cmd+K, opening): the focus goes to the message, once per request. Shown again
  // (back to its tab) it leaves the focus where it is.
  $effect(() => {
    const request = panel.commitFocus;
    if (request === panel.commitFocusDone) return;
    panel.commitFocusDone = request;
    void tick().then(() => field?.focus());
  });

  // ---- How a row reads ----

  const STATUS_NAMES: Record<FileEntry["status"], string> = {
    M: "Modified",
    A: "Added",
    D: "Deleted",
    R: "Renamed",
    C: "Copied",
    T: "Type changed",
    U: "Conflicted",
    "?": "New, untracked",
  };

  function rowTitle(f: FileEntry): string {
    const p = f.origPath === null ? f.path : `${f.origPath} → ${f.path}`;
    return `${STATUS_NAMES[f.status]}: ${p}${blocked(f) ? `\n${blocked(f)}` : ""}${locked(f) ? "\nPart of the merge: it goes in with it." : ""}`;
  }

  /** The row's accessible name: what changed, and why it cannot be ticked or unticked. */
  function rowLabel(f: FileEntry): string {
    const p = visibleText(f.origPath === null ? f.path : `${f.origPath} to ${f.path}`);
    const why = blocked(f);
    const note = why !== null ? `, ${why}` : locked(f) ? ", goes in with the merge" : "";
    return `${STATUS_NAMES[f.status].toLowerCase()}, ${p}${note}`;
  }

  /** "src/a.ts" → keep the file name: the characters after the last "/". */
  const keepOf = (p: string) => Math.min(48, p.length - p.lastIndexOf("/"));

  const countLine = $derived.by(() => {
    if (files === null) return "";
    const n = files.length;
    const all = `${n}${truncated ? "+" : ""} file${n === 1 ? "" : "s"}`;
    return `${chosen.length} of ${all} ticked`;
  });
</script>

<!-- Escape goes back to the worktree list (before it would hide a panel lying over the graph). -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="commit"
  data-testid="commit-panel"
  onkeydown={(e) => {
    if (e.key === "Escape" && !e.defaultPrevented && pending === null) {
      e.preventDefault();
      closeCommit(view.repoId);
    }
  }}
>
  {#if !active}
    <!-- Why it cannot be made active now, as the worktree row says. -->
    {@const why = wt ? makeActiveReason(wt, snap) : undefined}
    <div class="elsewhere" data-testid="commit-elsewhere">
      <p>Commits run in the active worktree. Make this one active to commit its changes.</p>
      <button class="button primary small" disabled={switching || why !== undefined} onclick={() => void activate()}
        >{switching ? "Switching…" : "Make it active"}</button
      >
      {#if why !== undefined && !switching}
        <p class="why" data-testid="commit-elsewhere-why">{why}.</p>
      {/if}
    </div>
  {:else if merging}
    <p class="banner" data-testid="commit-merge">
      Concluding the merge{stopped?.incoming ? ` of ${visibleText(stopped.incoming)}` : ""}: every changed tracked file goes into it.
      {#each mergeBlocked.slice(0, 3) as f (f.path)}
        <span class="blocked">{visibleText(f.path)} cannot go in yet: {midSentence(blocked(f) ?? "")}</span>
      {/each}
      {#if mergeBlocked.length > 3}
        <span class="blocked">And {mergeBlocked.length - 3} more.</span>
      {/if}
    </p>
  {/if}

  <div class="all">
    <input
      class="checkbox"
      type="checkbox"
      aria-label="Tick every file"
      data-checked={allState === "true"}
      checked={allState === "true"}
      indeterminate={allState === "mixed"}
      disabled={tickable.length === 0 || pending !== null || (merging && tickable.every(locked))}
      onchange={() => tickAll(allState !== "true")}
      data-testid="tick-all"
    />
    <span class="count" data-testid="commit-count">{countLine}</span>
  </div>

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="files"
    role="listbox"
    aria-label="Changed files"
    aria-multiselectable="true"
    tabindex="0"
    aria-activedescendant={activeRow !== null ? `cf-${uid}-${activeRow}` : undefined}
    bind:this={scroller}
    bind:clientHeight={height}
    onscroll={() => (top = scroller!.scrollTop)}
    onkeydown={onListKeydown}
    data-testid="commit-files"
  >
    {#if files === null}
      <p class="state">{readError ?? "Reading the changed files…"}</p>
    {:else if files.length === 0}
      <p class="state">No changes: everything here is committed.</p>
    {:else}
      <div class="inner" style:height="{layout.total}px">
        {#each shown as f, k (f.path)}
          {@const i = first + k}
          {@const why = blocked(f)}
          {@const on = isTicked(f)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            id="cf-{uid}-{i}"
            class="row"
            class:cursor={i === cursor}
            class:off={why !== null}
            class:locked={locked(f)}
            role="option"
            tabindex="-1"
            aria-selected={on}
            aria-disabled={why !== null || locked(f)}
            aria-label={rowLabel(f)}
            title={rowTitle(f)}
            data-path={f.path}
            data-status={f.status}
            style:transform="translateY({layout.tops[i]}px)"
            onclick={() => onRowClick(i)}
          >
            <span class="line">
              <span class="checkbox box" data-checked={on} aria-hidden="true"></span>
              <span class="status s-{f.status === '?' ? 'N' : f.status}" aria-hidden="true">{f.status === "?" ? "?" : f.status}</span>
              <span class="path" class:deleted={f.status === "D"}>
                {#if f.origPath !== null}
                  <MiddleText class="old" text={f.origPath} keep={keepOf(f.origPath)} title={null} dimThrough="/" /><span class="arrow">→</span>
                {/if}
                <MiddleText text={f.path} keep={keepOf(f.path)} title={null} dimThrough="/" />
              </span>
            </span>
            {#if why !== null}
              <span class="why">{why}</span>
            {/if}
          </div>
        {/each}
      </div>
      {#if truncated}
        <p class="more" data-testid="commit-truncated">
          Only the first 5,000 changed files are listed. Commit these, and the rest come next.
        </p>
      {/if}
    {/if}
  </div>

  <div class="compose">
    <div class="label-row">
      <label class="field-label" for="cm-{uid}">Message</label>
      <span class="gauge" class:over={subject.length > SUBJECT_MAX} title="The first line is the summary; {SUBJECT_MAX} characters fit everywhere"
        >{subject.length}/{SUBJECT_MAX}</span
      >
    </div>
    <textarea
      id="cm-{uid}"
      class="text-input message"
      rows="4"
      placeholder={merging ? `Merge ${stopped?.incoming ?? "branch"}` : "Summary of the change"}
      spellcheck="true"
      bind:value={message}
      bind:this={field}
      readonly={pending !== null}
      onkeydown={onMessageKeydown}
      data-testid="commit-message"
    ></textarea>
    {#if active}
      <div class="buttons">
        <button
          class="button primary"
          disabled={reason !== null || pending !== null}
          title="{emptyCommit ? 'Make an empty commit' : 'Commit the ticked files'} ({COMMIT_KEYS})"
          onclick={() => void commit(false)}
          data-testid="commit-button">{pending === "commit" ? "Committing…" : "Commit"}</button
        >
        {#if pushTo !== null}
          <button
            class="button"
            disabled={reason !== null || pushReason !== null || pending !== null}
            onclick={() => void commit(true)}
            data-testid="commit-push-button"
          >
            {#if pending === "push"}
              Committing and pushing…
            {:else if "remote" in pushTo}
              <span class="push-label">Commit & Push to <MiddleText text="{pushTo.remote}/{pushTo.branch}" keep={12} /></span>
            {:else}
              Commit & Push
            {/if}
          </button>
        {/if}
      </div>
      {#if emptyOffered}
        <label class="empty-row" title="A commit that changes no file (git commit --allow-empty), to run CI again, say">
          <input
            class="checkbox"
            type="checkbox"
            bind:checked={allowEmpty}
            disabled={pending !== null}
            data-testid="commit-empty"
          />
          <span>Commit with no changes</span>
        </label>
      {/if}
      {#if (reason ?? pushReason) !== null && pending === null}
        <p class="reason" data-testid="commit-reason">
          <svg width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
            <circle cx="8" cy="8" r="6.3" fill="none" stroke="currentColor" stroke-width="1.4" />
            <path d="M8 7.2v3.9" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
            <circle cx="8" cy="4.9" r="0.95" fill="currentColor" />
          </svg>
          <span>{reason ?? pushReason}</span>
        </p>
      {/if}
    {/if}
  </div>
</div>

<style>
  .commit {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .banner .blocked {
    display: block;
    margin-top: 4px;
  }

  .elsewhere,
  .banner {
    flex: none;
    margin: 10px 10px 2px;
    padding: 9px 11px;
    border-radius: var(--radius);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }

  .elsewhere .why {
    font-size: var(--fs-xs);
  }

  .elsewhere {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    border: 1px solid var(--border-strong);
    background: var(--bg-elevated);
    color: var(--text-muted);
  }

  .banner {
    border: 1px solid color-mix(in srgb, var(--warning) 40%, transparent);
    background: color-mix(in srgb, var(--warning) 8%, transparent);
    color: var(--text);
  }

  .all {
    display: flex;
    align-items: center;
    gap: 9px;
    flex: none;
    height: 30px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
  }

  .all .checkbox:indeterminate {
    border-color: var(--accent);
    background: var(--accent);
  }

  .all .checkbox:indeterminate::before {
    height: 2px;
    clip-path: none;
    transform: scale(1);
  }

  .count {
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    font-weight: 550;
  }

  .files {
    position: relative;
    flex: 1 1 0;
    min-height: 72px;
    overflow: auto;
    outline: none;
  }

  .files:focus-visible {
    box-shadow: inset 0 0 0 2px var(--focus-ring);
  }

  .inner {
    position: relative;
    contain: strict;
    width: 100%;
    margin: 3px 0;
  }

  .state,
  .more {
    padding: 12px;
    color: var(--text-faint);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }

  .more {
    padding-top: 6px;
    color: var(--text-muted);
  }

  .row {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
    min-height: 26px;
    padding: 0 10px 0 12px;
    font-size: var(--fs-sm);
    cursor: default;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .files:focus-visible .row.cursor {
    background: var(--bg-selected);
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 26px;
    min-width: 0;
  }

  .box {
    flex: none;
  }

  .off .box {
    opacity: 0.45;
  }

  .locked .box {
    opacity: 0.6;
  }

  .status {
    display: inline-grid;
    place-items: center;
    flex: none;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    background: color-mix(in srgb, currentColor 13%, transparent);
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 700;
  }

  /* The details panel's file colours (ChangedFiles): A green, M amber, D red, R blue. */
  .s-A {
    color: var(--success);
  }

  .s-M,
  .s-T {
    color: var(--warning);
  }

  .s-D,
  .s-U {
    color: var(--danger);
  }

  .s-R,
  .s-C {
    color: var(--info);
  }

  .s-N {
    color: var(--text-muted);
  }

  .path {
    display: flex;
    align-items: center;
    gap: 5px;
    flex: 1 1 auto;
    min-width: 0;
    white-space: nowrap;
  }

  .path :global(.mid) {
    flex: 0 1 auto;
  }

  .path :global(.old),
  .arrow {
    color: var(--text-muted);
  }

  /* Both sides of a rename give way alike, each keeping its file name. */
  .path :global(.old) {
    flex: 0 1 auto;
  }

  .arrow {
    flex: none;
  }

  .deleted :global(.mid:last-child) {
    color: var(--text-muted);
    text-decoration: line-through;
    text-decoration-color: color-mix(in srgb, var(--danger) 55%, transparent);
  }

  .off .path {
    color: var(--text-muted);
  }

  .why {
    margin: -6px 0 0 46px;
    padding-bottom: 3px;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    line-height: 13px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .compose {
    display: flex;
    flex-direction: column;
    flex: none;
    gap: 6px;
    padding: 10px 12px 12px;
    border-top: 1px solid var(--border);
  }

  .label-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }

  .label-row .field-label {
    margin: 0;
  }

  .gauge {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }

  .gauge.over {
    color: var(--warning);
    font-weight: 600;
  }

  .message {
    min-height: 76px;
    font-size: var(--fs-sm);
  }

  .buttons {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 4px;
  }

  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
    white-space: nowrap;
    transition:
      background-color 80ms linear,
      border-color 80ms linear;
  }

  .button.small {
    height: 26px;
  }

  .button:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .button:active:not(:disabled) {
    background: var(--bg-pressed);
  }

  .button:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .button:disabled {
    border-color: var(--border);
    background: transparent;
    color: var(--text-faint);
  }

  .primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .primary:hover:not(:disabled) {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .primary:disabled {
    border-color: var(--border-strong);
    background: var(--bg-pressed);
    color: var(--text-faint);
  }

  .push-label {
    display: inline-flex;
    gap: 0.3em;
    min-width: 0;
    overflow: hidden;
  }

  .empty-row {
    display: flex;
    align-items: center;
    gap: 7px;
    width: fit-content;
    margin-top: 2px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    cursor: default;
    user-select: none;
  }

  .reason {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin: 2px 0 0;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
  }

  .reason svg {
    flex: none;
    margin-top: 1.5px;
    color: var(--text-faint);
  }
</style>
