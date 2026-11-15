<!--
  The marker of a branch's open pull request: "#123" with the pull-request mark, after the
  branch's label in the graph and on its worktree's row. A draft is hollow and quiet; a marker
  from a list that may be out of date (older than 10 minutes, or the last reload failed) is
  dimmed and its tooltip says as of when. A button: it opens the pull request in the browser,
  and its click never reaches the row (no selection, no double-click action). Out of the Tab
  order, like the rows' other small buttons: the row's menu has "Open pull request #N".
  Always there, hidden without a pull request, so a recycled graph row only changes it.
-->
<script lang="ts">
  import { prTooltip } from "../state/prs.svelte";
  import type { PullRequest } from "../types";

  type Props = {
    pr: PullRequest | null;
    /** "as of 14:05" when the list it comes from may be out of date. */
    stale: string | null;
    onOpen: (pr: PullRequest) => void;
  };

  let { pr, stale, onOpen }: Props = $props();
</script>

<button
  type="button"
  class="pr-chip"
  class:draft={pr?.state === "draft"}
  class:stale={stale !== null}
  hidden={!pr}
  tabindex="-1"
  title={pr ? prTooltip(pr, stale) : undefined}
  aria-label={pr ? `Open ${pr.state === "draft" ? "draft " : ""}pull request #${pr.number}: ${pr.title}` : undefined}
  data-pr={pr?.number}
  onclick={(e) => {
    e.stopPropagation();
    if (pr) onOpen(pr);
  }}
  ondblclick={(e) => e.stopPropagation()}
  onpointerdown={(e) => e.stopPropagation()}
>
  <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
    <circle cx="2.4" cy="2" r="1.25" fill="none" stroke="currentColor" stroke-width="1.05" />
    <circle cx="2.4" cy="8" r="1.25" fill="none" stroke="currentColor" stroke-width="1.05" />
    <circle cx="7.6" cy="8" r="1.25" fill="none" stroke="currentColor" stroke-width="1.05" />
    <path d="M2.4 3.25v3.5M7.6 6.75V4.1c0-.9-.6-1.5-1.5-1.5H4.6" fill="none" stroke="currentColor" stroke-width="1.05" />
    <path d="M5.7 1.4 4.5 2.6l1.2 1.2" fill="none" stroke="currentColor" stroke-width="1.05" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
  <span class="n">#{pr?.number ?? ""}</span>
</button>

<style>
  .pr-chip {
    --pr: var(--success);
    display: inline-flex;
    align-items: center;
    gap: 3px;
    flex: none;
    height: 18px;
    padding: 0 5px 0 4px;
    border: 1px solid color-mix(in srgb, var(--pr) 32%, transparent);
    border-radius: 9px;
    background: color-mix(in srgb, var(--pr) 9%, var(--bg));
    color: var(--text-muted);
    font: inherit;
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    line-height: 16px;
    white-space: nowrap;
    cursor: pointer;
    transition:
      background-color 80ms linear,
      border-color 80ms linear;
  }

  .pr-chip[hidden] {
    display: none;
  }

  .pr-chip svg {
    display: block;
    flex: none;
    color: color-mix(in srgb, var(--pr) 85%, var(--text));
  }

  .pr-chip:hover {
    border-color: color-mix(in srgb, var(--pr) 55%, transparent);
    background: color-mix(in srgb, var(--pr) 16%, var(--bg));
    color: var(--text);
  }

  .pr-chip:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 1px;
  }

  /* A draft: hollow, quiet. */
  .draft {
    border-style: dashed;
    border-color: var(--border-strong);
    background: transparent;
    color: var(--text-faint);
  }

  .draft svg {
    color: var(--text-faint);
  }

  .draft:hover {
    border-color: var(--text-faint);
    background: var(--bg-hover);
  }

  /* From a list that may be out of date. */
  .stale {
    opacity: 0.55;
  }

  .stale:hover {
    opacity: 0.85;
  }
</style>
