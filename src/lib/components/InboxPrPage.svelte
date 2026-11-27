<!--
  One pull request of the Pull requests page, on a page of its own: what the side panel's Pull
  request tab shows of it (PullRequestView: its state, branches, checks, comments and reviews,
  with reply and resolve, Open in browser, Copy link), read and written by its repository's
  name, never through a repository tab (that repository may be on another branch). Above it,
  Back (Escape) and what can be done with its repository: when it is open in gg, Go to
  repository and checking out its branch there (gg's own checkout, its safety included, only
  on a click); when it is not, Clone and open.
-->
<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { inboxStale } from "../inbox";
  import { app } from "../state/app.svelte";
  import {
    asPullRequest,
    backToList,
    checkOutInRepo,
    checkoutOf,
    findOpenRepo,
    goToRepo,
    inbox,
    inboxPrSource,
  } from "../state/inbox.svelte";
  import { showDialog } from "../state/overlays.svelte";
  import { clock } from "../state/prs.svelte";
  import { findRepoView } from "../state/repo.svelte";
  import type { InboxPr, InboxRepoMatch } from "../types";
  import CloneDialog from "./dialogs/CloneDialog.svelte";
  import MiddleText from "./MiddleText.svelte";
  import PullRequestView from "./PullRequestView.svelte";

  type Props = { pr: InboxPr };
  let { pr }: Props = $props();

  const source = $derived(inboxPrSource(pr));
  const shown = $derived(asPullRequest(pr));
  const stale = $derived(inboxStale(inbox.data, clock.now, app.settings.dateFormat));

  /** The open repository that has its repository as a remote: undefined while asked. */
  let match = $state<InboxRepoMatch | null | undefined>(undefined);
  const repoName = $derived(match ? (app.tabs.find((t) => t.repoId === match!.repoId)?.name ?? null) : null);
  const plan = $derived(match ? checkoutOf(pr, match) : null);
  const planLabel = $derived.by(() => {
    if (!plan) return null;
    switch (plan.kind) {
      case "current":
        return "Checked out";
      case "worktree":
        return "Go to its worktree";
      case "fetch":
        return "Fetch and check out";
      default:
        return "Check out";
    }
  });

  // Asked when the page shows, and again when a repository opens or closes.
  $effect(() => {
    const key = pr.key;
    void app.tabs.length;
    untrack(() => {
      void findOpenRepo(pr).then((m) => {
        if (pr.key === key) match = m;
      });
    });
  });

  let back: HTMLButtonElement | undefined = $state();
  onMount(() => {
    void tick().then(() => back?.focus({ preventScroll: true }));
  });

  function onKeydown(e: KeyboardEvent) {
    if (e.key !== "Escape" || e.defaultPrevented) return;
    // A field's own Escape (a reply being written) is its own.
    const target = e.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable]")) return;
    e.preventDefault();
    backToList();
  }

  function cloneAndOpen() {
    showDialog(CloneDialog, { host: pr.host, owner: pr.owner, name: pr.name });
  }

</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="pr-page" data-testid="inbox-pr-page" onkeydown={onKeydown}>
  <header class="bar">
    <div class="bar-inner">
    <button class="back" type="button" bind:this={back} onclick={backToList} title="Back to the pull requests (Esc)" data-testid="inbox-back">
      <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
        <path d="M8.6 3.2 4.8 7l3.8 3.8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
      <span>Pull requests</span>
    </button>
    <span class="repo" data-testid="inbox-pr-repo">
      <MiddleText text="{pr.owner}/{pr.name}" keep={10} />
    </span>
    <div class="actions" data-testid="inbox-pr-actions">
      {#if match}
        <button class="button" type="button" data-testid="inbox-go-to-repo" title="Show {repoName ?? 'the repository'}'s tab" onclick={() => goToRepo(match!.repoId)}>
          Go to repository
        </button>
        {#if plan && planLabel}
          <button
            class="button"
            type="button"
            data-testid="inbox-checkout"
            data-plan={plan.kind}
            disabled={plan.kind === "current" || (findRepoView(match.repoId)?.snapshot.op ?? null) !== null}
            title={plan.kind === "current"
              ? `${plan.branch} is checked out in ${repoName ?? "the repository"}`
              : plan.kind === "fetch"
                ? `Fetch ${plan.remoteBranch} into ${plan.branch} and check it out in ${repoName ?? "the repository"}`
                : `Check out ${plan.branch} in ${repoName ?? "the repository"}`}
            onclick={() => void checkOutInRepo(match!, plan!)}
          >
            {planLabel}
          </button>
        {/if}
      {:else if match === null}
        <button class="button" type="button" data-testid="inbox-clone" title="Clone {pr.owner}/{pr.name} into a folder and open it in gg" onclick={cloneAndOpen}>
          Clone and open…
        </button>
      {/if}
    </div>
    </div>
  </header>
  <div class="scroll">
    <div class="column">
      {#key pr.key}
        <PullRequestView {source} pr={shown} {stale} />
      {/key}
    </div>
  </div>
</div>

<style>
  .pr-page {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .bar {
    flex: none;
    border-bottom: 1px solid var(--border);
  }

  /* As wide as the pull request under it, so their edges line up. */
  .bar-inner {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    max-width: 760px;
    min-height: 40px;
    margin: 0 auto;
    padding: 6px 16px 6px 12px;
  }

  .back {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    height: 28px;
    padding: 0 10px 0 6px;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    font-weight: 550;
    transition: background-color 80ms linear;
  }

  .back:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .back:active {
    background: var(--bg-pressed);
  }

  .back svg {
    display: block;
  }

  .repo {
    display: flex;
    flex: 1 1 120px;
    min-width: 0;
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
  }

  .actions {
    display: flex;
    gap: 6px;
    flex: none;
    margin-left: auto;
  }

  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
    white-space: nowrap;
    transition:
      background-color 80ms linear,
      border-color 80ms linear;
  }

  .button:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .button:active:not(:disabled) {
    background: var(--bg-pressed);
  }

  .button:disabled {
    color: var(--text-faint);
  }

  .button:focus-visible,
  .back:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .scroll {
    display: flex;
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }

  /* As wide as reads well; the view inside lays itself out by this width. */
  .column {
    display: flex;
    flex-direction: column;
    width: 100%;
    max-width: 760px;
    margin: 0 auto;
    padding: 4px 8px 24px;
  }

  .column :global(.pr-view) {
    overflow: visible;
  }

  /* On a page, Open in browser and Copy link are as wide as their labels. */
  .column :global(.pr-view .actions .button) {
    flex: none;
  }
</style>
