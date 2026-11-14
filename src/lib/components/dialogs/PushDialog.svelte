<!--
  Push a local branch: to which remote (its upstream's to begin with, else origin, else the
  first), where on it that goes, said plainly ("origin/feature": the upstream when it is on
  that remote, else the branch of the same name), whether that becomes its upstream, and
  force (with lease), off to begin with. A branch without an upstream always gets this one
  (the core sets it), so the choice only matters when pushing to another remote than the
  upstream's. Force makes the button red; the core then asks to confirm, saying what the
  remote would lose.
-->
<script lang="ts">
  import { defaultRemote, remoteOf, shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    branch: string;
    /** Its upstream ("origin/x"), or null. */
    upstream: string | null;
    remotes: string[];
    close: () => void;
  };

  let { repoId, branch, upstream, remotes, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  // svelte-ignore state_referenced_locally
  const upstreamRemote = upstream === null ? null : remoteOf(upstream, remotes);
  // svelte-ignore state_referenced_locally
  let remote = $state(defaultRemote(remotes, upstreamRemote));
  /** Asked for when pushing to another remote than the upstream's. */
  let otherUpstream = $state(false);
  let force = $state(false);

  const toUpstream = $derived(upstreamRemote !== null && remote === upstreamRemote);
  const target = $derived(toUpstream ? upstream! : `${remote}/${branch}`);
  /** The upstream is set by the push whatever is ticked: none yet, or it is the target. */
  const fixed = $derived(upstream === null || toUpstream);
  const setUpstream = $derived(upstream === null || (!toUpstream && otherUpstream));
  const upstreamNote = $derived(
    upstream === null
      ? "It has none yet, so this becomes it."
      : toUpstream
        ? "It is already."
        : `Instead of ${shown(upstream)}.`,
  );

  function submit() {
    closeAndRun(close, repoId, { op: "push", branch, remote, setUpstream, force, lease: null });
  }
</script>

<Dialog
  title="Push {shown(branch)}"
  primary={force ? "Force push" : "Push"}
  danger={force}
  valid={remote !== ""}
  onSubmit={submit}
  onCancel={close}
>
  <div class="field first">
    <label class="field-label" for="push-remote-{uid}">Remote</label>
    <div class="select-wrap">
      <select id="push-remote-{uid}" class="select" bind:value={remote}>
        {#each remotes as r (r)}
          <option value={r}>{shown(r)}</option>
        {/each}
      </select>
    </div>
  </div>
  <div class="dialog-target to">
    <span class="dialog-target-label">To</span>
    <span class="dialog-ref">{shown(target)}</span>
  </div>
  <label class="check-row">
    <input
      class="checkbox"
      type="checkbox"
      checked={fixed || otherUpstream}
      disabled={fixed}
      onchange={(e) => (otherUpstream = e.currentTarget.checked)}
    />
    <span>
      Set as its upstream
      <span class="check-note">{upstreamNote}</span>
    </span>
  </label>
  <label class="check-row danger">
    <input class="checkbox" type="checkbox" bind:checked={force} />
    <span>
      Force push (with lease)
      <span class="check-note">
        Replaces the remote branch with this one, if it is still where it was last fetched. You are asked to confirm.
      </span>
    </span>
  </label>
</Dialog>

<style>
  .first {
    margin-top: 4px;
  }

  .to {
    margin-top: 12px;
  }

  .check-row:has(.checkbox:disabled) .check-note {
    opacity: 0.85;
  }
</style>
