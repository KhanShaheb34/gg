<!--
  Continue what git stopped in. With every file resolved: the files that are staged as they
  are now, and what happens then. `force` ("Continue anyway…", R31): a red confirmation that
  names, apart, the files that still contain conflict markers (committed with them) and those
  git kept one side of (binary, or deleted on one side: what is in the folder now is taken);
  confirming tells the core to go ahead (`evenWithMarkers`).
-->
<script lang="ts">
  import { visibleText } from "../../format";
  import type { ConflictFile, HeadInfo, InProgress } from "../../types";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    kind: InProgress["kind"];
    /** The conflicted files (the first 200), staged by continuing. */
    files: ConflictFile[];
    /** How many are conflicted in all. */
    fileCount: number;
    /** Go ahead although not every file is resolved. */
    force?: boolean;
    /** HEAD as the banner showed it: what is continued is the stop there (R39). */
    head: HeadInfo;
    close: () => void;
  };

  let { repoId, kind, files, fileCount, force = false, head, close }: Props = $props();

  const then = $derived(
    {
      merge: "the merge is committed",
      rebase: "the rebase goes on with the next commit",
      "cherry-pick": "the cherry-picked commit is made",
      revert: "the commit that reverts it is made",
    }[kind],
  );

  const marked = $derived(files.filter((f) => f.state === "markers"));
  const undecided = $derived(files.filter((f) => f.state === "undecided"));
  /** Files counted but not named (past the first 200). */
  const unnamed = $derived(Math.max(0, fileCount - files.length));
  const filesWord = (n: number) => `${n} more file${n === 1 ? "" : "s"}`;
</script>

{#snippet list(items: ConflictFile[], more: number)}
  <ul class="files">
    {#each items as file (file.path)}
      <li class="path-tail"><bdi>{visibleText(file.path)}</bdi></li>
    {/each}
    {#if more > 0}
      <li class="more">and {filesWord(more)}</li>
    {/if}
  </ul>
{/snippet}

{#if force}
  <Dialog
    title="Continue the {kind} anyway?"
    primary="Continue anyway"
    danger
    onSubmit={() => closeAndRun(close, repoId, { op: "continue", evenWithMarkers: true, expectHead: head })}
    onCancel={close}
  >
    <p class="dialog-text">Every conflicted file is staged as it is now, then {then}.</p>
    {#if marked.length > 0}
      <p class="dialog-text lead">
        {marked.length === 1 ? "This file still contains" : "These files still contain"} conflict markers, which are committed with
        {marked.length === 1 ? "it" : "them"}:
      </p>
      {@render list(marked, 0)}
    {/if}
    {#if undecided.length > 0}
      <p class="dialog-text lead">
        {undecided.length === 1 ? "This file has" : "These files have"} no conflict markers to go by (binary, deleted on one side, or too
        large to read here): continuing takes
        what is in your folder now:
      </p>
      {@render list(undecided, 0)}
    {/if}
    {#if unnamed > 0}
      <p class="dialog-text lead">And {filesWord(unnamed)} that {unnamed === 1 ? "was" : "were"} not checked.</p>
    {/if}
  </Dialog>
{:else}
  <Dialog
    title="Continue the {kind}"
    primary={files.length > 0 ? "Stage and continue" : "Continue"}
    onSubmit={() => closeAndRun(close, repoId, { op: "continue", evenWithMarkers: false, expectHead: head })}
    onCancel={close}
  >
    {#if files.length > 0}
      <p class="dialog-text">
        {fileCount === 1 ? "This file is" : "These files are"} staged as {fileCount === 1 ? "it is" : "they are"} now, then {then}:
      </p>
      {@render list(files, unnamed)}
    {:else}
      <p class="dialog-text">Nothing needs staging: {then}.</p>
    {/if}
  </Dialog>
{/if}

<style>
  .lead {
    margin-top: 12px;
  }

  .files {
    max-height: calc(6 * 20px + 12px);
    overflow-y: auto;
    margin: 8px 0 2px;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    line-height: 20px;
    list-style: none;
  }

  .more {
    color: var(--text-muted);
    font-family: var(--font-sans, inherit);
  }
</style>
