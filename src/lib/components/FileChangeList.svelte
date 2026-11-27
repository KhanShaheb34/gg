<!--
  A read-only list of the files a change touches, as the Commit tab's "Committed on branch"
  shows them (and a pull request's page, its Files): each file's name, its folder muted (its
  old path after a rename), the lines added and removed (none when there is no count, as for a
  binary file) and its status letter in the app's file colours. A row opens nothing (gg has no
  view of one file's changes). `more`: how many more there are than listed ("and N more").
-->
<script lang="ts">
  import { groupDigits, visibleText } from "../format";
  import type { FileChange } from "../types";

  type Props = {
    id?: string;
    /** What the list is, for a screen reader. */
    label: string;
    files: FileChange[];
    more?: number;
    testid: string;
    listTestid: string;
    moreTestid: string;
  };

  let { id, label, files, more = 0, testid, listTestid, moreTestid }: Props = $props();

  const STATUS_NAMES: Record<FileChange["status"], string> = {
    M: "Modified",
    A: "Added",
    D: "Deleted",
    R: "Renamed",
    C: "Copied",
    T: "Type changed",
  };

  /** "src/lib/a.ts" → ["a.ts", "src/lib"]. */
  function split(p: string): [string, string] {
    const i = p.lastIndexOf("/");
    return i < 0 ? [p, ""] : [p.slice(i + 1), p.slice(0, i)];
  }

  const binary = (f: FileChange) => f.additions === null && f.deletions === null;

  function rowTitle(f: FileChange): string {
    const p = f.oldPath === null ? f.path : `${f.oldPath} → ${f.path}`;
    const counts = binary(f) ? "binary" : `+${f.additions ?? 0} −${f.deletions ?? 0}`;
    return `${STATUS_NAMES[f.status]}: ${p} (${counts})`;
  }

  function rowLabel(f: FileChange): string {
    const p = visibleText(f.oldPath === null ? f.path : `${f.oldPath} to ${f.path}`);
    const counts = binary(f) ? "binary" : `${f.additions ?? 0} added, ${f.deletions ?? 0} deleted`;
    return `${STATUS_NAMES[f.status].toLowerCase()}, ${p}, ${counts}`;
  }
</script>

<ul class="file-list" {id} aria-label={label} data-testid={listTestid}>
  {#each files as f (f.path)}
    {@const [name, folder] = split(f.path)}
    <li class="row" title={rowTitle(f)} aria-label={rowLabel(f)} data-path={f.path} data-status={f.status} data-testid={testid}>
      <span class="name" class:deleted={f.status === "D"} dir="auto">{visibleText(name)}</span>
      <span class="where">
        {#if folder}<span class="folder" dir="auto">{visibleText(folder)}</span>{/if}
        {#if f.oldPath !== null}<span class="from" dir="auto">← {visibleText(f.oldPath)}</span>{/if}
      </span>
      {#if !binary(f)}
        <span class="counts" aria-hidden="true"><span class="add">+{f.additions ?? 0}</span><span class="del">−{f.deletions ?? 0}</span></span>
      {/if}
      <span class="status s-{f.status}" aria-hidden="true">{f.status}</span>
    </li>
  {/each}
  {#if more > 0}
    <li class="more" data-testid={moreTestid}>and {groupDigits(more)} more</li>
  {/if}
</ul>

<style>
  .file-list {
    margin: 0;
    padding: 0 0 4px;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 24px;
    padding: 0 10px 0 24px;
    font-size: var(--fs-sm);
    cursor: default;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .name {
    flex: 0 1 auto;
    min-width: 2.5em;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    unicode-bidi: isolate;
  }

  .name.deleted {
    color: var(--text-muted);
    text-decoration: line-through;
    text-decoration-color: color-mix(in srgb, var(--danger) 55%, transparent);
  }

  .where {
    display: flex;
    gap: 6px;
    flex: 1 1 0;
    min-width: 0;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }

  .folder,
  .from {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    unicode-bidi: isolate;
  }

  .counts {
    display: flex;
    gap: 5px;
    flex: none;
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }

  .add {
    color: var(--success);
  }

  .del {
    color: var(--danger);
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

  /* The app's file colours (ChangedFiles, the commit list above): A green, M amber, D red, R blue. */
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

  .more {
    padding: 4px 12px 2px 24px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }
</style>
