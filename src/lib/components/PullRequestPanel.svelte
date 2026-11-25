<!--
  The side panel's Pull request tab: about one local branch, the active worktree's (as it
  changes) unless a branch's menu asked for another; then a line at the top names that one,
  with a button back to the current branch. It shows the branch's open pull request when it
  has one (the list's, one just made here, or one gh knows of); else the form to make one;
  else why none can be made there, plainly, with what to do: HEAD detached, no remote, a
  remote not on GitHub, the default branch, gh missing or signed out (where the branch can
  still be pushed and GitHub's compare page opened). While the list of pull requests loads
  for the first time, the form shows, waiting on the core's suggestion.

  What the core suggests (`pr_suggest`, which also asks gh whether the branch has a pull
  request) is asked when the form or the compare page would show, and kept while the branch,
  where it was pushed and the list stay as they were (pullRequest.svelte.ts).
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import { api } from "../ipc";
  import { PR_REASONS } from "../menus/common";
  import { app } from "../state/app.svelte";
  import { clock, staleSince } from "../state/prs.svelte";
  import {
    defaultBranch,
    keepSuggestion,
    suggestionTicket,
    keptSuggestion,
    knownPr,
    lastSuggestion,
    prBackToCurrent,
    suggestionStamp,
    unavailableHere,
  } from "../state/pullRequest.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import { panelState } from "../state/worktrees.svelte";
  import type { PrState, PrSuggestion, PullRequest } from "../types";
  import MiddleText from "./MiddleText.svelte";
  import PullRequestForm from "./PullRequestForm.svelte";
  import PullRequestView from "./PullRequestView.svelte";

  let { view, branch }: { view: RepoView; branch: string | null } = $props();

  const snap = $derived(view.snapshot);
  const panel = $derived(panelState(view.repoId));
  /** The tab is about another branch than the active worktree's. */
  const pinned = $derived(branch !== null && branch !== snap.head.branch);

  /**
   * The pull requests' state as last settled: while a load the tab asked for runs, what was
   * known stays shown (no form flashing up and away).
   */
  let settled = $state<PrState>(untrack(() => view.snapshot.prs));
  $effect(() => {
    if (snap.prs.status !== "loading" || settled.status === "unknown") settled = snap.prs;
  });

  // --- What the core suggests for the branch ------------------------------------------------

  let suggestion = $state<PrSuggestion | null>(null);
  let suggestFailed = $state<string | null>(null);
  /** The branch `suggestion` is for. */
  let suggestionFor: string | null = $state(null);
  let asked = 0;

  const known = $derived(branch === null ? null : knownPr(snap, branch));
  const unavailable = $derived(unavailableHere(settled, snap.remotes));
  const isDefault = $derived(branch !== null && branch === defaultBranch(snap, suggestion?.baseRemote ?? undefined));

  type Shown =
    | { kind: "detached" }
    | { kind: "unavailable"; why: "no_remote" | "not_github" }
    | { kind: "default"; branch: string }
    | { kind: "pr"; pr: PullRequest }
    | { kind: "form"; branch: string; compare: "no_gh" | "signed_out" | null };

  const shown: Shown = $derived.by(() => {
    if (branch === null) return { kind: "detached" };
    if (unavailable === "no_remote") return { kind: "unavailable", why: "no_remote" };
    const pr = known ?? (suggestionFor === branch ? (suggestion?.existing ?? null) : null);
    if (pr) return { kind: "pr", pr };
    if (unavailable === "not_github") return { kind: "unavailable", why: "not_github" };
    if (isDefault) return { kind: "default", branch };
    return { kind: "form", branch, compare: unavailable === "no_gh" || unavailable === "signed_out" ? unavailable : null };
  });

  /** Whether the suggestion is wanted: the form or the compare page would show, or gh may know of a pull request. */
  const wanted = $derived(branch !== null && known === null && unavailable !== "no_remote" && unavailable !== "not_github" && !isDefault);
  const stamp = $derived(branch === null ? "" : suggestionStamp(snap, branch));

  /**
   * Asks the core what to suggest for the tab's branch (or takes what it suggested before, while
   * that holds). Every call is a new request: an answer is taken only while it is the latest
   * one and the tab is still about its branch, so an answer for another branch that comes late
   * never fills this one's form.
   */
  async function suggest(force = false) {
    const n = ++asked;
    const b = branch;
    if (b === null) return;
    const root = snap.rootPath;
    const at = stamp;
    if (!force) {
      const kept = keptSuggestion(root, b, at);
      if (kept) {
        suggestionFor = b;
        suggestion = kept;
        suggestFailed = null;
        return;
      }
    }
    // Shown meanwhile: what was suggested for it before, if anything.
    if (suggestionFor !== b) {
      suggestion = lastSuggestion(root, b);
      suggestionFor = b;
    }
    suggestFailed = null;
    const ticket = suggestionTicket();
    try {
      const s = await api.prSuggest(view.repoId, b);
      // Kept for its branch whenever it comes; shown only if still asked for, for this branch.
      keepSuggestion(root, b, at, s, ticket);
      if (n !== asked || b !== branch) return;
      suggestion = s;
    } catch (e) {
      if (n !== asked || b !== branch) return;
      suggestFailed = e instanceof Error ? e.message : String(e);
    }
  }

  // Asked again when what it depends on changes, and when the user refreshes.
  let seenRefresh = untrack(() => panelState(view.repoId).prRefresh);
  $effect(() => {
    // The branch itself too: two branches at the same commit, neither pushed, share a stamp.
    void branch;
    void stamp;
    const refreshed = panel.prRefresh;
    if (!wanted) {
      // Nothing to ask for this branch: an answer still out for another is not taken.
      asked++;
      return;
    }
    const force = refreshed !== seenRefresh;
    seenRefresh = refreshed;
    // Only what is read above is followed: not what asking reads and writes.
    untrack(() => void suggest(force));
  });

  // Not known yet whether gh can be asked, or it could not be (not installed, signed out: the
  // user may have fixed that a moment ago): a refresh the user would have asked for loads the
  // pull requests, and asks gh again at once. At most once every few seconds per repository.
  $effect(() => {
    const status = snap.prs.status;
    const gone = unavailable === "no_gh" || unavailable === "signed_out";
    if (!wanted || !(status === "unknown" || gone)) return;
    const now = Date.now();
    if (now - (lastAsked.get(view.repoId) ?? 0) < ASK_AGAIN_MS) return;
    lastAsked.set(view.repoId, now);
    api.refresh(view.repoId, true).then(
      (s) => view.applySnapshot(s),
      () => {},
    );
  });

  // Asked for (a menu, the shortcut, back to the current branch): the focus goes into the tab,
  // to the title when there is a form, once per request. Shown again it leaves the focus.
  let root: HTMLElement | undefined = $state();
  $effect(() => {
    const request = panel.prFocus;
    if (request === panel.prFocusDone) return;
    panel.prFocusDone = request;
    void tick().then(() => {
      const target = root?.querySelector<HTMLElement>("[data-pr-focus]") ?? document.getElementById("side-tab-pr");
      target?.focus();
    });
  });

  // What had the focus here going with what the tab shows (the form giving way to the pull
  // request gh knows of), the focus is not lost with it: it goes to what the tab shows now.
  let focusInside = false;
  function onFocusOut(e: FocusEvent) {
    const to = e.relatedTarget;
    if (to instanceof Node) focusInside = !!root?.contains(to);
  }
  $effect(() => {
    void shown.kind;
    void tick().then(() => {
      if (!focusInside || (document.activeElement !== null && document.activeElement !== document.body)) return;
      root?.querySelector<HTMLElement>("[data-pr-focus]")?.focus();
    });
  });

  const stale = $derived(staleSince(snap.prs, clock.now, app.settings.dateFormat));

  const REASONS = {
    no_remote: "This repository has no remote. Add one on GitHub to open pull requests from here.",
    not_github: `${PR_REASONS.not_github}: pull requests are made on GitHub only.`,
  };
</script>

<script module lang="ts">
  /** When the tab last asked for the pull requests to be loaded, by repository. */
  const lastAsked = new Map<number, number>();
  const ASK_AGAIN_MS = 10_000;
</script>

<div class="pr-panel" data-testid="pr-panel" bind:this={root} onfocusin={() => (focusInside = true)} onfocusout={onFocusOut}>
  {#if pinned && branch !== null}
    <!-- Which branch this is (the active one is named at the top of the window), and the way
         back to the current branch's. -->
    <div class="for" data-testid="pr-branch">
      <svg class="glyph" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
        <circle cx="4" cy="3" r="1.5" fill="none" stroke="currentColor" stroke-width="1.15" />
        <circle cx="4" cy="11" r="1.5" fill="none" stroke="currentColor" stroke-width="1.15" />
        <circle cx="10" cy="5" r="1.5" fill="none" stroke="currentColor" stroke-width="1.15" />
        <path d="M4 4.5v5M10 6.5c0 2-2 2.4-4.6 3.6" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" />
      </svg>
      <MiddleText class="for-name" text={branch} keep={12} />
      <button
        class="icon-btn small"
        type="button"
        aria-label="Back to the current branch's pull request"
        title="Back to the current branch's pull request{snap.head.branch ? ` (${snap.head.branch})` : ''}"
        data-testid="pr-branch-return"
        data-pr-focus={shown.kind === "form" || shown.kind === "pr" ? undefined : true}
        onclick={() => prBackToCurrent(view)}
      >
        <!-- An arrow turning back. -->
        <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
          <path
            d="M5.2 3.4 2.6 6l2.6 2.6M2.9 6h5.4a2.9 2.9 0 0 1 0 5.8H6.6"
            fill="none"
            stroke="currentColor"
            stroke-width="1.35"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </button>
    </div>
  {/if}

  {#if shown.kind === "pr"}
    <PullRequestView repoId={view.repoId} pr={shown.pr} {stale} />
  {:else if shown.kind === "form"}
    {#key shown.branch}
      <PullRequestForm
        {view}
        branch={shown.branch}
        suggestion={suggestionFor === shown.branch ? suggestion : null}
        suggestFailed={suggestionFor === shown.branch ? suggestFailed : null}
        compare={shown.compare}
        onRetry={() => void suggest(true)}
        onPushPlan={() => void suggest(true)}
      />
    {/key}
  {:else}
    <div class="message" data-testid="pr-none" data-why={shown.kind === "unavailable" ? shown.why : shown.kind}>
      <div class="box">
        {#if shown.kind === "detached"}
          <p class="box-lead">HEAD is detached: there is no branch to open a pull request from.</p>
          <p>Check out a branch, or choose Create pull request… from a branch’s menu.</p>
        {:else if shown.kind === "default"}
          <p class="box-lead">{shown.branch} is the repository’s default branch: there is nothing to open a pull request into.</p>
          <p>Make a branch for the change, or choose Create pull request… from another branch’s menu.</p>
        {:else}
          <p class="box-lead">{REASONS[shown.why]}</p>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .pr-panel {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  /* The branch shown, when it is not the current one: a line under the panel's tabs, as the
     commit panel names another worktree. */
  .for {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
    height: 30px;
    padding: 0 4px 0 12px;
    border-bottom: 1px solid var(--border);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
  }

  .for .glyph {
    flex: none;
    color: var(--text-faint);
  }

  .for :global(.for-name) {
    flex: 1 1 auto;
    min-width: 0;
  }

  .for .icon-btn {
    flex: none;
    width: 22px;
    height: 22px;
    border-radius: 5px;
  }

  .message {
    flex: none;
    padding: 10px 12px 2px;
  }

  /* As the commit panel's box for another worktree. */
  .box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 9px 11px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .box-lead {
    color: var(--text);
  }
</style>
