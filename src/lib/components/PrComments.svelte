<!--
  A pull request's comments in the Pull request tab: a header with how many there are and an
  All / Humans / Bots filter (kept while the app runs), then each comment as a card
  (PrCommentCard), oldest first. Says so when the filter leaves none, when older comments or
  review threads were left out, and when the review threads could not be read. With `writes`,
  each thread offers what the user may do to it (PrCommentCard), and a box at the end adds a
  comment to the conversation, unless GitHub says the user cannot (archived, or locked to them).
-->
<script lang="ts" module>
  import type { CommentFilter } from "../prDetails";

  /** The filter chosen, the same in every repository while the app runs. */
  const chosen = $state<{ filter: CommentFilter }>({ filter: "all" });
</script>

<script lang="ts">
  import type { DateFormat } from "../format";
  import { commentCounts, filterComments, noComments } from "../prDetails";
  import type { PrWrites } from "../state/prCompose.svelte";
  import type { PrDetails } from "../types";
  import PrCommentCard from "./PrCommentCard.svelte";
  import PrComposer from "./PrComposer.svelte";
  import Segmented from "./Segmented.svelte";

  type Props = {
    details: PrDetails;
    dateFormat: DateFormat;
    now: number;
    onpage: (url: string) => void;
    onlink: (url: string) => void;
    /** What can be written to the pull request; none: read-only. */
    writes?: PrWrites | null;
  };

  let { details, dateFormat, now, onpage, onlink, writes = null }: Props = $props();

  const counts = $derived(commentCounts(details.comments));
  const shown = $derived(filterComments(details.comments, chosen.filter));
</script>

<section class="comments" aria-label="Comments" data-testid="pr-comments">
  <div class="section-head">
    <span class="title">Comments</span>
    <span class="count" data-testid="pr-comments-count">{counts.all + details.commentsOmitted}</span>
    <span class="spacer"></span>
    {#if counts.all > 0}
      <Segmented
        label="Show comments from"
        testid="pr-comments-filter"
        options={[
          { value: "all", label: "All", title: `All comments (${counts.all})` },
          { value: "humans", label: "Humans", title: `Comments from people (${counts.humans})` },
          { value: "bots", label: "Bots", title: `Comments from bots (${counts.bots})` },
        ]}
        value={chosen.filter}
        onchange={(v) => (chosen.filter = v)}
      />
    {/if}
  </div>
  {#if details.threadsError}
    <p class="note" data-testid="pr-threads-error" title={details.threadsError}>Code comments could not be loaded: {details.threadsError}</p>
  {/if}
  {#if details.commentsOmitted > 0}
    <p class="note">{details.commentsOmitted} older {details.commentsOmitted === 1 ? "comment is" : "comments are"} on GitHub only.</p>
  {/if}
  {#if shown.length === 0}
    <p class="none" data-testid="pr-comments-none">{noComments(counts.all === 0 ? "all" : chosen.filter)}</p>
  {:else}
    <ol class="list">
      {#each shown as c (c.id || c.url)}
        <li><PrCommentCard comment={c} {dateFormat} {now} {onpage} {onlink} {writes} /></li>
      {/each}
    </ol>
  {/if}
  {#if details.threadsPartial}
    <p class="note">Only the first 50 code comment threads are shown.</p>
  {/if}
  {#if writes && details.canComment}
    <div class="add">
      <PrComposer
        draft={writes.draft(null)}
        placeholder="Add a comment…"
        label="Add a comment to pull request #{details.number}"
        sendLabel="Comment"
        onsend={() => writes?.send(null)}
        onlog={writes.showLog}
        testid="pr-comment-box"
      />
    </div>
  {/if}
</section>

<style>
  .comments {
    display: flex;
    flex-direction: column;
    min-width: 0;
    padding-bottom: 12px;
  }

  /* The filter goes under the title when the panel is too narrow for both on one line. */
  .section-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 6px;
    min-width: 0;
    min-height: 30px;
    padding: 4px 10px 4px 28px;
  }

  .section-head :global(.segmented) {
    flex: none;
    padding: 1px;
    border-radius: 7px;
  }

  .section-head :global(.segment) {
    height: 20px;
    padding: 0 7px;
    border-radius: 5px;
    font-size: var(--fs-xs);
  }

  .title {
    flex: none;
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
  }

  .count {
    flex: none;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }

  .spacer {
    flex: 1 1 0;
    min-width: 0;
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 2px 0 0;
    padding: 0 10px;
    list-style: none;
  }

  .list > li {
    flex: none;
    min-width: 0;
  }

  .none {
    padding: 6px 12px 0 28px;
    color: var(--text-faint);
    font-size: var(--fs-sm);
  }

  /* The conversation's box, after its last comment. */
  .add {
    margin: 10px 10px 0;
  }

  .note {
    margin: 0 10px 6px;
    padding: 0 2px 0 18px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
  }
</style>
