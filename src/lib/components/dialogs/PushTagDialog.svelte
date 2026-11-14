<!--
  Push a tag to a remote: the only one, or origin, chosen to begin with. (The menu item is
  disabled when there is no remote.)
-->
<script lang="ts">
  import { defaultRemote, shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = { repoId: number; name: string; remotes: string[]; close: () => void };

  let { repoId, name, remotes, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  // svelte-ignore state_referenced_locally
  let remote = $state(defaultRemote(remotes));

  function submit() {
    closeAndRun(close, repoId, { op: "push_tag", name, remote });
  }
</script>

<Dialog title="Push tag {shown(name)}" primary="Push tag" valid={remote !== ""} onSubmit={submit} onCancel={close}>
  <p class="dialog-text">Sends the tag to the remote, so that others get it when they fetch.</p>
  <div class="field">
    <label class="field-label" for="push-remote-{uid}">Remote</label>
    <div class="select-wrap">
      <select id="push-remote-{uid}" class="select" bind:value={remote}>
        {#each remotes as r (r)}
          <option value={r}>{shown(r)}</option>
        {/each}
      </select>
    </div>
  </div>
</Dialog>
