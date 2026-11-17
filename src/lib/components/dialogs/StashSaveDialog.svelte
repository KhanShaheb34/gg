<!--
  Stash the uncommitted changes: an optional message, and whether untracked files go into
  the stash too (off: they stay where they are).
-->
<script lang="ts">
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    /** Untracked files there are now. */
    untracked: number;
    /** The active worktree it was opened for: what it asks for runs only there. */
    worktree: string;
    close: () => void;
  };

  let { repoId, untracked, worktree, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  let message = $state("");
  let includeUntracked = $state(false);

  const untrackedNote = $derived(
    untracked === 0
      ? "There are none now."
      : `${untracked} new file${untracked === 1 ? "" : "s"} git does not track yet; without this, ${untracked === 1 ? "it stays" : "they stay"} where ${untracked === 1 ? "it is" : "they are"}.`,
  );

  function submit() {
    const text = message.trim();
    closeAndRun(close, repoId, { op: "stash_save", message: text === "" ? null : text, includeUntracked, expectWorktree: worktree });
  }
</script>

<Dialog title="Stash changes" primary="Stash" onSubmit={submit} onCancel={close}>
  <p class="dialog-text">The changes are put aside, and the files go back to the last commit.</p>
  <div class="field">
    <label class="field-label" for="stash-message-{uid}">Message (optional)</label>
    <input
      id="stash-message-{uid}"
      class="text-input"
      type="text"
      bind:value={message}
      placeholder="What these changes are"
      autocomplete="off"
      spellcheck="false"
    />
  </div>
  <label class="check-row">
    <input class="checkbox" type="checkbox" bind:checked={includeUntracked} />
    <span>
      Include untracked files
      <span class="check-note">{untrackedNote}</span>
    </span>
  </label>
</Dialog>

<style>
  .field {
    margin-top: 12px;
  }
</style>
