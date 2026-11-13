<!--
  The selected commit's details, docked under the graph: on the left the commit's hash,
  parents, author, committer and date, then its whole message; on the right the files it
  changed (ChangedFiles), as a tree or a flat list. The row's own information shows at once;
  the rest when the core answers (see CommitDetailsLoader). The uncommitted row opens no
  panel.

  Everything that comes from git (message, names, paths) is shown as text, never as HTML,
  each piece in its own bidi isolate so that a right-to-left override in it cannot reorder
  the text around it.
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import { formatDateLong, groupDigits, shortHash } from "../format";
  import { api, CoreError } from "../ipc";
  import { toast } from "../state/app.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import ChangedFiles from "./ChangedFiles.svelte";
  import Splitter from "./Splitter.svelte";

  let { view }: { view: RepoView } = $props();

  const loader = $derived(view.details);

  // Follow the selection; the row (from the graph) is shown until the details arrive.
  $effect(() => {
    const hash = view.selectedHash;
    const index = view.selectedIndex;
    untrack(() => {
      const wasOpen = view.details.hash !== null;
      view.details.show(hash, index === null ? null : (view.cache.get(index) ?? null));
      if (wasOpen && view.details.hash === null) keepFocus();
    });
  });

  // --- Height and how files are shown, remembered for every repository -----------------

  type FileView = "tree" | "list";
  const STORAGE_KEY = "gg.details.v1";
  const DEFAULT_H = 280;
  const MIN_H = 120;
  /** Room the graph keeps above the panel, px (matches the graph area's min-height). */
  const MIN_GRAPH_H = 160;

  function readStored(): { height: number; files: FileView } {
    const out = { height: DEFAULT_H, files: "tree" as FileView };
    try {
      const raw: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
      if (raw && typeof raw === "object") {
        const o = raw as Record<string, unknown>;
        if (typeof o.height === "number" && Number.isFinite(o.height)) out.height = Math.max(MIN_H, Math.round(o.height));
        if (o.files === "tree" || o.files === "list") out.files = o.files;
      }
    } catch {
      // Defaults, then.
    }
    return out;
  }

  /** The height the user chose (kept when the window is too small for it), and the file view. */
  let prefs = $state(readStored());

  function save() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
    } catch {
      // Kept until the window closes.
    }
  }

  let panel: HTMLElement | undefined = $state();
  /** The tallest the panel may be now: the repository view's height less the graph's minimum. */
  let room = $state(Infinity);
  /** The height shown: the chosen one, as far as the window allows. */
  const height = $derived(Math.max(MIN_H, Math.min(prefs.height, room)));

  $effect(() => {
    const host = panel?.parentElement;
    if (!host) return;
    const measure = () => (room = host.clientHeight - MIN_GRAPH_H);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(host);
    return () => observer.disconnect();
  });

  let resizeFrom = 0;
  const resizer = {
    onstart: () => (resizeFrom = height),
    onmove: (dy: number) => (prefs.height = Math.max(MIN_H, Math.min(room, Math.round(resizeFrom - dy)))),
    onend: save,
    onreset: () => {
      prefs.height = DEFAULT_H;
      save();
    },
  };

  function setFileView(files: FileView) {
    prefs.files = files;
    save();
  }

  // --- What is shown ------------------------------------------------------------------------

  const hash = $derived(loader.hash);
  const details = $derived(loader.details);
  const row = $derived(loader.row);
  const parents = $derived(details?.parents ?? row?.parents ?? []);
  const author = $derived(
    details ? { name: details.author, email: details.authorEmail } : row ? { name: row.author, email: row.email } : null,
  );
  const committer = $derived(
    details && (details.committer !== details.author || details.committerEmail !== details.authorEmail)
      ? { name: details.committer, email: details.committerEmail }
      : null,
  );
  const time = $derived(details?.authorTime ?? row?.time ?? null);
  const message = $derived(details?.message ?? row?.subject ?? "");
  /** The message's first line, shown as its title, and the rest as written. */
  const title = $derived(message.split("\n", 1)[0]);
  const rest = $derived(message.slice(title.length));
  /**
   * The information column is as wide as the message's longest line needs (within 48 to 80
   * characters, at most half the panel); the files take the rest.
   */
  const infoCh = $derived.by(() => {
    let longest = 0;
    for (const line of message.split("\n")) longest = Math.max(longest, line.length);
    return Math.min(80, Math.max(48, longest + 4));
  });

  const files = $derived(details?.files ?? []);
  const totals = $derived.by(() => {
    let add = 0;
    let del = 0;
    for (const f of files) {
      add += f.additions ?? 0;
      del += f.deletions ?? 0;
    }
    return { add, del };
  });

  // --- Hash and parents ---------------------------------------------------------------------

  let hashEl: HTMLElement | undefined = $state();

  function copyHash() {
    // Selecting part of the hash with the pointer is not a click to copy it.
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed && hashEl?.contains(selection.anchorNode)) return;
    if (!hash) return;
    navigator.clipboard.writeText(hash).then(
      () => toast("info", "Copied"),
      () => toast("error", "Could not copy the commit hash"),
    );
  }

  function onHashKey(e: KeyboardEvent) {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    copyHash();
  }

  /** Selects commit `target` in the graph and scrolls to it. */
  async function goTo(target: string) {
    // A row number is good only for the rows it was looked up in; look again if they changed.
    for (let attempt = 0; attempt < 2; attempt++) {
      const generation = view.snapshot.generation;
      let index: number | null;
      try {
        index = await api.findRow(view.repoId, target);
      } catch (e) {
        const text = e instanceof Error ? e.message : String(e);
        toast("error", `Could not go to that commit: ${text}`, e instanceof CoreError ? (e.detail ?? undefined) : undefined);
        return;
      }
      if (view.snapshot.generation !== generation) continue;
      if (index === null) {
        toast("info", "That commit is not shown in the graph");
        return;
      }
      await view.selectRow(index, true);
      focusGraph(panel?.parentElement);
      return;
    }
    toast("info", "The graph changed while looking for that commit; try again");
  }

  function focusGraph(host: Element | null | undefined) {
    host?.querySelector<HTMLElement>('[role="grid"]')?.focus({ preventScroll: true });
  }

  function close() {
    view.select(null);
  }

  /**
   * The panel is closing (Escape, its button, or the selection gone after a refresh or a
   * change of filter) and still on screen: a focus inside it (or nowhere) goes to the graph,
   * once the panel is gone.
   */
  function keepFocus() {
    if (!panel) return;
    const focus = document.activeElement;
    if (focus !== document.body && !panel.contains(focus)) return;
    const host = panel.parentElement;
    void tick().then(() => focusGraph(host));
  }

  /** Escape closes the panel from inside it, or when nothing else has the focus. */
  function onWindowKey(e: KeyboardEvent) {
    if (e.key !== "Escape" || e.defaultPrevented || !hash || !panel) return;
    const focus = document.activeElement;
    if (focus !== document.body && focus !== null && !panel.contains(focus)) return;
    e.preventDefault();
    close();
  }

  // --- The message's scroll fade --------------------------------------------------------

  let info: HTMLElement | undefined = $state();
  /** The information column has more below what shows. */
  let more = $state(false);

  function checkMore() {
    if (info) more = info.scrollHeight - info.scrollTop - info.clientHeight > 1;
  }

  $effect(() => {
    void message;
    void committer;
    if (!info) return;
    checkMore();
    const observer = new ResizeObserver(checkMore);
    observer.observe(info);
    return () => observer.disconnect();
  });

  const plural = (n: number, word: string) => `${groupDigits(n)} ${word}${n === 1 ? "" : "s"}`;
</script>

<svelte:window onkeydown={onWindowKey} />

{#snippet parentButton(parent: string)}
  <button class="parent" title="Go to {parent}" onclick={() => goTo(parent)}>{shortHash(parent)}</button>
{/snippet}

{#if hash}
  <section
    class="details"
    aria-label="Commit details"
    data-testid="commit-details"
    bind:this={panel}
    style:height="{height}px"
    style:--info-w="{infoCh}ch"
  >
    <Splitter label="Resize details panel" edge="top" value={height} {...resizer} />

    <div class="info" class:more bind:this={info} onscroll={checkMore}>
      <dl class="fields">
        <dt>Commit</dt>
        <dd>
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <span
            class="hash selectable"
            role="button"
            tabindex="0"
            title="Copy the commit hash"
            data-testid="details-hash"
            bind:this={hashEl}
            onclick={copyHash}
            onkeydown={onHashKey}
            >{hash}<svg class="copy" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
              ><rect x="3.75" y="3.75" width="6.5" height="6.5" rx="1.4" fill="none" stroke="currentColor" stroke-width="1.1" /><path
                d="M8.25 2.4v-.15c0-.55-.45-1-1-1H2.6c-.55 0-1 .45-1 1v4.65c0 .55.45 1 1 1h.15"
                fill="none"
                stroke="currentColor"
                stroke-width="1.1"
                stroke-linecap="round"
              /></svg
            ></span
          >
        </dd>

        <dt>{parents.length > 1 ? "Parents" : "Parent"}</dt>
        <dd class="parents" data-testid="details-parents">
          {#each parents as parent (parent)}
            {@render parentButton(parent)}
          {:else}
            <span class="faint">None (root commit)</span>
          {/each}
        </dd>

        <dt>Author</dt>
        <dd class="selectable" data-testid="details-author">
          {#if author}<bdi>{author.name}</bdi> <span class="email">&lt;<bdi>{author.email}</bdi>&gt;</span>{/if}
        </dd>

        {#if committer}
          <dt>Committer</dt>
          <dd class="selectable" data-testid="details-committer">
            <bdi>{committer.name}</bdi> <span class="email">&lt;<bdi>{committer.email}</bdi>&gt;</span>
          </dd>
        {/if}

        <dt>Date</dt>
        <dd class="selectable date" data-testid="details-date">{time === null ? "" : formatDateLong(time)}</dd>
      </dl>

      <div class="message selectable" data-testid="details-message"><bdi class="title">{title}</bdi>{rest}</div>
    </div>

    <div class="changes">
      <div class="changes-head">
        {#if details}
          <span class="count" data-testid="details-files-count">{plural(files.length, "file")} changed</span>
          {#if files.length > 0}
            <span class="totals"
              ><span class="add" class:zero={totals.add === 0}>+{groupDigits(totals.add)}</span>
              <span class="del" class:zero={totals.del === 0}>−{groupDigits(totals.del)}</span></span
            >
          {/if}
        {:else}
          <span class="count muted">Changed files</span>
        {/if}
        <span class="spacer"></span>
        <div class="seg" role="group" aria-label="Show changed files as">
          <button
            class="icon-btn seg-btn"
            aria-label="Tree"
            aria-pressed={prefs.files === "tree"}
            title="Show as a tree"
            onclick={() => setFileView("tree")}
          >
            <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
              <path
                d="M2.5 2.5h4M4.5 6.5h7M4.5 10.5h7M2.5 2.5v8h2M2.5 6.5h2"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>
          <button
            class="icon-btn seg-btn"
            aria-label="List"
            aria-pressed={prefs.files === "list"}
            title="Show as a list"
            onclick={() => setFileView("list")}
          >
            <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
              <path d="M2.5 3h9M2.5 7h9M2.5 11h9" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" />
            </svg>
          </button>
        </div>
        <button class="icon-btn close" aria-label="Close details" title="Close (Esc)" onclick={close}>
          <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
            <path d="M3.5 3.5l7 7M10.5 3.5l-7 7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
          </svg>
        </button>
      </div>

      {#if details && parents.length > 1}
        <p class="note" data-testid="details-files-note">
          Compared with the first parent {@render parentButton(parents[0])}
        </p>
      {/if}

      {#if loader.error}
        <div class="state">
          <p>Could not load the changes: <bdi class="selectable">{loader.error}</bdi></p>
          <button class="retry" onclick={() => loader.retry()}>Try again</button>
        </div>
      {:else if !details}
        <div class="state loading" aria-live="polite"><p>Loading changes…</p></div>
      {:else if files.length === 0}
        <div class="state"><p>No files changed</p></div>
      {:else}
        <ChangedFiles {files} mode={prefs.files} commit={hash} />
      {/if}
    </div>
  </section>
{/if}

<style>
  .details {
    position: relative;
    display: grid;
    /* The information column fits the message (--info-w), the files take the rest. */
    grid-template-columns: minmax(380px, min(var(--info-w), 50%)) minmax(0, 1fr);
    flex: 0 1 auto;
    min-height: 120px;
    border-top: 1px solid var(--border);
    background: var(--bg);
  }

  /* --- Commit information and message --- */

  .info {
    min-width: 0;
    overflow: auto;
    /* The first field sits on the files header's centre line (36 px). */
    padding: 9px 16px 16px;
  }

  /* More below: the text fades out at the bottom edge (the divider is the files column's, so
     it does not fade with it). */
  .info.more {
    -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 28px), transparent);
    mask-image: linear-gradient(to bottom, #000 calc(100% - 28px), transparent);
  }

  .fields {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 5px 14px;
    margin: 0;
    font-size: var(--fs-sm);
    line-height: 18px;
  }

  dt {
    color: var(--text-faint);
    font-weight: 500;
  }

  dd {
    min-width: 0;
    margin: 0;
    overflow-wrap: anywhere;
  }

  .hash {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    border-radius: 3px;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
    cursor: pointer;
  }

  .hash .copy {
    flex: none;
    color: var(--text-faint);
    transition: color 80ms linear;
  }

  .hash:hover,
  .hash:hover .copy {
    color: var(--accent);
  }

  .parents {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 10px;
  }

  .parent {
    padding: 0;
    border: 0;
    border-radius: 3px;
    background: none;
    color: var(--accent);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    line-height: 18px;
    cursor: pointer;
  }

  .parent:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .email,
  .date {
    color: var(--text-muted);
  }

  .faint {
    color: var(--text-faint);
  }

  .message {
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    font-size: var(--fs);
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .message .title {
    font-weight: 600;
  }

  /* --- Changed files --- */

  .changes {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border-left: 1px solid var(--border);
  }

  .changes-head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
    height: 36px;
    padding: 0 6px 0 14px;
    border-bottom: 1px solid var(--border);
    font-size: var(--fs-sm);
  }

  .count {
    font-weight: 600;
    white-space: nowrap;
  }

  .count.muted {
    color: var(--text-muted);
    font-weight: 500;
  }

  .totals {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }

  .add {
    color: var(--success);
  }

  .del {
    color: var(--danger);
  }

  .add.zero,
  .del.zero {
    color: var(--text-faint);
  }

  .spacer {
    flex: 1;
  }

  .seg {
    display: inline-flex;
    gap: 1px;
    padding: 2px;
    border-radius: 7px;
    background: var(--bg-hover);
  }

  .seg-btn {
    width: 24px;
    height: 22px;
    border-radius: 5px;
  }

  .seg-btn[aria-pressed="true"] {
    background: var(--bg-elevated);
    color: var(--text);
    box-shadow: var(--shadow-tab);
  }

  .close {
    width: 26px;
    height: 26px;
  }

  .note {
    flex: none;
    padding: 6px 14px 0;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .note .parent {
    margin-left: 2px;
    font-size: var(--fs-xs);
    line-height: inherit;
  }

  .state {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 12px 14px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  /* Shown only when the answer takes a moment, so a quick one does not flash. */
  .loading {
    opacity: 0;
    animation: appear 120ms linear 150ms forwards;
  }

  @keyframes appear {
    to {
      opacity: 1;
    }
  }

  .retry {
    height: 24px;
    padding: 0 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    font-size: var(--fs-sm);
  }

  .retry:hover {
    background: var(--bg-hover);
  }
</style>
