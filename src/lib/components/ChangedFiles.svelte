<!--
  A commit's changed files, as a folder tree or a flat list, for the details panel. Each row
  shows the name, `+a | −d` line counts (or "binary") and a status letter coloured A green,
  M amber, D red, R blue. Only the rows in view are drawn, so ten thousand files cost what
  thirty do; the tree and the sorted list are built once per commit (fileTree.ts).

  Paths come from git: they are shown as text inside bidi isolates, with control and
  bidi-override characters made visible (visibleText); tooltips keep the exact path.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { countWidths, fileTreeOf, flattenTree, listRowsOf, type TreeNode, type TreeRow } from "../fileTree";
  import { visibleText } from "../format";
  import type { FileChange } from "../types";

  type Props = {
    files: FileChange[];
    mode: "tree" | "list";
    /** The commit shown: a new one starts at the top with every folder open. */
    commit: string;
  };

  let { files, mode, commit }: Props = $props();

  /** Height of one row, px. */
  const ROW_H = 24;
  /** Rows drawn beyond each edge of the viewport. */
  const OVERSCAN = 8;

  const tree = $derived(mode === "tree" ? fileTreeOf(files) : null);
  /** Folders closed in the tree, for the commit shown. */
  let closed = $state.raw({ commit: null as string | null, paths: new Set<string>() });
  const NONE: ReadonlySet<string> = new Set();
  const closedPaths = $derived(closed.commit === commit ? closed.paths : NONE);
  const rows: TreeRow[] = $derived(tree ? flattenTree(tree, closedPaths) : listRowsOf(files));
  const widths = $derived(countWidths(files));

  let scroller: HTMLDivElement | undefined = $state();
  let top = $state(0);
  let height = $state(0);
  const first = $derived(Math.max(0, Math.floor(top / ROW_H) - OVERSCAN));
  const last = $derived(Math.min(rows.length, Math.ceil((top + height) / ROW_H) + OVERSCAN));
  const shown = $derived(rows.slice(first, last));
  /** The tree row the keyboard is on (-1: none yet). */
  let active = $state(-1);

  // Another commit, or another way of showing its files: start at the top.
  $effect(() => {
    void commit;
    void mode;
    untrack(() => {
      if (scroller) scroller.scrollTop = 0;
      top = 0;
      active = -1;
    });
  });

  function toggle(node: TreeNode) {
    const paths = new Set(closedPaths);
    if (!paths.delete(node.path)) paths.add(node.path);
    closed = { commit, paths };
  }

  function onRowClick(index: number) {
    const node = rows[index]?.node;
    if (!node || !tree) return;
    active = index;
    if (node.file === null) toggle(node);
  }

  /** Moves the keyboard to tree row `index` and scrolls it into view. */
  function moveTo(index: number) {
    if (rows.length === 0 || !scroller) return;
    active = Math.max(0, Math.min(rows.length - 1, index));
    const y = active * ROW_H;
    if (y < scroller.scrollTop) scroller.scrollTop = y;
    else if (y + ROW_H > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = y + ROW_H - scroller.clientHeight;
    top = scroller.scrollTop;
  }

  /** Arrow keys walk the tree, Left and Right close and open folders (as in a file tree). */
  function onKeydown(e: KeyboardEvent) {
    if (!tree || rows.length === 0 || e.metaKey || e.ctrlKey || e.altKey) return;
    const at = active < 0 ? 0 : Math.min(active, rows.length - 1);
    const current = rows[at];
    const folder = current.node.file === null;
    const isOpen = folder && !closedPaths.has(current.node.path);
    const page = Math.max(1, Math.floor((scroller?.clientHeight ?? 0) / ROW_H) - 1);
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

  /** The tooltip: status and the exact path(s). */
  function rowTitle(file: FileChange): string {
    const path = file.oldPath === null ? file.path : `${file.oldPath} → ${file.path}`;
    return `${STATUS_NAMES[file.status]}: ${path}`;
  }

  /** What assistive technology reads: "modified, src/a.ts, 12 added, 3 deleted". */
  function rowLabel(file: FileChange): string {
    const path = visibleText(file.oldPath === null ? file.path : `${file.oldPath} to ${file.path}`);
    const counts =
      file.additions === null && file.deletions === null
        ? "binary"
        : `${file.additions ?? 0} added, ${file.deletions ?? 0} deleted`;
    return `${STATUS_NAMES[file.status].toLowerCase()}, ${path}, ${counts}`;
  }
</script>

<!-- A scrolled list takes the focus for the keys; a tree's rows are reached through it
     (aria-activedescendant), as the graph's are. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="files"
  role={tree ? "tree" : "list"}
  aria-label="Changed files"
  aria-activedescendant={tree && active >= first && active < last ? `gg-file-${active}` : undefined}
  tabindex="0"
  bind:this={scroller}
  bind:clientHeight={height}
  style:--add-w="{widths.add}ch"
  style:--del-w="{widths.del}ch"
  onscroll={() => (top = scroller!.scrollTop)}
  onkeydown={onKeydown}
>
  <div class="files-inner" style:height="{rows.length * ROW_H}px">
    {#each shown as item, k (`${item.node.file ? "f" : "d"}:${item.node.path}`)}
      {@const index = first + k}
      {@const node = item.node}
      {@const file = node.file}
      {#if file}
        {@const old = oldLabel(file, tree !== null)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row"
          class:flat={!tree}
          class:active={index === active}
          role={tree ? "treeitem" : "listitem"}
          id={tree ? `gg-file-${index}` : undefined}
          aria-label={rowLabel(file)}
          aria-level={tree ? item.depth + 1 : undefined}
          aria-posinset={tree ? item.pos : undefined}
          aria-setsize={tree ? item.size : undefined}
          aria-selected={tree ? index === active : undefined}
          data-row
          data-file={file.path}
          data-status={file.status}
          title={rowTitle(file)}
          style:transform="translateY({index * ROW_H}px)"
          style:--depth={item.depth}
          onclick={() => onRowClick(index)}
        >
          <span class="name selectable" class:deleted={file.status === "D"}
            >{#if old !== null}<bdi class="old">{visibleText(old)}</bdi><span class="arrow">{" → "}</span
              >{/if}{#if !tree}<bdi class="dir">{visibleText(folderOf(node.name))}</bdi><bdi
                >{visibleText(node.name.slice(folderOf(node.name).length))}</bdi
              >{:else}<bdi>{visibleText(node.name)}</bdi>{/if}</span
          >
          <span class="counts"
            >{#if file.additions === null && file.deletions === null}<span class="binary">binary</span
              >{:else}<span class="add" class:zero={!file.additions}>+{file.additions ?? 0}</span>
              <span class="bar">|</span>
              <span class="del" class:zero={!file.deletions}>−{file.deletions ?? 0}</span>{/if}</span
          >
          <span class="status s-{file.status}" aria-hidden="true">{file.status}</span>
        </div>
      {:else}
        {@const isOpen = !closedPaths.has(node.path)}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
        <div
          class="row folder"
          class:active={index === active}
          role="treeitem"
          id="gg-file-{index}"
          aria-level={item.depth + 1}
          aria-posinset={item.pos}
          aria-setsize={item.size}
          aria-expanded={isOpen}
          aria-selected={index === active}
          data-row
          title={node.path}
          style:transform="translateY({index * ROW_H}px)"
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
          ><bdi class="name">{visibleText(node.name)}</bdi>
        </div>
      {/if}
    {/each}
  </div>
</div>

<style>
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

  /*
   * One left edge with the header's text: a folder's chevron at 14 px per level, a file's
   * name where its siblings' names start (chevron 10 + gap 5). The flat list starts at 14.
   */
  .row {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 24px;
    padding: 0 12px 0 calc(29px + var(--depth) * 14px);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }

  .row.flat {
    padding-left: 14px;
  }

  .row.folder {
    gap: 5px;
    padding-left: calc(14px + var(--depth) * 14px);
    color: var(--text-muted);
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .row.active {
    background: var(--bg-selected);
  }

  .chevron {
    flex: none;
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

  .old,
  .dir {
    color: var(--text-muted);
  }

  .arrow {
    color: var(--text-faint);
  }

  /* As wide as the counts the commit needs ("+a | −d"), whatever this row shows, so the
     columns and a "binary" label line up. */
  .counts {
    flex: none;
    width: calc(var(--add-w, 4ch) + var(--del-w, 4ch) + 3ch);
    text-align: right;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-faint);
  }

  /* Widths fit the commit's widest counts (countWidths), so the bars line up down the list. */
  .add,
  .del {
    display: inline-block;
  }

  .add {
    min-width: var(--add-w, 4ch);
    text-align: right;
    color: var(--success);
  }

  .del {
    min-width: var(--del-w, 4ch);
    color: var(--danger);
    text-align: left;
  }

  .add.zero,
  .del.zero {
    color: var(--text-faint);
  }

  .bar {
    color: var(--border-strong);
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
