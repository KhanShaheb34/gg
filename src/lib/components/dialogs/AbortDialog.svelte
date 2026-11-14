<!--
  Abort what git stopped in: a one-line confirmation of what goes back. Red: the changes made
  to resolve its conflicts are lost.
-->
<script lang="ts">
  import { shown } from "../../menus/common";
  import type { InProgress } from "../../types";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    kind: InProgress["kind"];
    /** The branch HEAD is on, or null (detached, as during a rebase). */
    branch: string | null;
    close: () => void;
  };

  let { repoId, kind, branch, close }: Props = $props();

  const who = $derived(branch === null ? (kind === "rebase" ? "The branch" : "HEAD") : shown(branch));
</script>

<Dialog
  title="Abort the {kind}?"
  primary="Abort {kind}"
  danger
  onSubmit={() => closeAndRun(close, repoId, { op: "abort" })}
  onCancel={close}
>
  <p class="dialog-text">
    {who} goes back to where it was before the {kind}; changes made to resolve its conflicts are lost.
  </p>
</Dialog>
