<!--
  Delete a local branch: a plain confirmation. If the branch has commits git would lose
  (not merged into its upstream, or into the current branch when it has none), the core
  asks again, saying how many; a merged branch is deleted after this one question.
-->
<script lang="ts">
  import { shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    branch: string;
    /** Its upstream ("origin/x"), which stays, or null. */
    upstream: string | null;
    close: () => void;
  };

  let { repoId, branch, upstream, close }: Props = $props();
</script>

<Dialog
  title="Delete branch {shown(branch)}"
  primary="Delete branch"
  onSubmit={() => closeAndRun(close, repoId, { op: "delete_branch", branch })}
  onCancel={close}
>
  <p class="dialog-text">
    The branch is deleted from this repository{upstream === null ? "" : `; ${shown(upstream)} stays on the remote`}.
  </p>
  <p class="dialog-text muted">If it has commits not merged yet (into its upstream, or else the current branch), you are asked again before they are lost.</p>
</Dialog>
