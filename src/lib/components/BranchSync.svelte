<!--
  Beside the branch picker: how the current branch stands with its upstream, as its label in
  the graph shows it (↑ ahead, ↓ behind), each count a button. ↓ pulls the current branch as its
  menu's Pull does; ↑ pushes it to its upstream as Push does (to an upstream of another name,
  through the Push dialog, which names where it goes). Nothing shows when the branch is level
  with its upstream, has none, or HEAD is detached. While an action runs in the repository
  both wait (the counts are about to change). See branchSync.ts.
-->
<script lang="ts">
  import { branchSync } from "../branchSync";
  import { runOp } from "../state/ops.svelte";
  import { showDialog } from "../state/overlays.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import PushDialog from "./dialogs/PushDialog.svelte";

  let { view }: { view: RepoView } = $props();

  const sync = $derived(branchSync(view.snapshot));
  /**
   * A click sent, until its outcome: the core's snapshot shows the operation running a moment
   * later, and a second click meanwhile must not send it again.
   */
  let sending = $state<"pull" | "push" | null>(null);

  const tip = (title: string, ...more: (string | null | undefined)[]) => [title, ...more.filter(Boolean)].join("\n");
  const count = (n: number) => (n > 999 ? "999+" : String(n));

  async function send(kind: "pull" | "push") {
    const s = sync;
    const part = s && s[kind];
    if (!s || !part || part.disabledReason || sending !== null) return;
    if (part.request === null) {
      const snap = view.snapshot;
      showDialog(PushDialog, { repoId: snap.repoId, branch: s.branch, upstream: s.upstream, remotes: snap.remotes });
      return;
    }
    sending = kind;
    try {
      await runOp(view.repoId, part.request);
    } catch {
      // Reported by runOp.
    } finally {
      sending = null;
    }
  }
</script>

{#if sync}
  <div class="sync" role="group" aria-label="Sync {sync.branch} with {sync.upstream}" data-testid="branch-sync">
    {#if sync.push}
      {@const p = sync.push}
      {@const off = !!p.disabledReason || sending !== null}
      <button
        class="part"
        data-testid="sync-push"
        aria-label={p.title}
        aria-disabled={off}
        aria-busy={sending === "push"}
        title={tip(p.title, p.note, p.disabledReason)}
        onclick={() => void send("push")}
      >
        <!-- An arrow up: out to the remote. -->
        <svg width="10" height="12" viewBox="0 0 10 12" aria-hidden="true">
          <path d="M5 10.6V1.9M1.7 5.1 5 1.8l3.3 3.3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        <span class="count">{count(p.count)}</span>
      </button>
    {/if}
    {#if sync.pull}
      {@const p = sync.pull}
      {@const off = !!p.disabledReason || sending !== null}
      <button
        class="part"
        data-testid="sync-pull"
        aria-label={p.title}
        aria-disabled={off}
        aria-busy={sending === "pull"}
        title={tip(p.title, p.disabledReason)}
        onclick={() => void send("pull")}
      >
        <!-- An arrow down: in from the remote. -->
        <svg width="10" height="12" viewBox="0 0 10 12" aria-hidden="true">
          <path d="M5 1.4v8.7M1.7 6.9 5 10.2l3.3-3.3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        <span class="count">{count(p.count)}</span>
      </button>
    {/if}
  </div>
{/if}

<style>
  /* A quiet accent pill, as the running operation's: something to do. It keeps its width (the
     branch's name and the folder beside it give way first). */
  .sync {
    display: inline-flex;
    align-items: stretch;
    flex: none;
    height: 22px;
    margin: 0 6px 0 -6px;
    border-radius: 11px;
    background: color-mix(in srgb, var(--accent) 11%, transparent);
    color: var(--accent);
    font-size: var(--fs-sm);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    white-space: nowrap;
    animation: sync-in 140ms var(--ease);
  }

  .part {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 0 8px 0 7px;
    border: 0;
    border-radius: 11px;
    background: transparent;
    color: inherit;
    font: inherit;
    line-height: 1;
    transition:
      background-color 80ms linear,
      color 80ms linear;
  }

  /* Both counts: each half rounded on its outer side only, a hairline between them. */
  .part + .part {
    position: relative;
    padding-left: 6px;
  }

  .part:has(+ .part) {
    padding-right: 6px;
    border-radius: 11px 0 0 11px;
  }

  .part + .part {
    border-radius: 0 11px 11px 0;
  }

  .part + .part::before {
    content: "";
    position: absolute;
    left: 0;
    top: 5px;
    bottom: 5px;
    width: 1px;
    background: color-mix(in srgb, var(--accent) 30%, transparent);
  }

  .part:hover {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }

  .part:active {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
  }

  .part[aria-disabled="true"] {
    background: transparent;
    color: color-mix(in srgb, var(--accent) 55%, transparent);
  }

  .part[aria-busy="true"] {
    cursor: progress;
  }

  .part svg {
    display: block;
    flex: none;
  }

  .count {
    letter-spacing: -0.01em;
  }

  @keyframes sync-in {
    from {
      opacity: 0;
    }
  }
</style>
