<!--
  The files a pull request changes, on its page of the Pull requests page: a header that opens
  and closes it as Checks does (the count, and "+N −M" in all), then the files as "Committed on
  branch" lists them (FileChangeList: name, folder, old path of a rename, lines, status letter;
  read-only, no diff). Read with the rest of the pull request (when its page shows, and on
  Refresh), never on the checks' polls. Open while there are a few; closed when there are very
  many (more than 50), and the first 500 drawn, then "and N more". Its states, in one quiet
  line: reading, none, could not be read (with Retry), more than GitHub lists (3,000).
-->
<script lang="ts">
  import { untrack } from "svelte";
  import { groupDigits } from "../format";
  import { filesOf, inbox, loadFiles } from "../state/inbox.svelte";
  import type { InboxPr } from "../types";
  import FileChangeList from "./FileChangeList.svelte";

  type Props = { pr: InboxPr };
  let { pr }: Props = $props();

  /** Most files drawn at once. */
  const DRAWN = 500;
  /** More than this: closed until opened. */
  const MANY = 50;

  const key = $derived(pr.key);
  const known = $derived(filesOf(pr));
  const data = $derived(known?.data ?? null);
  let chosen = $state<boolean | null>(null);
  const open = $derived(chosen ?? (data !== null && data.files.length <= MANY));
  const totals = $derived(
    data
      ? data.files.reduce((t, f) => ({ add: t.add + (f.additions ?? 0), del: t.del + (f.deletions ?? 0) }), { add: 0, del: 0 })
      : null,
  );

  // When its page shows (another pull request: its own), and on Refresh.
  $effect(() => {
    void key;
    void inbox.refreshTick;
    untrack(() => {
      chosen = null;
      void loadFiles(pr);
    });
  });
</script>

<section class="files" aria-label="Files changed" data-testid="pr-files" data-state={data ? (data.files.length === 0 ? "none" : "listed") : known?.error ? "failed" : "loading"}>
  <button
    class="section-head"
    type="button"
    aria-expanded={data && data.files.length > 0 ? open : undefined}
    disabled={!data || data.files.length === 0}
    data-testid="pr-files-toggle"
    onclick={() => (chosen = !open)}
  >
    <svg class="chevron" class:open class:none={!data || data.files.length === 0} width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M3.5 2l3 3-3 3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
    <span class="title">Files changed</span>
    {#if data}
      <span class="count" data-testid="pr-files-count">{groupDigits(data.files.length)}{data.truncated ? "+" : ""}</span>
      {#if totals && data.files.length > 0}
        <span class="summary" aria-label="{totals.add} lines added, {totals.del} removed"><span class="add">+{groupDigits(totals.add)}</span> <span class="del">−{groupDigits(totals.del)}</span></span>
      {/if}
    {/if}
  </button>
  {#if known?.error}
    <p class="line failed" role="status" data-testid="pr-files-failed">
      <span class="why">Could not read the files: {known.error}</span>
      <button class="retry" type="button" disabled={known.loading} onclick={() => void loadFiles(pr)}>{known.loading ? "Retrying…" : "Retry"}</button>
    </p>
  {:else if !data}
    <p class="line" role="status" data-testid="pr-files-loading">Reading the files…</p>
  {:else if data.files.length === 0}
    <p class="line" data-testid="pr-files-none">No files changed.</p>
  {/if}
  {#if data && data.truncated}
    <p class="line" data-testid="pr-files-truncated">GitHub lists the first {groupDigits(data.files.length)} files of this pull request: open it in the browser for the rest.</p>
  {/if}
  {#if data && open && data.files.length > 0}
    <FileChangeList
      label="Files changed in #{pr.number}"
      files={data.files.slice(0, DRAWN)}
      more={data.files.length - Math.min(DRAWN, data.files.length)}
      testid="pr-file"
      listTestid="pr-files-list"
      moreTestid="pr-files-more"
    />
  {/if}
</section>

<style>
  .files {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  /* As Checks' header: a quiet label and what it holds, the whole row the toggle. */
  .section-head {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    min-width: 0;
    height: 30px;
    padding: 0 12px;
    border: 0;
    background: transparent;
    text-align: left;
  }

  .section-head:not(:disabled):hover {
    background: var(--bg-hover);
  }

  .section-head:focus-visible {
    outline-offset: -2px;
  }

  .chevron {
    flex: none;
    color: var(--text-faint);
    transition: transform 80ms var(--ease);
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  .chevron.none {
    visibility: hidden;
  }

  .title {
    flex: none;
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
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

  .summary {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .add {
    color: var(--success);
  }

  .del {
    color: var(--danger);
  }

  .line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin: 0;
    padding: 0 12px 6px 28px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
  }

  .why {
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

  .retry:disabled {
    color: var(--text-faint);
  }

  /* The rows as "Committed on branch" draws them, indented under the header like Checks'. */
  .files :global(.file-list) {
    margin-bottom: 6px;
  }
</style>
