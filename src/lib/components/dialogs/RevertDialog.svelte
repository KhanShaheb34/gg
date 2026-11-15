<!--
  Revert a commit: a new commit on HEAD undoes its changes. One line to confirm; for a merge
  commit, which parent's side to keep (the changes it brought relative to that parent are
  undone).
-->
<script lang="ts">
  import { shortHash } from "../../format";
  import { shown } from "../../menus/common";
  import type { HeadInfo } from "../../types";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import ParentChoice from "./ParentChoice.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    hash: string;
    subject: string;
    parents: string[];
    /** HEAD as the menu showed it: the new commit goes there (R39). */
    head: HeadInfo;
    close: () => void;
  };

  let { repoId, hash, subject, parents, head, close }: Props = $props();

  const branch = $derived(head.branch);

  let mainline = $state(1);
  const merge = $derived(parents.length > 1);

  function submit() {
    closeAndRun(close, repoId, { op: "revert", hash, mainline: merge ? mainline : null, expectHead: head });
  }
</script>

<Dialog title="Revert {shortHash(hash)}" primary="Revert" onSubmit={submit} onCancel={close}>
  <p class="dialog-text">
    A new commit on {branch === null ? "the detached HEAD" : shown(branch)} undoes the changes of
  </p>
  <div class="dialog-target target">
    <CommitRef {hash} {subject} />
  </div>
  {#if merge}
    <ParentChoice {repoId} {parents} legend="It is a merge: keep the side of" bind:mainline />
  {/if}
</Dialog>

<style>
  .target {
    margin-top: 8px;
  }
</style>
