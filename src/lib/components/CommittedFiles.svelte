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
  the core reads git again only for a new HEAD, base or base commit. A read that fails keeps
  what was shown and says so in one line with Retry; with nothing shown yet, the header and
  that line alone.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { groupDigits, shortHash, visibleText } from "../format";
  import { branchFilesKey, branchFilesOf, loadBranchFiles } from "../state/branchFiles.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import { panelState } from "../state/worktrees.svelte";
  import FileChangeList from "./FileChangeList.svelte";

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
  const key = $derived(branchFilesKey(snap, path));

  // Asked again only when the key moves (and not for a worktree the snapshot does not list),
  // or when the last request for it failed (its key then stands unanswered).
  $effect(() => {
    const k = key;
    if (k === "") return;
    untrack(() => {
      if (branchFilesOf(root, path)?.key === k) return;
      void loadBranchFiles(view.repoId, root, path, k);
    });
  });

  function retry() {
    if (key !== "") void loadBranchFiles(view.repoId, root, path, key);
  }

  /** The list, while it is of the branch checked out there now. */
  const data = $derived(known?.data && wt?.branch === known.data.branch ? known.data : null);
  /** Why the last read failed (said while there is a branch to read). */
  const error = $derived(wt?.branch ? (known?.error ?? null) : null);
  const retrying = $derived(known?.loading ?? false);

  const fold = $derived(panelState(view.repoId).branchFold);
  $effect(() => fold.see(path, dirty));
  const open = $derived(fold.isOpen(path, dirty));

  const uid = Math.random().toString(36).slice(2, 8);

  const headerTitle = $derived(
    data
      ? `The files ${data.branch}'s commits change since it left ${data.base} (at ${shortHash(data.mergeBase)}): committed work only, not the changes above`
      : "",
  );
</script>

{#snippet failure(why: string)}
  <p class="error" role="status" data-testid="branch-files-error">
    <span class="why">Could not read the branch's files: {why}</span>
    <button class="retry" disabled={retrying} onclick={retry} data-testid="branch-files-retry">{retrying ? "Retrying…" : "Retry"}</button>
  </p>
{/snippet}

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
    {#if error !== null}{@render failure(error)}{/if}
    {#if open}
      <FileChangeList
        id="cb-list-{uid}"
        label="Files committed on {data.branch}"
        files={data.files}
        more={data.total - data.files.length}
        testid="branch-file"
        listTestid="branch-files-list"
        moreTestid="branch-files-more"
      />
    {/if}
  </section>
{:else if error !== null}
  <!-- Nothing read yet: the header alone, and why. -->
  <section class="committed" aria-labelledby="cb-{uid}" data-testid="branch-files">
    <p id="cb-{uid}" class="head static" data-testid="branch-files-toggle">
      <span class="label">Committed on branch</span>
    </p>
    {@render failure(error)}
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

  .head:hover:not(.static) {
    background: var(--bg-hover);
  }

  .head.static {
    margin: 0;
  }

  /* A read that failed: one quiet line, its Retry at the end. */
  .error {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin: 0;
    padding: 0 12px 6px 24px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
  }

  .error .why {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .retry {
    flex: none;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 550;
  }

  .retry:hover:not(:disabled) {
    text-decoration: underline;
  }

  .retry:disabled {
    color: var(--text-faint);
  }

  .retry:focus-visible {
    border-radius: 3px;
    outline-offset: 2px;
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

  /* The list scrolls inside the panel rather than pushing the message out of sight. */
  .committed :global(.file-list) {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }
</style>
