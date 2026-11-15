<!--
  Push a local branch: to which remote (its upstream's to begin with, else origin, else the
  only one; among several without origin, none until one is chosen: R29), and where on it
  that goes, said plainly (R38): the branch of the same name ("origin/feature"). When the
  branch's upstream is on that remote under another name, the user can pick it instead, and
  the destination line names it; the same name stays chosen to begin with. A branch without
  an upstream gets the branch pushed to as its upstream (the core sets it); an existing
  upstream is never changed. Force (with lease), off to begin with, makes the button red;
  the core then asks to confirm, saying what the remote would lose.
-->
<script lang="ts">
  import { defaultRemote, remoteOf, shown } from "../../menus/common";
  import Dialog from "../Dialog.svelte";
  import RemoteSelect from "./RemoteSelect.svelte";
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

  // svelte-ignore state_referenced_locally
  const upstreamRemote = upstream === null ? null : remoteOf(upstream, remotes);
  /** The upstream's branch on its remote ("main" of "origin/main"). */
  // svelte-ignore state_referenced_locally
  const upstreamBranch = upstream !== null && upstreamRemote !== null ? upstream.slice(upstreamRemote.length + 1) : null;
  // svelte-ignore state_referenced_locally
  let remote = $state(defaultRemote(remotes, upstreamRemote));
  /** Push to the upstream of another name rather than the branch of the same name. */
  let toUpstream = $state(false);
  let force = $state(false);
  const uid = Math.random().toString(36).slice(2, 8);

  /** The upstream is on the chosen remote under another name: the user may pick it. */
  const otherName = $derived(upstreamRemote !== null && remote === upstreamRemote && upstreamBranch !== branch);
  const to = $derived(otherName && toUpstream ? upstreamBranch : null);
  const target = $derived(`${remote}/${to ?? branch}`);
  const upstreamNote = $derived(
    upstream === null ? `${shown(target)} becomes its upstream: it has none yet.` : `Its upstream stays ${shown(upstream)}.`,
  );

  function submit() {
    closeAndRun(close, repoId, { op: "push", branch, remote, to, force });
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
  <div class="first">
    <RemoteSelect {remotes} bind:value={remote} />
  </div>
  {#if remote !== "" && otherName}
    <fieldset class="choices to">
      <legend class="field-label">To</legend>
      <label class="choice">
        <input class="radio" type="radio" name="push-to-{uid}" value={false} bind:group={toUpstream} />
        <span>
          <span class="choice-title dialog-ref">{shown(`${remote}/${branch}`)}</span>
          <span class="choice-text">The branch of the same name.</span>
        </span>
      </label>
      <label class="choice">
        <input class="radio" type="radio" name="push-to-{uid}" value={true} bind:group={toUpstream} />
        <span>
          <span class="choice-title dialog-ref">{shown(upstream!)}</span>
          <span class="choice-text">Its upstream, which has another name.</span>
        </span>
      </label>
    </fieldset>
  {:else if remote !== ""}
    <div class="dialog-target to">
      <span class="dialog-target-label">To</span>
      <span class="dialog-ref" data-testid="push-target">{shown(target)}</span>
    </div>
  {/if}
  {#if remote !== ""}
    <p class="dialog-text muted note" data-testid="upstream-note">{upstreamNote}</p>
  {/if}
  <label class="check-row danger">
    <input class="checkbox" type="checkbox" bind:checked={force} />
    <span>
      Force push (with lease)
      <span class="check-note">
        Replaces {shown(target)} with this branch, if it is still where it was last fetched. You are asked to confirm.
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

  .note {
    margin-top: 8px;
  }
</style>
