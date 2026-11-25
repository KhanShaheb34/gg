<!--
  The Commit tab's "Committed on branch", under the commit message: the files the branch
  checked out in the worktree shown changes in its own commits, compared from where it left
  its base (the pull request's base, else the default branch of the remote it tracks, else
  the first remote's; branch_files). A header that opens and closes it, with the count and the
  base; each file's name, its folder muted, the lines added and removed (none for a binary
  file) and its status letter in the app's file colours. Read-only: a row opens nothing (the
  app has no view of one file's changes). The first 500, then "and N more".

  None at all when HEAD is detached there, the branch is the default branch itself, no base is
  known or it changes nothing. Closed by default while the worktree has uncommitted changes,
  open while it has none; the user's click holds until that changes (BranchFold).

  Asked for only when what it depends on moves (branchFilesKey), never with every snapshot:
  the core reads git again only for a new HEAD, base or base commit.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { groupDigits, shortHash, visibleText } from "../format";
  import { branchFilesKey, branchFilesOf, loadBranchFiles } from "../state/branchFiles.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import { panelState } from "../state/worktrees.svelte";
  import type { FileChange } from "../types";

  type Props = {
    view: RepoView;
    /** The worktree the Commit tab shows. */
    path: string;
    /** It has uncommitted changes (files in the list above). */
    dirty: boolean;
  };

  let { view, path, dirty }: Props = $props();

  const snap = $derived(view.snapshot);
  const root = $derived(snap.rootPath);
  const wt = $derived(snap.worktrees.find((w) => w.path === path) ?? null);
  const known = $derived(branchFilesOf(root, path));
  /** The base last answered: what the key follows the commit of. */
  const lastBase = $derived(known?.data?.base ?? null);
  const key = $derived(branchFilesKey(snap, path, lastBase));

  // Asked again only when the key moves (and not for a worktree the snapshot does not list).
  $effect(() => {
    const k = key;
    if (k === "") return;
    untrack(() => {
      if (branchFilesOf(root, path)?.key === k) return;
      const at = snap;
      void loadBranchFiles(view.repoId, root, path, k, (base) => branchFilesKey(at, path, base));
    });
  });

  /** The list, while it is of the branch checked out there now. */
  const data = $derived(known?.data && wt?.branch === known.data.branch ? known.data : null);

  const fold = $derived(panelState(view.repoId).branchFold);
  $effect(() => fold.see(path, dirty));
  const open = $derived(fold.isOpen(path, dirty));

  const uid = Math.random().toString(36).slice(2, 8);

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

  const headerTitle = $derived(
    data
      ? `The files ${data.branch}'s commits change since it left ${data.base} (at ${shortHash(data.mergeBase)}): committed work only, not the changes above`
      : "",
  );
</script>

{#if data}
  <section class="committed" class:open aria-labelledby="cb-{uid}" data-testid="branch-files" data-base={data.base}>
    <button
      id="cb-{uid}"
      class="head"
      aria-expanded={open}
      aria-controls="cb-list-{uid}"
      title={headerTitle}
      onclick={() => fold.toggle(path, dirty)}
      data-testid="branch-files-toggle"
    >
      <span class="chevron" aria-hidden="true"></span>
      <span class="label">Committed on branch</span>
      <span class="count" data-testid="branch-files-count">{groupDigits(data.total)}</span>
      <span class="base" data-testid="branch-files-base">vs {visibleText(data.base)}</span>
    </button>
    {#if open}
      <ul class="list" id="cb-list-{uid}" aria-label="Files committed on {data.branch}" data-testid="branch-files-list">
        {#each data.files as f (f.path)}
          {@const [name, folder] = split(f.path)}
          <li class="row" title={rowTitle(f)} aria-label={rowLabel(f)} data-path={f.path} data-status={f.status} data-testid="branch-file">
            <span class="name" class:deleted={f.status === "D"} dir="auto">{visibleText(name)}</span>
            <span class="where">
              {#if folder}<span class="folder" dir="auto">{visibleText(folder)}</span>{/if}
              {#if f.oldPath !== null}<span class="from" dir="auto">← {visibleText(f.oldPath)}</span>{/if}
            </span>
            {#if !binary(f)}
              <span class="counts" aria-hidden="true"
                ><span class="add">+{f.additions ?? 0}</span><span class="del">−{f.deletions ?? 0}</span></span
              >
            {/if}
            <span class="status s-{f.status}" aria-hidden="true">{f.status}</span>
          </li>
        {/each}
        {#if data.total > data.files.length}
          <li class="more" data-testid="branch-files-more">and {groupDigits(data.total - data.files.length)} more</li>
        {/if}
      </ul>
    {/if}
  </section>
{/if}

<style>
  /* Under the message: a header line, and when open the list, which scrolls inside the panel
     rather than pushing the message out of sight. */
  .committed {
    display: flex;
    flex-direction: column;
    flex: 0 0 auto;
    min-height: 0;
    border-top: 1px solid var(--border);
  }

  .committed.open {
    flex: 0 1 auto;
    max-height: 45%;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: none;
    width: 100%;
    height: 30px;
    padding: 0 12px;
    border: 0;
    border-radius: 0;
    background: transparent;
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-align: left;
    cursor: default;
  }

  .head:hover {
    background: var(--bg-hover);
  }

  .head:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px var(--focus-ring);
  }

  /* A small chevron, turning down when open. */
  .chevron {
    flex: none;
    width: 5px;
    height: 5px;
    margin: 0 2px 0 1px;
    border-right: 1.3px solid var(--text-faint);
    border-bottom: 1.3px solid var(--text-faint);
    transform: translateY(-1px) rotate(-45deg);
    transition: transform 80ms var(--ease);
  }

  .open .chevron {
    transform: translateY(-2px) rotate(45deg);
  }

  .label {
    flex: none;
  }

  .count {
    flex: none;
    padding: 0 5px;
    border-radius: 8px;
    background: var(--bg-chrome);
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    line-height: 16px;
  }

  .base {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-weight: 500;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .list {
    flex: 1 1 auto;
    min-height: 0;
    margin: 0;
    padding: 0 0 4px;
    overflow-y: auto;
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
