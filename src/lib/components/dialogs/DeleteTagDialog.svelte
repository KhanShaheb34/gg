<!--
  Delete a tag here and, if asked, on a remote too (the remote goes first: if it cannot be
  reached, nothing changes). With one remote the choice names it; with several, a list
  (origin chosen to begin with; without origin, none until one is chosen: R29). Without
  remotes, only the local tag is deleted.
-->
<script lang="ts">
  import { defaultRemote, shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import RemoteSelect from "./RemoteSelect.svelte";
  import { closeAndRun } from "./run";

  type Props = { repoId: number; name: string; remotes: string[]; close: () => void };

  let { repoId, name, remotes, close }: Props = $props();

  let alsoRemote = $state(false);
  // svelte-ignore state_referenced_locally
  let remote = $state(defaultRemote(remotes));

  function submit() {
    closeAndRun(close, repoId, { op: "delete_tag", name, remote: alsoRemote ? remote : null });
  }
</script>

<Dialog
  title="Delete tag {shown(name)}"
  primary="Delete tag"
  danger
  valid={!alsoRemote || remote !== ""}
  onSubmit={submit}
  onCancel={close}
>
  <p class="dialog-text">The tag is deleted from this repository. The commit it marks stays.</p>
  {#if remotes.length > 0}
    <label class="check-row">
      <input class="checkbox" type="checkbox" bind:checked={alsoRemote} />
      <span>
        {remotes.length === 1 ? `Also delete it on ${shown(remotes[0])}` : "Also delete it on a remote"}
        <span class="check-note">Others then no longer get it when they fetch.</span>
      </span>
    </label>
    {#if remotes.length > 1}
      <div class="remote">
        <RemoteSelect {remotes} bind:value={remote} disabled={!alsoRemote} />
      </div>
    {/if}
  {/if}
</Dialog>

<style>
  .remote {
    margin-left: 23px;
  }

  .remote :global(.field:has(select:disabled)) {
    opacity: 0.55;
  }
</style>
