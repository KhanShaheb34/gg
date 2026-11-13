<!--
  The selected commit's details, docked under the graph: on the left the commit's hash,
  parents, author, committer and date, then its whole message; on the right the files it
  changed, as a tree or a flat list. The row's own information shows at once; the rest when
  the core answers (see CommitDetailsLoader). The uncommitted row opens no panel.

  Everything that comes from git (message, names, paths) is shown as text, never as HTML.
  The file list draws only the rows in view, so a commit with ten thousand files is as quick
  to show as one with ten.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { buildFileTree, flattenTree, type TreeNode, type TreeRow } from "../fileTree";
  import { formatDateLong, shortHash } from "../format";
  import { api } from "../ipc";
  import { toast } from "../state/app.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import type { FileChange } from "../types";
  import Splitter from "./Splitter.svelte";

  let { view }: { view: RepoView } = $props();

  const loader = $derived(view.details);

  // Follow the selection; the row (from the graph) is shown until the details arrive.
  $effect(() => {
    const hash = view.selectedHash;
    const index = view.selectedIndex;
    untrack(() => view.details.show(hash, index === null ? null : (view.cache.get(index) ?? null)));
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

  let prefs = $state(readStored());

  function save() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
    } catch {
      // Kept until the window closes.
    }
  }

  let panel: HTMLElement | undefined = $state();
  let resizeFrom = 0;
  const resizer = {
    onstart: () => (resizeFrom = panel?.offsetHeight ?? prefs.height),
    onmove: (dy: number) => {
      const room = (panel?.parentElement?.clientHeight ?? Infinity) - MIN_GRAPH_H;
      prefs.height = Math.max(MIN_H, Math.min(room, Math.round(resizeFrom - dy)));
    },
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
      const index = await api.findRow(view.repoId, target).catch(() => null);
      if (view.snapshot.generation !== generation) continue;
      if (index === null) {
        toast("info", "That commit is not shown in the graph");
        return;
      }
      await view.selectRow(index, true);
      focusGraph();
      return;
    }
  }

  function focusGraph() {
    panel?.parentElement?.querySelector<HTMLElement>('[role="grid"]')?.focus({ preventScroll: true });
  }

  function close() {
    const inside = panel?.contains(document.activeElement) ?? false;
    view.select(null);
    if (inside || document.activeElement === document.body) focusGraph();
  }

  /** Escape closes the panel from inside it, or when nothing else has the focus. */
  function onWindowKey(e: KeyboardEvent) {
    if (e.key !== "Escape" || e.defaultPrevented || !hash || !panel) return;
    const focus = document.activeElement;
    if (focus !== document.body && focus !== null && !panel.contains(focus)) return;
    e.preventDefault();
    close();
  }

  // --- Files --------------------------------------------------------------------------------

  /** Height of one file row, px. */
  const FILE_ROW_H = 24;
  /** Rows drawn beyond each edge of the list's viewport. */
  const FILE_OVERSCAN = 8;

  const tree = $derived(prefs.files === "tree" ? buildFileTree(files) : null);
  /** Folders closed in the tree, for the commit shown. */
  let closed = $state.raw({ hash: null as string | null, paths: new Set<string>() });
  const closedPaths = $derived(closed.hash === hash ? closed.paths : new Set<string>());
  const rows: TreeRow[] = $derived(
    tree
      ? flattenTree(tree, closedPaths)
      : files.map((file) => ({ node: { name: file.path, path: file.path, children: [], file }, depth: 0 })),
  );

  let filesEl: HTMLDivElement | undefined = $state();
  let filesTop = $state(0);
  let filesH = $state(0);
  const first = $derived(Math.max(0, Math.floor(filesTop / FILE_ROW_H) - FILE_OVERSCAN));
  const last = $derived(Math.min(rows.length, Math.ceil((filesTop + filesH) / FILE_ROW_H) + FILE_OVERSCAN));
  const shown = $derived(rows.slice(first, last));
  /** The tree row the keyboard is on (-1: none yet). */
  let active = $state(-1);

  // Another commit, or another way of showing its files: start at the top.
  $effect(() => {
    void hash;
    void prefs.files;
    untrack(() => {
      if (filesEl) filesEl.scrollTop = 0;
      filesTop = 0;
      active = -1;
    });
  });

  function toggle(node: TreeNode) {
    const paths = new Set(closedPaths);
    if (!paths.delete(node.path)) paths.add(node.path);
    closed = { hash, paths };
  }

  function onRowClick(index: number) {
    const node = rows[index]?.node;
    if (!node || !tree) return;
    active = index;
    if (node.file === null) toggle(node);
  }

  /** Moves the keyboard to tree row `index` and scrolls it into view. */
  function moveTo(index: number) {
    if (rows.length === 0 || !filesEl) return;
    active = Math.max(0, Math.min(rows.length - 1, index));
    const top = active * FILE_ROW_H;
    if (top < filesEl.scrollTop) filesEl.scrollTop = top;
    else if (top + FILE_ROW_H > filesEl.scrollTop + filesEl.clientHeight)
      filesEl.scrollTop = top + FILE_ROW_H - filesEl.clientHeight;
    filesTop = filesEl.scrollTop;
  }

  /** Arrow keys walk the tree, Left and Right close and open folders (as in a file tree). */
  function onTreeKey(e: KeyboardEvent) {
    if (!tree || rows.length === 0 || e.metaKey || e.ctrlKey || e.altKey) return;
    const at = active < 0 ? 0 : Math.min(active, rows.length - 1);
    const current = rows[at];
    const folder = current.node.file === null;
    const isOpen = folder && !closedPaths.has(current.node.path);
    const page = Math.max(1, Math.floor((filesEl?.clientHeight ?? 0) / FILE_ROW_H) - 1);
    switch (e.key) {
      case "ArrowDown":
        moveTo(active < 0 ? 0 : at + 1);
        break;
      case "ArrowUp":
        moveTo(active < 0 ? 0 : at - 1);
        break;
      case "PageDown":
        moveTo(at + page);
        break;
      case "PageUp":
        moveTo(at - page);
        break;
      case "Home":
        moveTo(0);
        break;
      case "End":
        moveTo(rows.length - 1);
        break;
      case "ArrowRight":
        if (!folder) return;
        if (isOpen) moveTo(at + 1);
        else {
          toggle(current.node);
          active = at;
        }
        break;
      case "ArrowLeft": {
        if (isOpen) {
          toggle(current.node);
          active = at;
          break;
        }
        let parent = at - 1;
        while (parent >= 0 && rows[parent].depth >= current.depth) parent--;
        if (parent < 0) return;
        moveTo(parent);
        break;
      }
      case "Enter":
      case " ":
        if (!folder) return;
        toggle(current.node);
        active = at;
        break;
      default:
        return;
    }
    e.preventDefault();
  }

  /** The folder part of `path`, with its trailing `/` ("" at the root). */
  const folderOf = (path: string) => path.slice(0, path.lastIndexOf("/") + 1);

  /**
   * Where a renamed or copied file came from, as its row shows it: in the list the old path;
   * in the tree the old name when it stayed in its folder, else the old path.
   */
  function oldLabel(file: FileChange, inTree: boolean): string | null {
    if (file.oldPath === null) return null;
    if (!inTree) return file.oldPath;
    const dir = folderOf(file.oldPath);
    // Moved within its folder: just the old name; from elsewhere: the whole old path.
    return dir === folderOf(file.path) ? file.oldPath.slice(dir.length) : file.oldPath;
  }

  const STATUS_NAMES: Record<FileChange["status"], string> = {
    A: "Added",
    M: "Modified",
    D: "Deleted",
    R: "Renamed",
    C: "Copied",
    T: "Type changed",
  };

  function rowTitle(file: FileChange): string {
    const path = file.oldPath === null ? file.path : `${file.oldPath} → ${file.path}`;
    return `${STATUS_NAMES[file.status]}: ${path}`;
  }

  const plural = (n: number, word: string) => `${n.toLocaleString("en-US")} ${word}${n === 1 ? "" : "s"}`;
</script>

<svelte:window onkeydown={onWindowKey} />

{#if hash}
  <section
    class="details"
    aria-label="Commit details"
    data-testid="commit-details"
    bind:this={panel}
    style:height="{prefs.height}px"
  >
    <Splitter label="Resize details panel" edge="top" value={prefs.height} {...resizer} />

    <div class="info">
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
            <button class="parent" title="Go to {parent}" onclick={() => goTo(parent)}>{shortHash(parent)}</button>
          {:else}
            <span class="faint">None (root commit)</span>
          {/each}
        </dd>

        <dt>Author</dt>
        <dd class="selectable" data-testid="details-author">
          {#if author}{author.name} <span class="email">&lt;{author.email}&gt;</span>{/if}
        </dd>

        {#if committer}
          <dt>Committer</dt>
          <dd class="selectable" data-testid="details-committer">
            {committer.name} <span class="email">&lt;{committer.email}&gt;</span>
          </dd>
        {/if}

        <dt>Date</dt>
        <dd class="selectable date" data-testid="details-date">{time === null ? "" : formatDateLong(time)}</dd>
      </dl>

      <div class="message selectable" data-testid="details-message"><span class="title">{title}</span>{rest}</div>
    </div>

    <div class="changes">
      <div class="changes-head">
        {#if details}
          <span class="count" data-testid="details-files-count">{plural(files.length, "file")} changed</span>
          {#if files.length > 0}
            <span class="totals"
              ><span class="add" class:zero={totals.add === 0}>+{totals.add.toLocaleString("en-US")}</span>
              <span class="del" class:zero={totals.del === 0}>−{totals.del.toLocaleString("en-US")}</span></span
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
              <path
                d="M2.5 3h9M2.5 7h9M2.5 11h9"
                fill="none"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linecap="round"
              />
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
        <p class="note" data-testid="details-files-note">Compared with the first parent {shortHash(parents[0])}</p>
      {/if}

      {#if loader.error}
        <div class="state">
          <p>Could not load the changes: <span class="selectable">{loader.error}</span></p>
          <button class="retry" onclick={() => loader.retry()}>Try again</button>
        </div>
      {:else if !details}
        <div class="state loading" aria-live="polite"><p>Loading changes…</p></div>
      {:else if files.length === 0}
        <div class="state"><p>No files changed</p></div>
      {:else}
        <!-- A scrolled list takes the focus for the keys; a tree's rows are reached through it
             (aria-activedescendant), as the graph's are. -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <div
          class="files"
          role={tree ? "tree" : "list"}
          aria-label="Changed files"
          aria-activedescendant={tree && active >= first && active < last ? `gg-file-${active}` : undefined}
          tabindex="0"
          bind:this={filesEl}
          bind:clientHeight={filesH}
          onscroll={() => (filesTop = filesEl!.scrollTop)}
          onkeydown={onTreeKey}
        >
          <div class="files-inner" style:height="{rows.length * FILE_ROW_H}px">
            {#each shown as item, k (`${item.node.file ? "f" : "d"}:${item.node.path}`)}
              {@const index = first + k}
              {@const node = item.node}
              {@const file = node.file}
              {#if file}
                {@const old = oldLabel(file, tree !== null)}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <div
                  class="file-row"
                  class:flat={!tree}
                  class:active={index === active}
                  role={tree ? "treeitem" : "listitem"}
                  id={tree ? `gg-file-${index}` : undefined}
                  aria-level={tree ? item.depth + 1 : undefined}
                  aria-selected={tree ? index === active : undefined}
                  data-row
                  data-file={file.path}
                  data-status={file.status}
                  title={rowTitle(file)}
                  style:transform="translateY({index * FILE_ROW_H}px)"
                  style:--depth={item.depth}
                  onclick={() => onRowClick(index)}
                >
                  <span class="name selectable" class:deleted={file.status === "D"}
                    >{#if old !== null}<span class="old">{old}</span><span class="arrow">{" → "}</span>{/if}{#if !tree}<span
                        class="dir">{folderOf(node.name)}</span
                      >{node.name.slice(folderOf(node.name).length)}{:else}{node.name}{/if}</span
                  >
                  <span class="counts"
                    >{#if file.additions === null && file.deletions === null}<span class="binary">binary</span
                      >{:else}<span class="add" class:zero={!file.additions}>+{file.additions ?? 0}</span>
                      <span class="bar">|</span>
                      <span class="del" class:zero={!file.deletions}>−{file.deletions ?? 0}</span>{/if}</span
                  >
                  <span class="status s-{file.status}">{file.status}</span>
                </div>
              {:else}
                {@const isOpen = !closedPaths.has(node.path)}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
                <div
                  class="file-row folder"
                  class:active={index === active}
                  role="treeitem"
                  id="gg-file-{index}"
                  aria-level={item.depth + 1}
                  aria-expanded={isOpen}
                  aria-selected={index === active}
                  data-row
                  style:transform="translateY({index * FILE_ROW_H}px)"
                  style:--depth={item.depth}
                  onclick={() => onRowClick(index)}
                >
                  <svg class="chevron" class:open={isOpen} width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"
                    ><path
                      d="M3.5 2l3 3-3 3"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.3"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    /></svg
                  ><span class="name">{node.name}</span>
                </div>
              {/if}
            {/each}
          </div>
        </div>
      {/if}
    </div>
  </section>
{/if}

<style>
  .details {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    flex: 0 1 auto;
    min-height: 120px;
    border-top: 1px solid var(--border);
    background: var(--bg);
  }

  /* --- Commit information and message --- */

  .info {
    min-width: 0;
    overflow: auto;
    padding: 12px 16px 16px;
    border-right: 1px solid var(--border);
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

  .hash:hover {
    color: var(--accent);
  }

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

  .files {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 0;
    outline: none;
  }

  .files:focus-visible {
    box-shadow: inset 0 0 0 2px var(--focus-ring);
  }

  .files-inner {
    position: relative;
    contain: strict;
    width: 100%;
  }

  .file-row {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 24px;
    padding: 0 12px 0 calc(17px + var(--depth) * 14px);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }

  .file-row.flat {
    padding-left: 14px;
  }

  .file-row:hover {
    background: var(--bg-hover);
  }

  .file-row.active {
    background: var(--bg-selected);
  }

  .folder {
    gap: 5px;
    padding-left: calc(2px + var(--depth) * 14px);
    color: var(--text-muted);
  }

  .chevron {
    flex: none;
    margin-left: 0;
    color: var(--text-faint);
    transition: transform 80ms var(--ease);
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  .name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .name.deleted {
    color: var(--text-muted);
    text-decoration: line-through;
    text-decoration-color: color-mix(in srgb, var(--danger) 55%, transparent);
  }

  .old {
    color: var(--text-muted);
  }

  .arrow {
    color: var(--text-faint);
  }

  .counts {
    flex: none;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-faint);
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

  .bar {
    color: var(--border-strong);
  }

  .dir {
    color: var(--text-muted);
  }

  .binary {
    font-family: var(--font-ui);
    font-style: italic;
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

  .s-A {
    color: var(--success);
  }

  .s-M,
  .s-T {
    color: var(--warning);
  }

  .s-D {
    color: var(--danger);
  }

  .s-R,
  .s-C {
    color: var(--info);
  }
</style>
