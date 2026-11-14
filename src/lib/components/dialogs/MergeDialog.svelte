<!--
  Merge a branch (local or remote) into the current one, with Git Graph's options: a merge
  commit even when a fast-forward is possible (remembered per repository: read as the dialog
  opens, saved when it merges), squash (stage the changes as one; no merge commit, so the
  first option does not apply), and no commit (stop before committing the merge).
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../../ipc";
  import { shown } from "../../menus/common";
  import type { MergeTarget } from "../../types";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    target: MergeTarget;
    /** The current branch. */
    into: string;
    close: () => void;
  };

  let { repoId, target, into, close }: Props = $props();

  let noFf = $state(true);
  let squash = $state(false);
  let noCommit = $state(false);
  /** The repository's saved choice, once read; changed by hand before then: kept. */
  let saved: boolean | null = null;
  let touched = false;

  onMount(() => {
    api
      .getRepoPrefs(repoId)
      .then((prefs) => {
        saved = prefs.mergeNoFf;
        if (!touched) noFf = prefs.mergeNoFf;
      })
      .catch(() => {}); // the default stands
  });

  function submit() {
    if (noFf !== saved) api.setRepoPrefs(repoId, { mergeNoFf: noFf }).catch(() => {});
    closeAndRun(close, repoId, { op: "merge", target, noFf: noFf && !squash, squash, noCommit });
  }
</script>

<Dialog title="Merge {shown(target.name)} into {shown(into)}" primary="Merge" onSubmit={submit} onCancel={close}>
  <label class="check-row first">
    <input
      class="checkbox"
      type="checkbox"
      bind:checked={noFf}
      disabled={squash}
      onchange={() => (touched = true)}
    />
    <span>
      Create a merge commit even if fast-forward is possible
      <span class="check-note">Remembered for this repository.</span>
    </span>
  </label>
  <label class="check-row">
    <input class="checkbox" type="checkbox" bind:checked={squash} />
    <span>
      Squash the commits
      <span class="check-note">Staged as one change for you to commit; no merge is recorded.</span>
    </span>
  </label>
  <label class="check-row">
    <input class="checkbox" type="checkbox" bind:checked={noCommit} />
    <span>
      No commit
      <span class="check-note">Stops before committing, with the result staged.</span>
    </span>
  </label>
</Dialog>

<style>
  .first {
    margin-top: 4px;
  }

  .check-row:has(.checkbox:disabled) {
    opacity: 0.55;
  }
</style>
