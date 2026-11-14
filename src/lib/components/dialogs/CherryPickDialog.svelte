<!--
  Cherry-pick a commit onto HEAD: optionally only applying its changes (staged, no commit);
  for a merge commit, which parent its changes are taken relative to. Conflicts are not a
  failure: git stops, and the conflict banner takes it from there.
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

  let noCommit = $state(false);
  let mainline = $state(1);
  const merge = $derived(parents.length > 1);

  function submit() {
    closeAndRun(close, repoId, { op: "cherry_pick", hash, mainline: merge ? mainline : null, noCommit, expectHead: head });
  }
</script>

<Dialog
  title="Cherry-pick {shortHash(hash)} onto {branch === null ? 'HEAD' : shown(branch)}"
  primary="Cherry-pick"
  onSubmit={submit}
  onCancel={close}
>
  <div class="dialog-target">
    <CommitRef {hash} {subject} />
  </div>
  {#if merge}
    <ParentChoice {repoId} {parents} legend="It is a merge: take its changes relative to" bind:mainline />
  {/if}
  <label class="check-row">
    <input class="checkbox" type="checkbox" bind:checked={noCommit} />
    <span>
      Apply the changes without committing
      <span class="check-note">They are left staged, for you to change or commit.</span>
    </span>
  </label>
</Dialog>
