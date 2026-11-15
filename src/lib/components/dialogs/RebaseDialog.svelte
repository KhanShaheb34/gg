<!--
  Rebase the current branch onto another (local or remote): a short confirmation naming both
  (in the title), and that the current branch's commits are rewritten.
-->
<script lang="ts">
  import { shown } from "../../menus/common";
  import type { HeadInfo } from "../../types";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    /** The branch rebased onto. */
    onto: string;
    /** The current branch, rebased. */
    branch: string;
    /** HEAD as the menu showed it: the rebase is of that branch (R39). */
    head: HeadInfo;
    /** The active worktree it was opened for: what it asks for runs only there. */
    worktree: string;
    close: () => void;
  };

  let { repoId, onto, branch, head, worktree, close }: Props = $props();
</script>

<Dialog
  title="Rebase {shown(branch)} onto {shown(onto)}"
  primary="Rebase"
  onSubmit={() => closeAndRun(close, repoId, { op: "rebase", onto, expectHead: head, expectWorktree: worktree })}
  onCancel={close}
>
  <p class="dialog-text">Its own commits are made again on top, with new hashes.</p>
  <p class="dialog-text muted">If they were pushed, pushing again takes a force push.</p>
</Dialog>
