<!--
  The "Remote" list of a dialog that sends something to a remote (push, push tag, delete
  tag). While nothing is chosen in it (several remotes, none of them origin: R29), it says
  to choose one, and the dialog's primary button stays disabled.
-->
<script lang="ts">
  import { CHOOSE_REMOTE, shown } from "../../menus/common";

  type Props = { remotes: string[]; value: string; disabled?: boolean };

  let { remotes, value = $bindable(), disabled = false }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
</script>

<div class="field">
  <label class="field-label" for="remote-{uid}">Remote</label>
  <div class="select-wrap">
    <select
      id="remote-{uid}"
      class="select"
      bind:value
      {disabled}
      aria-describedby={value === "" ? `remote-hint-${uid}` : undefined}
    >
      {#if value === ""}
        <option value="" disabled>Choose a remote…</option>
      {/if}
      {#each remotes as r (r)}
        <option value={r}>{shown(r)}</option>
      {/each}
    </select>
  </div>
  {#if value === "" && !disabled}
    <p class="field-hint" id="remote-hint-{uid}">{CHOOSE_REMOTE}</p>
  {/if}
</div>
