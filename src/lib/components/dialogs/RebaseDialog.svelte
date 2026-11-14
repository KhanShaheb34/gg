<!--
  Rebase the current branch onto another (local or remote): a one-line confirmation naming
  both, and that the current branch's commits are rewritten.
-->
<script lang="ts">
  import { shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    /** The branch rebased onto. */
    onto: string;
    /** The current branch, rebased. */
    branch: string;
    close: () => void;
  };

  let { repoId, onto, branch, close }: Props = $props();
</script>

<Dialog
  title="Rebase {shown(branch)} onto {shown(onto)}"
  primary="Rebase"
  onSubmit={() => closeAndRun(close, repoId, { op: "rebase", onto })}
  onCancel={close}
>
  <p class="dialog-text">
    The commits of {shown(branch)} that {shown(onto)} lacks are made again on top of it, so they get new hashes:
    if they were pushed, pushing them again takes a force push.
  </p>
</Dialog>
