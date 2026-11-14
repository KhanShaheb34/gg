<!--
  Continue what git stopped in: the files that are staged as they are now (every file that
  was conflicted; the user resolved them), and what happens then.
-->
<script lang="ts">
  import { visibleText } from "../../format";
  import type { InProgress } from "../../types";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    kind: InProgress["kind"];
    /** The conflicted files, staged by continuing. */
    files: string[];
    close: () => void;
  };

  let { repoId, kind, files, close }: Props = $props();

  const then = $derived(
    {
      merge: "the merge is committed",
      rebase: "the rebase goes on with the next commit",
      "cherry-pick": "the cherry-picked commit is made",
      revert: "the commit that reverts it is made",
    }[kind],
  );
</script>

<Dialog
  title="Continue the {kind}"
  primary={files.length > 0 ? "Stage and continue" : "Continue"}
  onSubmit={() => closeAndRun(close, repoId, { op: "continue", evenWithMarkers: false })}
  onCancel={close}
>
  {#if files.length > 0}
    <p class="dialog-text">
      {files.length === 1 ? "This file is" : "These files are"} staged as {files.length === 1 ? "it is" : "they are"} now,
      then {then}:
    </p>
    <ul class="files">
      {#each files as file (file)}
        <li class="path-tail"><bdi>{visibleText(file)}</bdi></li>
      {/each}
    </ul>
  {:else}
    <p class="dialog-text">Nothing needs staging: {then}.</p>
  {/if}
</Dialog>

<style>
  .files {
    max-height: calc(8 * 20px + 12px);
    overflow-y: auto;
    margin: 10px 0 2px;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    line-height: 20px;
    list-style: none;
  }
</style>
