<!--
  A pull request in full, in the side panel's Pull request tab (a branch's open pull request)
  and on the Pull requests page (any of the user's, opened from its list); it reads and writes
  only through its PrSource, so it is one view in both places. Its state (Draft or
  Open, in the markers' colours), its number, its title in full, base ← head (cut in the
  middle), Open in browser and Copy link, and "as of 14:05" when the list it comes from may be
  out of date. Below it (read-only), what gh says of it now: where it stands for merging and
  its review decision (a status line), its checks (PrChecks) and its comments (PrComments).

  Those are asked for while it shows (the core asks gh at most once every 10 s per pull
  request): when it first shows, when the user refreshes, after the list of pull requests
  reloads, after a push of its branch (and once more a little later, when the checks it
  started are there), and every 15 s while some check runs and the window is focused and
  visible; never otherwise. The first load shows placeholders of the sections' size; a reload
  keeps what is shown; a failed one keeps it too and says so in one line, with Retry.

  The comments take what the user may write (prCompose.svelte.ts): a reply in a thread,
  resolving it, a comment on the conversation; their drafts are the source's (the side panel's
  PanelState, the page's own), so they stay through other tabs and other commits. Each sent, the details load again, the
  comments shown meanwhile.
-->
<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import { visibleText } from "../format";
  import { mergeWording, POLL_MS, reviewWording, shouldPoll } from "../prDetails";
  import { app, windowState } from "../state/app.svelte";
  import { writesFor } from "../state/prCompose.svelte";
  import { detailsOf } from "../state/prDetails.svelte";
  import type { PrSource } from "../state/prSource.svelte";
  import { clock, copyLink } from "../state/prs.svelte";
  import type { PullRequest } from "../types";
  import MiddleText from "./MiddleText.svelte";
  import PrChecks from "./PrChecks.svelte";
  import PrComments from "./PrComments.svelte";

  type Props = {
    /** Where it comes from: a repository's side panel, or the Pull requests page. */
    source: PrSource;
    pr: PullRequest;
    /** "as of 14:05" when the list it comes from may be out of date. */
    stale: string | null;
    /** A section after the checks (the Pull requests page: the files it changes). */
    more?: Snippet;
  };

  let { source, pr, stale, more }: Props = $props();
  const draft = $derived(pr.state === "draft");
  const key = $derived(source.key);
  /** As values, so only a change of them asks again (not each new snapshot they are read from). */
  const refresh = $derived(source.refresh);
  const pushedAt = $derived(source.pushed);
  /** As values: a new snapshot (or the same pull request in it, anew) changes neither. */
  const number = $derived(pr.number);
  /** This pull request's details: an answer for another is never shown here. */
  const entry = $derived(detailsOf(key, number));
  const details = $derived(entry?.data ?? null);
  const status = $derived(details ? mergeWording(details, pr.base) : null);
  const review = $derived(details ? reviewWording(details.review) : null);
  /** What the comments can write to this pull request, with the source's drafts. */
  const writes = $derived(writesFor(source.compose, source.target(number), (from) => source.showLog(from)));

  /** How often a running check is asked about again (tests may make it shorter). */
  function pollMs(): number {
    const override = (window as { __GG_PR_POLL_MS__?: number }).__GG_PR_POLL_MS__;
    return typeof override === "number" && override > 0 ? override : POLL_MS;
  }

  function load(background = false) {
    void source.load(number, background);
  }

  // Asked for when it shows (or its pull request changes), when the user refreshes, and after
  // the list reloads; not for any other change of the repository.
  $effect(() => {
    void number;
    void key;
    void refresh;
    untrack(() => load());
  });

  // After a push of its branch: at once, and once more when the checks it started are there.
  let seenPush = untrack(() => source.pushed);
  $effect(() => {
    const pushed = pushedAt;
    if (pushed === seenPush) return;
    seenPush = pushed;
    untrack(() => load());
    const later = setTimeout(() => load(true), pollMs());
    return () => clearTimeout(later);
  });

  // Every 15 s while a check runs, the tab shows it and the window is focused and visible.
  $effect(() => {
    const go = shouldPoll({
      pending: (details?.checksSummary.pending ?? 0) > 0,
      tabShown: true,
      focused: windowState.focused,
      visible: windowState.visible,
    });
    if (!go) return;
    const every = pollMs();
    // Back after a while (the window focused again): asked at once.
    if (Date.now() - untrack(() => entry?.askedAt ?? 0) >= every) untrack(() => load(true));
    const timer = setInterval(() => load(true), every);
    return () => clearInterval(timer);
  });

  /** The core's message without its lead ("Could not load the checks and comments: "), said by the line itself. */
  const reasonOf = (message: string) => message.replace(/^Could not load the checks and comments:\s*/, "");

  const openLink = (url: string) => source.openLink(url);
  const openPage = (url: string) => source.openPage(url);
</script>

<div class="pr-view" data-testid="pr-view">
  <section class="head" aria-label="Pull request #{pr.number}">
    <div class="meta">
      <span class="state" class:draft data-testid="pr-state">
        <svg width="12" height="12" viewBox="0 0 10 10" aria-hidden="true">
          <circle cx="2.4" cy="2" r="1.25" fill="none" stroke="currentColor" stroke-width="1.05" />
          <circle cx="2.4" cy="8" r="1.25" fill="none" stroke="currentColor" stroke-width="1.05" />
          <circle cx="7.6" cy="8" r="1.25" fill="none" stroke="currentColor" stroke-width="1.05" />
          <path d="M2.4 3.25v3.5M7.6 6.75V4.1c0-.9-.6-1.5-1.5-1.5H4.6" fill="none" stroke="currentColor" stroke-width="1.05" />
          <path d="M5.7 1.4 4.5 2.6l1.2 1.2" fill="none" stroke="currentColor" stroke-width="1.05" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        {draft ? "Draft" : "Open"}
      </span>
      <span class="number" data-testid="pr-number">#{pr.number}</span>
      {#if stale}
        <span class="stale" data-testid="pr-stale" title="The pull requests were last loaded then: refresh to load them again">{stale}</span>
      {/if}
    </div>
    <h2 class="title selectable" data-testid="pr-view-title">{visibleText(pr.title)}</h2>
    <p class="flow" data-testid="pr-flow" aria-label="Into {pr.base} from {pr.head}">
      <MiddleText class="ref base" text={pr.base} keep={10} />
      <span class="arrow" aria-hidden="true">←</span>
      <MiddleText class="ref" text={pr.head} keep={14} />
    </p>
    <div class="actions">
      <button class="button" type="button" data-pr-focus data-testid="pr-open" onclick={() => source.openPage(pr.url)}>Open in browser</button>
      <button class="button" type="button" data-testid="pr-copy" onclick={() => void copyLink(pr.url)}>Copy link</button>
    </div>
  </section>
  <div class="rest" data-testid="pr-details" aria-busy={entry?.loading ?? true}>
    {#if entry?.error}
      <p class="failed" role="status" data-testid="pr-details-failed">
        <span class="failed-text" title={entry.error}>{details ? "Could not refresh: " : "Could not load: "}{reasonOf(entry.error)}</span>
        <button class="retry" type="button" data-testid="pr-details-retry" disabled={entry.loading} onclick={() => load()}>Retry</button>
      </p>
    {/if}
    {#if details && status}
      <div class="status" data-testid="pr-status">
        <span class="chip {status.tone}" data-testid="pr-merge-status"><span class="chip-dot" aria-hidden="true"></span>{status.text}</span>
        {#if review}<span class="chip {review.tone}" data-testid="pr-review">{review.text}</span>{/if}
      </div>
      <PrChecks id="{key}#{number}" checks={details.checks} summary={details.checksSummary} now={Math.max(clock.now, details.loadedAtMs)} onlink={openLink} />
      {#if more}{@render more()}{/if}
      <div class="divider" aria-hidden="true"></div>
      <PrComments {details} dateFormat={app.settings.dateFormat} now={clock.now} onpage={openPage} onlink={openLink} {writes} />
    {:else if !entry?.error}
      <!-- The first load: the sections' outline, as tall as they will start. -->
      <div class="skeleton" aria-hidden="true" data-testid="pr-details-loading">
        <div class="status"><span class="bone chip-bone"></span></div>
        <div class="bone-head"><span class="bone w-40"></span><span class="bone w-90 end"></span></div>
        <div class="bone-head"><span class="bone w-60"></span></div>
        <div class="bone-card"><span class="bone w-50"></span><span class="bone w-full"></span><span class="bone w-70"></span></div>
        <div class="bone-card"><span class="bone w-50"></span><span class="bone w-full"></span><span class="bone w-40"></span></div>
      </div>
      <span class="sr-only" role="status">Loading the checks and comments…</span>
    {/if}
  </div>
</div>

<style>
  .pr-view {
    /* Its width decides how the buttons sit (below). */
    container: pr-view / inline-size;
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .head {
    display: flex;
    flex-direction: column;
    flex: none;
    gap: 6px;
    padding: 12px 12px 14px;
    border-bottom: 1px solid var(--border);
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    height: 20px;
  }

  /* The marker's colours (PrChip): open in the success tint, a draft hollow and quiet. */
  .state {
    --pr: var(--success);
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
    height: 20px;
    padding: 0 7px 0 6px;
    border: 1px solid color-mix(in srgb, var(--pr) 32%, transparent);
    border-radius: 10px;
    background: color-mix(in srgb, var(--pr) 9%, var(--bg));
    color: var(--text);
    font-size: var(--fs-xs);
    font-weight: 600;
    line-height: 18px;
  }

  .state svg {
    display: block;
    flex: none;
    color: color-mix(in srgb, var(--pr) 85%, var(--text));
  }

  .state.draft {
    border-style: dashed;
    border-color: var(--border-strong);
    background: transparent;
    color: var(--text-muted);
  }

  .state.draft svg {
    color: var(--text-faint);
  }

  .number {
    flex: none;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }

  .stale {
    min-width: 0;
    margin-left: auto;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    margin: 2px 0 0;
    color: var(--text);
    font-size: var(--fs);
    font-weight: 600;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }

  .flow {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    margin: 0;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }

  .flow :global(.ref) {
    flex: 0 1 auto;
    min-width: 0;
  }

  .flow :global(.base) {
    flex-shrink: 2;
  }

  .arrow {
    flex: none;
    color: var(--text-faint);
    font-family: var(--font-ui);
  }

  /* Side by side, each as wide as its label; in a panel narrower than 280 px (its least width
     is 240), stacked, each the row's width: by rule, whatever the font's widths. */
  .actions {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }

  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: 1 1 auto;
    min-width: 0;
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

  @container pr-view (width < 280px) {
    .actions {
      flex-direction: column;
    }

    .button {
      flex: none;
    }
  }

  .button:hover {
    background: var(--bg-hover);
  }

  .button:active {
    background: var(--bg-pressed);
  }

  .button:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .rest {
    display: flex;
    flex-direction: column;
    flex: 1 0 auto;
    min-width: 0;
  }

  /* Each section as tall as what it holds: the view scrolls, never squeezes them. */
  .rest > :global(*) {
    flex: none;
  }

  /* Where it stands for merging, and its review decision: status, not buttons. */
  .status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-height: 40px;
    padding: 9px 12px 8px;
  }

  .chip {
    --tone: var(--text-faint);
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 9px 0 8px;
    border: 1px solid color-mix(in srgb, var(--tone) 30%, transparent);
    border-radius: 11px;
    background: color-mix(in srgb, var(--tone) 8%, var(--bg));
    color: var(--text);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }

  .chip.success {
    --tone: var(--success);
  }

  .chip.warning {
    --tone: var(--warning);
  }

  .chip.danger {
    --tone: var(--danger);
  }

  .chip.neutral {
    border-color: var(--border-strong);
    background: transparent;
    color: var(--text-muted);
  }

  .chip-dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--tone);
  }

  .divider {
    flex: none;
    height: 1px;
    margin: 4px 12px 6px;
    background: var(--border);
  }

  .failed {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    margin: 8px 12px 0;
    padding: 5px 6px 5px 10px;
    border: 1px solid color-mix(in srgb, var(--danger) 30%, var(--border));
    border-radius: var(--radius);
    background: var(--danger-tint);
    color: var(--text);
    font-size: var(--fs-xs);
  }

  .failed-text {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .retry {
    flex: none;
    height: 22px;
    padding: 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-xs);
    font-weight: 550;
  }

  .retry:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .retry:disabled {
    color: var(--text-faint);
  }

  /* Placeholders of the sections' size while they first load: still, quiet. */
  .skeleton {
    display: flex;
    flex-direction: column;
  }

  .bone {
    display: block;
    height: 9px;
    border-radius: 5px;
    background: var(--bg-hover);
  }

  .chip-bone {
    width: 104px;
    height: 22px;
    border-radius: 11px;
  }

  .bone-head {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    padding: 0 12px 0 28px;
  }

  .bone-card {
    display: flex;
    flex-direction: column;
    gap: 9px;
    margin: 4px 10px 4px;
    padding: 11px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .w-40 {
    width: 40px;
  }

  .w-50 {
    width: 50%;
  }

  .w-60 {
    width: 60px;
  }

  .w-70 {
    width: 70%;
  }

  .w-90 {
    width: 90px;
  }

  .w-full {
    width: 100%;
  }

  .end {
    margin-left: auto;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
