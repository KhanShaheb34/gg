<!--
  One comment of a pull request in the Pull request tab: its author (a "bot" tag for a bot's),
  when, what it is (a review's verdict; a thread's path:line, Resolved, Outdated), its body
  drawn as Markdown without its HTML (Markdown.svelte), and "Open on GitHub". A thread's
  replies sit indented under its first comment. A resolved thread (or one hidden on GitHub)
  shows its first lines only, without its replies; any body longer than about 12 lines is
  shortened; "Show more" and "Show less" toggle either, and the user's choice is kept while
  the app runs.
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
  import type { PrComment, PrReply } from "../types";
  import Markdown from "./Markdown.svelte";

  type Props = {
    comment: PrComment;
    dateFormat: DateFormat;
    now: number;
    /** Opens a comment's page (on the repository's GitHub host). */
    onpage: (url: string) => void;
    /** Opens a link of a body (any https address). */
    onlink: (url: string) => void;
  };

  let { comment, dateFormat, now, onpage, onlink }: Props = $props();

  const shortened = $derived(startsShortened(comment));
  const expanded = $derived(toggled.get(comment.id) ?? false);
  const rendered = $derived(renderMarkdown(comment.body));
  const replies = $derived((comment.thread?.replies ?? []).map((r) => ({ ...r, rendered: renderMarkdown(r.body) })));
  /** The body is taller than the lines shown before "Show more". */
  let overflows = $state(false);
  /** Something is hidden until "Show more": the body's end, or a resolved thread's replies. */
  const hasMore = $derived(overflows || (shortened === "resolved" && replies.length > 0));

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
  {#if hasMore || expanded}
    <button
      class="text-btn more"
      type="button"
      aria-expanded={expanded}
      data-testid="pr-comment-more"
      onclick={() => toggled.set(comment.id, !expanded)}
    >
      {expanded ? "Show less" : shortened === "resolved" && replies.length > 0 ? `Show more (${replies.length} ${replies.length === 1 ? "reply" : "replies"})` : "Show more"}
    </button>
  {/if}
</article>

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
</style>
