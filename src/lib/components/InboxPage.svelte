<!--
  The Pull requests page across repositories, in place of a repository (its button in the tab
  bar, or ⌘⇧I): the user's open pull requests, those waiting for their review, and those they
  are mentioned in or assigned to, from every GitHub host gg knows of, through gh. What gg kept
  shows at once; while gh is asked again the Refresh button turns, nothing else waits. Without
  a list it says why in one box (gh missing or signed out, the rate limit, no connection, empty);
  with one, a host that failed is a quiet line above it, and the list is "as of" when it loaded.
  A pull request opens on its own page (InboxPrPage). The header groups the list by role,
  repository or status (kept in gg's config). Each part spans the page's width, so the list's
  scroll bar is at the window's edge; what is in them is centred.
-->
<script lang="ts">
  import { tick } from "svelte";
  import { hostNotes, inboxStale, layoutOf, pageState } from "../inbox";
  import { isMac } from "../platform";
  import { app } from "../state/app.svelte";
  import { inbox, refreshInbox, setGroupBy, setPinned, showInboxPr, toggle, unfoldedNow } from "../state/inbox.svelte";
  import Segmented from "./Segmented.svelte";
  import { clock } from "../state/prs.svelte";
  import { openWebPage } from "../state/editor";
  import type { InboxPr } from "../types";
  import InboxList from "./InboxList.svelte";
  import InboxPrPage from "./InboxPrPage.svelte";

  const REFRESH_KEYS = isMac() ? "⌘R" : "Ctrl+R";
  const now = $derived(pageState(inbox.data, inbox.loading));
  const notes = $derived(hostNotes(inbox.data));
  const stale = $derived(inboxStale(inbox.data, clock.now, app.settings.dateFormat));
  /** Who the list is for: the account on each host that answered. */
  const accounts = $derived(
    (inbox.data?.hosts ?? [])
      .filter((h) => h.login)
      .map((h) => (h.host === "github.com" ? `@${h.login}` : `@${h.login} on ${h.host}`))
      .join(", "),
  );
  const partial = $derived((inbox.data?.hosts ?? []).some((h) => h.partial));
  /** The list's rows: as grouped, folded and pinned now (old by the minute clock). */
  const layout = $derived(
    layoutOf(inbox.data?.prs ?? [], inbox.groupBy, clock.now, unfoldedNow(), inbox.data?.pins ?? []),
  );

  let page: HTMLElement | undefined = $state();

  function open(pr: InboxPr) {
    showInboxPr(pr);
  }

  // Back on the list (from a pull request's page): the list takes the focus again.
  let wasPr = false;
  $effect(() => {
    const onPr = inbox.pr !== null;
    if (wasPr && !onPr) void tick().then(() => page?.querySelector<HTMLElement>("[data-inbox-focus]")?.focus({ preventScroll: true }));
    wasPr = onPr;
  });

  const WHAT: Record<string, string> = {
    no_gh: "The GitHub CLI (gh) was not found",
    signed_out: "gh is not signed in",
    rate_limited: "GitHub’s rate limit was reached",
    offline: "GitHub could not be reached",
    failed: "The pull requests could not be loaded",
  };
</script>

<section class="inbox" bind:this={page} data-testid="inbox" aria-labelledby="inbox-title">
  {#if inbox.pr}
    <InboxPrPage pr={inbox.pr} />
  {:else}
    <header class="head">
      <h1 id="inbox-title">Pull requests</h1>
      {#if accounts}<span class="who" data-testid="inbox-accounts">{accounts}</span>{/if}
      <span class="spacer"></span>
      <Segmented
        label="Group by"
        testid="inbox-group-by"
        value={inbox.groupBy}
        options={[
          { value: "role", label: "Role", title: "Yours, waiting for your review, mentioned or assigned" },
          { value: "repository", label: "Repository", title: "By repository, newest first" },
          { value: "status", label: "Status", title: "What needs attention first: failing checks, changes requested, review required…" },
        ]}
        onchange={setGroupBy}
      />
      {#if stale}<span class="stale" data-testid="inbox-stale" title="When the list was last loaded: refresh to load it again">{stale}</span>{/if}
      <button
        class="icon-btn"
        aria-label="Refresh the pull requests"
        title="Refresh ({REFRESH_KEYS})"
        aria-busy={inbox.loading}
        data-testid="inbox-refresh"
        onclick={() => void refreshInbox("user")}
      >
        <svg class:spinning={inbox.loading} width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
          <path d="M13 8a5 5 0 1 1-1.46-3.54M13 2.8v2.9h-2.9" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
      </button>
    </header>

    {#each notes as h (h.host)}
      <p class="note" role="status" data-testid="inbox-host-note" data-problem={h.problem}>
        <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="8" cy="8" r="6.2" fill="none" stroke="currentColor" stroke-width="1.4" />
          <path d="M8 4.6v4.2M8 11.1v.3" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
        </svg>
        <span class="note-text"><strong>{h.host}:</strong> {h.message}</span>
      </p>
    {/each}
    {#if inbox.error}
      <p class="note" role="status" data-testid="inbox-error">
        <span class="note-text">{inbox.error}</span>
      </p>
    {/if}

    {#if now.kind === "list"}
      <InboxList {layout} onopen={open} ontoggle={toggle} onpin={(pr, pinned) => void setPinned(pr, pinned)} />
      {#if partial}
        <p class="partial" data-testid="inbox-partial">The newest 100 of each group are shown.</p>
      {/if}
    {:else if now.kind === "loading"}
      <div class="skeleton" aria-hidden="true" data-testid="inbox-loading">
        <div class="bone-head"><span class="bone w-30"></span></div>
        {#each [70, 55, 80, 60] as w (w)}
          <div class="bone-row"><span class="bone" style:width="{w}%"></span><span class="bone w-40"></span></div>
        {/each}
      </div>
      <span class="sr-only" role="status">Loading your pull requests…</span>
    {:else}
      <div class="state" data-testid="inbox-state" data-state={now.kind === "problem" ? now.problem : "empty"}>
        <div class="box">
          {#if now.kind === "empty"}
            <p class="lead">No open pull requests need you</p>
            <p>None of yours are open, none wait for your review, and you are not mentioned in or assigned to any{accounts ? ` (${accounts})` : ""}.</p>
          {:else}
            <p class="lead">{WHAT[now.problem]}</p>
            <p>{now.message}</p>
            {#if now.detail}
              <details class="detail">
                <summary>What gh said</summary>
                <pre>{now.detail}</pre>
              </details>
            {/if}
          {/if}
          <div class="box-actions">
            {#if now.kind === "problem" && now.problem === "no_gh"}
              <button class="button" type="button" onclick={() => void openWebPage("link", "https://cli.github.com")}>Get the GitHub CLI</button>
            {/if}
            <button class="button" type="button" data-testid="inbox-retry" disabled={inbox.loading} onclick={() => void refreshInbox("user")}>
              {inbox.loading ? "Refreshing…" : "Refresh"}
            </button>
          </div>
        </div>
      </div>
    {/if}
  {/if}
</section>

<style>
  /* The page spans the window: each part centres what it holds, so the list's scroll bar is at
     the window's edge. */
  .inbox {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
    width: 100%;
    max-width: 960px;
    height: 44px;
    margin: 0 auto;
    padding: 0 10px 0 16px;
  }

  h1 {
    margin: 0;
    flex: none;
    color: var(--text);
    font-size: 15px;
    font-weight: 650;
  }

  @media (max-width: 600px) {
    .who {
      display: none;
    }
  }

  .who {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .spacer {
    flex: 1 1 auto;
  }

  .stale {
    flex: none;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }

  .spinning {
    animation: spin 0.8s linear infinite;
  }

  /* A host that failed while the list shows: a quiet line, as a notice reads. */
  .note {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    flex: none;
    width: calc(100% - 24px);
    max-width: 936px;
    margin: 0 auto 6px;
    padding: 7px 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 18px;
  }

  .note svg {
    flex: none;
    margin-top: 2px;
    color: var(--warning);
  }

  .note strong {
    color: var(--text);
    font-weight: 600;
  }

  .note-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .partial {
    flex: none;
    width: 100%;
    max-width: 960px;
    margin: 0 auto;
    padding: 6px 16px 8px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .state {
    display: flex;
    justify-content: center;
    flex: 1 1 auto;
    padding: 48px 16px;
    overflow-y: auto;
  }

  .box {
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: min(460px, 100%);
    height: fit-content;
    padding: 16px 18px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    background: var(--bg-elevated);
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 1.5;
    overflow-wrap: anywhere;
  }

  .box p {
    margin: 0;
  }

  .lead {
    color: var(--text);
    font-size: var(--fs);
    font-weight: 600;
  }

  .box-actions {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }

  .button {
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
  }

  .button:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .button:disabled {
    color: var(--text-faint);
  }

  .detail {
    font-size: var(--fs-xs);
  }

  .detail pre {
    max-height: 160px;
    margin: 6px 0 0;
    overflow: auto;
    font-family: var(--font-mono);
    white-space: pre-wrap;
  }

  .skeleton {
    display: flex;
    flex-direction: column;
    width: 100%;
    max-width: 960px;
    margin: 0 auto;
    padding: 0 8px;
  }

  .bone-head {
    display: flex;
    align-items: flex-end;
    height: 34px;
    padding: 0 8px 9px;
  }

  .bone-row {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 8px;
    height: 56px;
    padding: 0 8px 0 26px;
  }

  .bone {
    display: block;
    height: 9px;
    border-radius: 5px;
    background: var(--bg-hover);
  }

  .w-30 {
    width: 30%;
  }

  .w-40 {
    width: 40%;
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
