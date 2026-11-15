<!--
  Checking out a branch that is behind its upstream and has nothing of its own (spec §5.3):
  fast-forward it to the upstream first? Yes checks it out and brings it up to date; No checks
  it out as it is; Cancel does neither.
-->
<script lang="ts">
  import { commits, shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    branch: string;
    /** Its upstream ("origin/staging"). */
    upstream: string;
    /** How many commits it is behind the upstream, as last fetched. */
    behind: number;
    /** The active worktree it was opened for: what it asks for runs only there. */
    worktree: string;
    close: () => void;
  };

  let { repoId, branch, upstream, behind, worktree, close }: Props = $props();

  function checkOut(fastForward: boolean) {
    closeAndRun(close, repoId, { op: "checkout_branch", branch, fastForward, expectWorktree: worktree });
  }
</script>

<Dialog
  title="Fast-forward {commits(behind)} first?"
  primary="Yes"
  secondary="No"
  onSubmit={() => checkOut(true)}
  onSecondary={() => checkOut(false)}
  onCancel={close}
>
  <p class="dialog-text">{shown(branch)} is {commits(behind)} behind {shown(upstream)}.</p>
  <p class="dialog-text muted">Yes brings it up to date as it is checked out; No checks it out as it is.</p>
</Dialog>
