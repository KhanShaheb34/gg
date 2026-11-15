<!--
  Merge a branch (local or remote) into the current one, with these options: a merge
  commit even when a fast-forward is possible (remembered per repository: read as the dialog
  opens, saved when it merges, if it is known to have changed), squash (stage the changes as one; no merge commit, so the
  first option does not apply), and no commit (stop before committing the merge).
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { api, CoreError } from "../../ipc";
  import { shown } from "../../menus/common";
  import { runOp } from "../../state/ops.svelte";
  import type { HeadInfo, MergeTarget } from "../../types";
  import Dialog from "../Dialog.svelte";

  type Props = {
    repoId: number;
    target: MergeTarget;
    /** The current branch. */
    into: string;
    /** HEAD as the menu showed it: the merge is into that branch (R39). */
    head: HeadInfo;
    close: () => void;
  };

  let { repoId, target, into, head, close }: Props = $props();

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

  /**
   * Merges, and remembers the merge-commit choice: only when it is known to differ from the
   * stored one (the stored one was read, or the box was changed by hand: never the default
   * over a choice not read yet), and not when the core refused the request before running it.
   */
  async function submit() {
    const choice = noFf;
    const save = (saved !== null || touched) && choice !== saved;
    close();
    try {
      await runOp(repoId, { op: "merge", target, noFf: choice && !squash, squash, noCommit, expectHead: head });
    } catch (e) {
      // Reported by runOp.
      if (e instanceof CoreError && e.code === "bad_request") return;
    }
    if (save) api.setRepoPrefs(repoId, { mergeNoFf: choice }).catch(() => {});
  }
</script>

<Dialog title="Merge {shown(target.name)} into {shown(into)}" primary="Merge" onSubmit={() => void submit()} onCancel={close}>
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
