<!--
  Delete a tag here and, if asked, on a remote too (the remote goes first: if it cannot be
  reached, nothing changes). With one remote the choice names it; with several, a list
  (origin chosen to begin with). Without remotes, only the local tag is deleted.
-->
<script lang="ts">
  import { shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = { repoId: number; name: string; remotes: string[]; close: () => void };

  let { repoId, name, remotes, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  let alsoRemote = $state(false);
  // svelte-ignore state_referenced_locally
  let remote = $state(remotes.includes("origin") ? "origin" : (remotes[0] ?? ""));

  function submit() {
    closeAndRun(close, repoId, { op: "delete_tag", name, remote: alsoRemote ? remote : null });
  }
</script>

<Dialog title="Delete tag {shown(name)}" primary="Delete tag" danger onSubmit={submit} onCancel={close}>
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
      <div class="field remote">
        <label class="field-label" for="delete-remote-{uid}">Remote</label>
        <div class="select-wrap">
          <select id="delete-remote-{uid}" class="select" bind:value={remote} disabled={!alsoRemote}>
            {#each remotes as r (r)}
              <option value={r}>{shown(r)}</option>
            {/each}
          </select>
        </div>
      </div>
    {/if}
  {/if}
</Dialog>

<style>
  .remote {
    margin-left: 23px;
  }

  .remote:has(select:disabled) {
    opacity: 0.55;
  }
</style>
