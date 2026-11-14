<!--
  Pulling a branch that has diverged from its upstream, when git's config does not say how to
  reconcile them (the core's `choose_pull_mode` hint): merge or rebase, merge chosen to
  begin with; the pull is then sent again that way.
-->
<script lang="ts">
  import { visibleText } from "../../format";
  import { shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    branch: string;
    /** Its upstream ("origin/x"), when known. */
    upstream: string | null;
    /** What the core said. */
    message: string;
    close: () => void;
  };

  let { repoId, branch, upstream, message, close }: Props = $props();

  type Mode = "merge" | "rebase";
  let mode: Mode = $state("merge");
  const uid = Math.random().toString(36).slice(2, 8);
  // svelte-ignore state_referenced_locally
  const from = upstream === null ? "its upstream" : shown(upstream);
  // svelte-ignore state_referenced_locally
  const into = shown(branch);
  const MODES: { mode: Mode; title: string; text: string }[] = [
    { mode: "merge", title: "Merge", text: `A merge commit joins ${from} into ${into}.` },
    {
      mode: "rebase",
      title: "Rebase",
      text: `The commits of ${into} are made again on top of ${from}, so they get new hashes.`,
    },
  ];
</script>

<Dialog
  title={upstream === null ? `Pull into ${into}` : `Pull ${from} into ${into}`}
  primary="Pull"
  onSubmit={() => closeAndRun(close, repoId, { op: "pull", branch, mode })}
  onCancel={close}
>
  <p class="dialog-text">{visibleText(message)}</p>
  <fieldset class="choices">
    <legend class="field-label">How to pull</legend>
    {#each MODES as m (m.mode)}
      <label class="choice">
        <input
          class="radio"
          type="radio"
          name="pull-mode-{uid}"
          value={m.mode}
          bind:group={mode}
          data-autofocus={m.mode === "merge" ? "" : undefined}
        />
        <span>
          <span class="choice-title">{m.title}</span>
          <span class="choice-text">{m.text}</span>
        </span>
      </label>
    {/each}
  </fieldset>
</Dialog>
