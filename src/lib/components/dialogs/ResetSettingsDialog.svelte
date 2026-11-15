<!--
  "Reset to defaults…" from the settings: says what goes back to what, and what stays.
-->
<script lang="ts">
  import { restoreDefaults } from "../../state/settings.svelte";
  import Dialog from "../Dialog.svelte";

  type Props = { close: () => void; onReset: () => void };

  let { close, onReset }: Props = $props();

  let busy = $state(false);
  let problem = $state<string | null>(null);

  async function reset() {
    busy = true;
    problem = null;
    try {
      await restoreDefaults();
    } catch (e) {
      problem = e instanceof Error ? e.message : String(e);
      return;
    } finally {
      busy = false;
    }
    close();
    onReset();
  }
</script>

<Dialog
  title="Reset all settings?"
  primary={busy ? "Resetting…" : "Reset settings"}
  valid={!busy}
  once={false}
  cancellable={!busy}
  onSubmit={() => void reset()}
  onCancel={close}
>
  <p class="dialog-text">
    The theme follows the system, gg fetches every 5 minutes, the editor and terminal are the system’s own, new
    worktrees go beside the repository, and dates show as “12 Nov 2026 16:02”.
  </p>
  <p class="dialog-text muted">Your recent repositories, open tabs and each repository’s own choices stay.</p>
  {#if problem}
    <p class="problem-text" role="alert">{problem}</p>
  {/if}
</Dialog>

<style>
  .problem-text {
    margin-top: 10px;
    color: var(--danger);
    font-size: var(--fs-sm);
  }
</style>
