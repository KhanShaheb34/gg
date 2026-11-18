<!--
  Reset the current branch (or a detached HEAD) to a commit: soft, mixed or hard, each saying
  what happens to the changes. The mode picked in the menu is chosen to begin with. Hard
  makes the button red, and the core asks for confirmation before it runs (it says what
  would be lost).
-->
<script lang="ts">
  import { shortHash } from "../../format";
  import { shown } from "../../menus/common";
  import type { HeadInfo } from "../../types";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import { closeAndRun } from "./run";

  type Mode = "soft" | "mixed" | "hard";
  type Props = {
    repoId: number;
    hash: string;
    subject: string;
    /** HEAD as the menu showed it: the reset is for that branch. */
    head: HeadInfo;
    mode: Mode;
    /** The active worktree it was opened for: what it asks for runs only there. */
    worktree: string;
    close: () => void;
  };

  let { repoId, hash, subject, head, mode: initial, worktree, close }: Props = $props();

  // svelte-ignore state_referenced_locally
  let mode: Mode = $state(initial);
  const uid = Math.random().toString(36).slice(2, 8);
  const what = $derived(head.branch === null ? "HEAD" : shown(head.branch));

  const MODES: { mode: Mode; title: string; text: (what: string) => string }[] = [
    {
      mode: "soft",
      title: "Soft",
      text: (what) => `Moves ${what} only. The difference to where it was stays staged; so do your changes.`,
    },
    {
      mode: "mixed",
      title: "Mixed",
      text: (what) => `Moves ${what} and unstages everything. Every change stays in your files, unstaged.`,
    },
    {
      mode: "hard",
      title: "Hard",
      text: () => "Makes your files match this commit: uncommitted changes are discarded. Untracked files stay.",
    },
  ];
</script>

<Dialog
  title="Reset {what} to {shortHash(hash)}"
  primary="Reset"
  danger={mode === "hard"}
  onSubmit={() => closeAndRun(close, repoId, { op: "reset", hash, mode, expectHead: head, expectWorktree: worktree })}
  onCancel={close}
>
  <div class="dialog-target">
    <span class="dialog-target-label">To</span>
    <CommitRef {hash} {subject} />
  </div>
  <fieldset class="choices">
    <legend class="field-label">What happens to the changes</legend>
    {#each MODES as m (m.mode)}
      <label class="choice" class:danger={m.mode === "hard"}>
        <input
          class="radio"
          type="radio"
          name="reset-mode-{uid}"
          value={m.mode}
          bind:group={mode}
          data-autofocus={m.mode === initial ? "" : undefined}
        />
        <span>
          <span class="choice-title">{m.title}</span>
          <span class="choice-text">{m.text(what)}</span>
        </span>
      </label>
    {/each}
  </fieldset>
</Dialog>
