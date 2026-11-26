<!--
  One comment of a pull request in the Pull request tab: its author (a "bot" tag for a bot's),
  when, what it is (a review's verdict; a thread's path:line, Resolved, Outdated), its body
  drawn as Markdown without its HTML (Markdown.svelte), and "Open on GitHub". A thread's
  replies sit indented under its first comment. A resolved thread (or one hidden on GitHub)
  shows its first lines only, without its replies; any body longer than about 12 lines is
  shortened; "Show more" and "Show less" toggle either, and the user's choice is kept while
  the app runs.

  A thread also takes what the user may do to it, as GitHub says (`writes` given): a quiet
  "Reply…" box under its replies (PrComposer) and Resolve; a resolved one, collapsed, offers
  Unresolve only, and its box once shown whole. What the user may not do is not offered. Each
  change is sent only on the user's click (or ⌘↩ in the box), and shows once gh has it.
-->
<script lang="ts" module>
  import { SvelteMap } from "svelte/reactivity";

  /** The comments the user opened (true) or closed (false), by id. */
  const toggled = new SvelteMap<string, boolean>();
</script>

<script lang="ts">
  import { formatDate, formatDateLong, type DateFormat } from "../format";
  import { renderMarkdown, type Rendered } from "../markdown";
  import { shownLines, startsShortened } from "../prDetails";
  import type { PrWrites } from "../state/prCompose.svelte";
  import type { PrComment, PrReply } from "../types";
  import Markdown from "./Markdown.svelte";
  import PrComposer from "./PrComposer.svelte";

  type Props = {
    comment: PrComment;
    dateFormat: DateFormat;
    now: number;
    /** Opens a comment's page (on the repository's GitHub host). */
    onpage: (url: string) => void;
    /** Opens a link of a body (any https address). */
    onlink: (url: string) => void;
    /** What can be written to the pull request (a thread's reply, resolving it); none: read-only. */
    writes?: PrWrites | null;
  };

  let { comment, dateFormat, now, onpage, onlink, writes = null }: Props = $props();

  /** The thread the user may act on, with what they may do; null for anything else. */
  const acts = $derived.by(() => {
    const t = comment.thread;
    if (!writes || !t || t.id === "") return null;
    const reply = t.canReply;
    const resolve = !t.resolved && t.canResolve;
    const unresolve = t.resolved && t.canUnresolve;
    return reply || resolve || unresolve ? { id: t.id, reply, resolve, unresolve } : null;
  });
  const resolving = $derived(acts && writes ? writes.resolving(acts.id) : null);

  const shortened = $derived(startsShortened(comment));
  const expanded = $derived(toggled.get(comment.id) ?? false);
  const rendered = $derived(renderMarkdown(comment.body));
  const replies = $derived((comment.thread?.replies ?? []).map((r) => ({ ...r, rendered: renderMarkdown(r.body) })));
  /** The body is taller than the lines shown before "Show more". */
  let overflows = $state(false);
  /** A resolved thread (or a hidden comment) shown folded: its first lines only. */
  const folded = $derived(shortened === "resolved" && !expanded);
  /**
   * Something is hidden until "Show more": the body's end, a resolved thread's replies, or the
   * box to reply in it.
   */
  const hasMore = $derived(overflows || (shortened === "resolved" && (replies.length > 0 || acts?.reply === true)));

  const context = $derived.by(() => {
    const parts: string[] = [];
    if (comment.kind === "review") {
      const verdict = {
        approved: "Approved",
        changes_requested: "Requested changes",
        commented: "Reviewed",
        dismissed: "Review dismissed",
        pending: "Review pending",
        unknown: "Reviewed",
      }[comment.reviewState ?? "unknown"];
      parts.push(verdict);
    }
    if (comment.minimized) parts.push("Hidden on GitHub");
    return parts;
  });

  function when(ms: number | null): { text: string; title: string } | null {
    if (ms === null) return null;
    return { text: formatDate(ms / 1000, dateFormat, now), title: formatDateLong(ms / 1000, dateFormat, now) };
  }

  /** Watches whether the body is taller than its room (it changes as disclosures open). */
  function measure(node: HTMLElement) {
    const check = () => (overflows = node.scrollHeight > node.clientHeight + 2);
    const observer = new ResizeObserver(check);
    observer.observe(node);
    const inner = node.firstElementChild;
    if (inner) observer.observe(inner);
    check();
    return { destroy: () => observer.disconnect() };
  }
</script>

{#snippet head(author: string, isBot: boolean, ms: number | null, url: string, reply: boolean)}
  {@const at = when(ms)}
  <div class="head" class:reply>
    <span class="author" title={author}>{author}</span>
    {#if isBot}<span class="bot-tag" title="A bot (an app's account)">bot</span>{/if}
    {#if at}<span class="time" title={at.title}>{at.text}</span>{/if}
    <span class="spacer"></span>
    <button
      class="icon-btn page"
      type="button"
      aria-label="Open on GitHub"
      title="Open on GitHub"
      data-testid="pr-comment-open"
      onclick={() => onpage(url)}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
        <path d="M5 2.2H3.1A1.1 1.1 0 0 0 2 3.3v5.6A1.1 1.1 0 0 0 3.1 10h5.6A1.1 1.1 0 0 0 9.8 8.9V7" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" />
        <path d="M6.6 2h3.4v3.4M10 2 5.6 6.4" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    </button>
  </div>
{/snippet}

<!-- A body: shortened to `clamp` lines (null: whole). The focus going into its hidden part
     (a link, by Tab) shows it whole, so nothing focused is ever out of sight. -->
{#snippet body(r: Rendered, url: string, clamp: number | null)}
  {#if r.blocks.length === 0}
    <p class="empty">No text.</p>
  {:else if clamp !== null}
    <div class="clip" class:faded={overflows} style:--lines={clamp} use:measure onfocusin={() => overflows && toggled.set(comment.id, true)}>
      <div><Markdown blocks={r.blocks} {onlink} /></div>
    </div>
  {:else}
    <Markdown blocks={r.blocks} {onlink} />
  {/if}
  {#if r.cut}
    <p class="cut">The rest is too long to show here: <button class="text-btn" type="button" onclick={() => onpage(url)}>open it on GitHub</button>.</p>
  {/if}
{/snippet}

{#snippet replyCard(r: PrReply & { rendered: Rendered })}
  <li class="reply-item" data-testid="pr-reply">
    {@render head(r.author, r.isBot, r.createdAtMs, r.url, true)}
    <div class="selectable">{@render body(r.rendered, r.url, null)}</div>
  </li>
{/snippet}

<article
  class="card"
  class:bot={comment.isBot}
  class:resolved={comment.thread?.resolved}
  aria-label="{comment.kind === 'thread' ? 'Code comment' : comment.kind === 'review' ? 'Review' : 'Comment'} by {comment.author}"
  data-testid="pr-comment"
  data-kind={comment.kind}
  data-bot={comment.isBot}
>
  {@render head(comment.author, comment.isBot, comment.createdAtMs, comment.url, false)}
  {#if comment.thread}
    {@const t = comment.thread}
    <p class="context" data-testid="pr-comment-context">
      <span class="path" title={t.line !== null ? `${t.path}:${t.line}` : t.path}><bdi>{t.path}{#if t.line !== null}<span class="line">:{t.line}</span>{/if}</bdi></span>
      {#if t.resolved}<span class="tag resolved-tag">Resolved</span>{/if}
      {#if t.outdated}<span class="tag">Outdated</span>{/if}
    </p>
  {:else if context.length > 0}
    <p class="context">
      {#each context as c, i (i)}<span class="tag" class:verdict={i === 0 && comment.kind === "review"} data-state={comment.reviewState}>{c}</span>{/each}
    </p>
  {/if}
  <div class="body selectable" data-testid="pr-comment-body">
    {@render body(rendered, comment.url, expanded ? null : shownLines(shortened))}
  </div>
  {#if replies.length > 0 && (expanded || shortened !== "resolved")}
    <ul class="replies" aria-label="Replies">
      {#each replies as r (r.id)}{@render replyCard(r)}{/each}
    </ul>
  {/if}
  {#if acts && folded}
    <!-- Folded: Show more, and Unresolve on the same line. -->
    <div class="foot">
      {#if hasMore}{@render more()}{/if}
      <span class="spacer"></span>
      {#if acts.unresolve}{@render resolveButton(acts.id, false)}{/if}
    </div>
    {@render resolveFailed()}
  {:else}
    {#if hasMore || expanded}{@render more()}{/if}
    {#if acts && writes}
      <div class="acts">
        {#if acts.reply}
          <PrComposer
            draft={writes.draft(acts.id)}
            placeholder="Reply…"
            label="Reply to {comment.author} on {comment.thread?.path ?? 'the code'}"
            sendLabel="Reply"
            onsend={() => writes?.send(acts.id)}
            onlog={writes.showLog}
            testid="pr-reply-box"
            lead={acts.resolve || acts.unresolve ? lead : undefined}
          />
        {:else}
          <div class="foot">{@render lead()}</div>
        {/if}
        {@render resolveFailed()}
      </div>
    {/if}
  {/if}
</article>

{#snippet more()}
  <button
    class="text-btn more"
    type="button"
    aria-expanded={expanded}
    data-testid="pr-comment-more"
    onclick={() => toggled.set(comment.id, !expanded)}
  >
    {expanded ? "Show less" : shortened === "resolved" && replies.length > 0 ? `Show more (${replies.length} ${replies.length === 1 ? "reply" : "replies"})` : "Show more"}
  </button>
{/snippet}

{#snippet lead()}
  {#if acts?.resolve}{@render resolveButton(acts.id, true)}{:else if acts?.unresolve}{@render resolveButton(acts.id, false)}{/if}
{/snippet}

<!-- Resolve (or Unresolve) the thread: quiet, beside the box; sent on the click alone. -->
{#snippet resolveButton(id: string, resolve: boolean)}
  {@const pending = resolving?.pending ?? false}
  <button
    class="resolve"
    type="button"
    disabled={pending}
    aria-label={pending ? (resolve ? "Resolving the conversation" : "Unresolving the conversation") : resolve ? "Resolve conversation" : "Unresolve conversation"}
    title={resolve ? "Resolve this conversation on GitHub" : "Open this conversation again on GitHub"}
    data-testid="pr-thread-resolve"
    data-action={resolve ? "resolve" : "unresolve"}
    onclick={() => writes?.setResolved(id, resolve)}
  >
    {#if pending}
      <svg class="spin" width="12" height="12" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.3" stroke-width="1.8" />
        <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
      </svg>
    {:else if resolve}
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
        <circle cx="6" cy="6" r="4.9" fill="none" stroke="currentColor" stroke-width="1.1" />
        <path d="M3.9 6.1 5.4 7.6 8.2 4.6" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    {:else}
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
        <path d="M2.6 4.4h4.6a2.6 2.6 0 0 1 0 5.2H5" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" />
        <path d="M4.3 2.6 2.5 4.4l1.8 1.8" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
    {/if}
    <span>{pending ? (resolve ? "Resolving…" : "Unresolving…") : resolve ? "Resolve" : "Unresolve"}</span>
  </button>
{/snippet}

{#snippet resolveFailed()}
  {#if resolving?.error && !resolving.pending}
    <p class="failed" role="alert" data-testid="pr-thread-resolve-error">
      <span class="failed-text" title={resolving.error}>{resolving.error}</span>
      {#if resolving.logFrom > 0}
        <button class="text-btn" type="button" onclick={() => writes?.showLog(resolving?.logFrom ?? 0)}>Show log</button>
      {/if}
    </p>
  {/if}
{/snippet}

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    padding: 8px 10px 9px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-elevated);
  }

  .card > :global(*) {
    flex: none;
  }

  /* A bot's comment: quieter than a person's. */
  .card.bot {
    background: transparent;
  }

  .card.bot .body {
    color: var(--text-muted);
  }

  .card.bot .body :global(.md) {
    color: var(--text-muted);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    height: 20px;
  }

  /* The name whole (but a very long one), the time cut first. */
  .author {
    flex: 0 0 auto;
    max-width: 62%;
    min-width: 0;
    overflow: hidden;
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bot-tag {
    flex: none;
    height: 15px;
    padding: 0 5px;
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: 10px;
    font-weight: 600;
    line-height: 13px;
    letter-spacing: 0.02em;
  }

  .time {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .spacer {
    flex: 1 1 0;
    min-width: 0;
  }

  .page {
    flex: none;
    width: 20px;
    height: 20px;
    margin-right: -4px;
    border-radius: 5px;
    color: var(--text-faint);
  }

  .context {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 6px;
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }

  .path {
    min-width: 0;
    max-width: 100%;
    overflow: hidden;
    font-family: var(--font-mono);
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }

  .line {
    color: var(--text-faint);
  }

  .tag {
    flex: none;
    height: 16px;
    padding: 0 5px;
    border-radius: 4px;
    background: var(--bg-chrome);
    color: var(--text-muted);
    font-size: 10.5px;
    font-weight: 550;
    line-height: 16px;
  }

  .resolved-tag {
    background: color-mix(in srgb, var(--success) 12%, transparent);
    color: color-mix(in srgb, var(--success) 80%, var(--text));
  }

  .verdict[data-state="approved"] {
    background: color-mix(in srgb, var(--success) 12%, transparent);
    color: color-mix(in srgb, var(--success) 80%, var(--text));
  }

  .verdict[data-state="changes_requested"] {
    background: var(--danger-tint);
    color: color-mix(in srgb, var(--danger) 80%, var(--text));
  }

  .body {
    min-width: 0;
    margin-top: 2px;
  }

  /* A shortened body: its first lines, fading out at the bottom when more follows. */
  .clip {
    max-height: calc(var(--lines) * 1.5 * var(--fs-sm));
    overflow: hidden;
  }

  .clip.faded {
    -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 28px), transparent);
    mask-image: linear-gradient(to bottom, #000 calc(100% - 28px), transparent);
  }

  .empty,
  .cut {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-style: italic;
  }

  .cut {
    margin-top: 6px;
    font-style: normal;
  }

  .replies {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 6px 0 0 4px;
    padding: 0 0 0 10px;
    border-left: 2px solid var(--border);
    list-style: none;
  }

  .reply-item {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .reply-item > :global(*) {
    flex: none;
  }

  .head.reply .author {
    font-size: var(--fs-xs);
  }

  .text-btn {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 550;
  }

  .text-btn:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .text-btn:focus-visible {
    border-radius: 3px;
    outline-offset: 2px;
  }

  .more {
    align-self: flex-start;
    margin-top: 2px;
  }

  /* What the user may do to a thread: under its replies, apart from them. */
  .acts {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
    margin-top: 2px;
  }

  .foot {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    min-height: 24px;
  }

  .foot .more {
    align-self: center;
    margin-top: 0;
  }

  /* Resolve and Unresolve: a quiet button, its colour only on hover. */
  .resolve {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    height: 24px;
    margin-left: -6px;
    padding: 0 7px 0 6px;
    border: 1px solid transparent;
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-weight: 550;
    white-space: nowrap;
    transition:
      background-color 80ms linear,
      color 80ms linear;
  }

  .foot .spacer + .resolve {
    margin-left: 0;
    margin-right: -6px;
  }

  .resolve svg {
    flex: none;
  }

  .resolve:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text);
  }

  .resolve[data-action="resolve"]:hover:not(:disabled) svg {
    color: color-mix(in srgb, var(--success) 85%, var(--text));
  }

  .resolve:active:not(:disabled) {
    background: var(--bg-pressed);
  }

  .resolve:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 1px;
  }

  .resolve:disabled {
    color: var(--text-faint);
  }

  .spin {
    animation: spin 0.8s linear infinite;
  }

  /* Why it could not be resolved: as the tab's own failed line. */
  .failed {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    margin: 0;
    padding: 4px 8px 4px 9px;
    border: 1px solid color-mix(in srgb, var(--danger) 30%, var(--border));
    border-radius: 6px;
    background: var(--danger-tint);
    color: var(--text);
    font-size: var(--fs-xs);
    line-height: 1.4;
  }

  .failed-text {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }
</style>
