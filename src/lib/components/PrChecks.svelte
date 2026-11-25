<!--
  A pull request's checks in the Pull request tab: a disclosure whose header sums them up
  ("2 passing · 1 running · 1 failing", "No checks"), and a row per check: its state's icon
  (turning while it runs, still when motion is reduced), its name (its workflow's, quieter,
  when that says more), how long it took or has run, and its page. Failing first, then
  running, then the rest. Open at first when something fails or runs, else closed; the
  user's own choice wins once made (kept per pull request while the app runs).
-->
<script lang="ts" module>
  import { SvelteMap } from "svelte/reactivity";

  /** The user's choice of open or closed, by pull request (repository root and number). */
  const chosen = new SvelteMap<string, boolean>();
</script>

<script lang="ts">
  import { checkStateText, checksOpenByDefault, checksSummaryText, checkTiming, orderChecks, workflowShown } from "../prDetails";
  import type { ChecksSummary, PrCheck } from "../types";

  type Props = {
    /** Which pull request (for the choice kept). */
    id: string;
    checks: PrCheck[];
    summary: ChecksSummary;
    now: number;
    onlink: (url: string) => void;
  };

  let { id, checks, summary, now, onlink }: Props = $props();

  const open = $derived(chosen.get(id) ?? checksOpenByDefault(summary));
  const ordered = $derived(orderChecks(checks));
  const listId = $derived(`pr-checks-${id.replace(/[^A-Za-z0-9_-]/g, "_")}`);
</script>

<section class="checks" aria-label="Checks" data-testid="pr-checks">
  <button
    class="section-head"
    type="button"
    aria-expanded={summary.total === 0 ? undefined : open}
    aria-controls={summary.total === 0 ? undefined : listId}
    disabled={summary.total === 0}
    data-testid="pr-checks-toggle"
    onclick={() => chosen.set(id, !open)}
  >
    <svg class="chevron" class:open class:none={summary.total === 0} width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M3.5 2l3 3-3 3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
    <span class="title">Checks</span>
    <span class="summary" data-testid="pr-checks-summary" title={checksSummaryText(summary)}>{checksSummaryText(summary)}</span>
  </button>
  {#if open && summary.total > 0}
    <ul class="rows" id={listId}>
      {#each ordered as c, i (i)}
        {@const workflow = workflowShown(c)}
        {@const timing = checkTiming(c, now)}
        <li class="row" data-testid="pr-check" data-state={c.state}>
          <span class="icon {c.state}" role="img" aria-label={checkStateText(c.state, c.startedAtMs !== null)} title={checkStateText(c.state, c.startedAtMs !== null)}>
            {#if c.state === "pending" && c.startedAtMs === null}
              <!-- Queued: a still ring with a dot, waiting. -->
              <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.6" />
                <circle cx="7" cy="7" r="1.5" fill="currentColor" />
              </svg>
            {:else if c.state === "pending"}
              <svg class="spin" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-opacity="0.28" stroke-width="1.6" />
                <path d="M7 2a5 5 0 0 1 5 5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
              </svg>
            {:else if c.state === "passing"}
              <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="6" fill="currentColor" fill-opacity="0.14" />
                <path d="M4.4 7.2 6.2 9l3.4-3.8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            {:else if c.state === "failing"}
              <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="6" fill="currentColor" fill-opacity="0.14" />
                <path d="M4.9 4.9l4.2 4.2M9.1 4.9 4.9 9.1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
              </svg>
            {:else if c.state === "cancelled"}
              <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="5.4" fill="none" stroke="currentColor" stroke-width="1.4" />
                <path d="M3.4 10.6 10.6 3.4" fill="none" stroke="currentColor" stroke-width="1.4" />
              </svg>
            {:else if c.state === "unknown"}
              <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="5.4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2 2" />
              </svg>
            {:else}
              <!-- Skipped or neutral: a dash. -->
              <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="5.4" fill="none" stroke="currentColor" stroke-width="1.4" />
                <path d="M4.6 7h4.8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
              </svg>
            {/if}
          </span>
          <span class="name" title={workflow ? `${workflow} / ${c.name}` : c.name}>
            {#if workflow}<span class="workflow">{workflow} /</span>{/if}
            {c.name}
          </span>
          {#if timing}<span class="timing">{timing}</span>{/if}
          {#if c.url}
            <button
              class="icon-btn page"
              type="button"
              aria-label="Open {c.name} in the browser"
              title="Open in the browser: {c.url}"
              data-testid="pr-check-open"
              onclick={() => onlink(c.url!)}
            >
              <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
                <path d="M5 2.2H3.1A1.1 1.1 0 0 0 2 3.3v5.6A1.1 1.1 0 0 0 3.1 10h5.6A1.1 1.1 0 0 0 9.8 8.9V7" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" />
                <path d="M6.6 2h3.4v3.4M10 2 5.6 6.4" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </button>
          {:else}
            <span class="page-gap"></span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .checks {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  /* As the comments' header: a quiet label and what it holds, the whole row the toggle. */
  .section-head {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    min-width: 0;
    height: 30px;
    padding: 0 12px;
    border: 0;
    background: transparent;
    text-align: left;
  }

  .section-head:not(:disabled):hover {
    background: var(--bg-hover);
  }

  .section-head:focus-visible {
    outline-offset: -2px;
  }

  .chevron {
    flex: none;
    color: var(--text-faint);
    transition: transform 80ms var(--ease);
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  .chevron.none {
    visibility: hidden;
  }

  .title {
    flex: none;
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
  }

  .summary {
    min-width: 0;
    margin-left: auto;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rows {
    margin: 0 0 6px;
    padding: 0;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    height: 26px;
    padding: 0 8px 0 28px;
  }

  .icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 14px;
    height: 14px;
    color: var(--text-faint);
  }

  .icon svg {
    display: block;
  }

  .icon.passing {
    color: var(--success);
  }

  .icon.failing {
    color: var(--danger);
  }

  .icon.pending {
    color: var(--warning);
  }

  .spin {
    animation: spin 1.1s linear infinite;
  }

  /* Still when motion is reduced: the ring and its arc, not turning. */
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none !important;
    }
  }

  .name {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    color: var(--text);
    font-size: var(--fs-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row[data-state="skipped"] .name,
  .row[data-state="neutral"] .name,
  .row[data-state="cancelled"] .name {
    color: var(--text-muted);
  }

  .workflow {
    color: var(--text-faint);
  }

  .timing {
    flex: none;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .page,
  .page-gap {
    flex: none;
    width: 20px;
    height: 20px;
  }

  .page {
    border-radius: 5px;
    color: var(--text-faint);
  }
</style>
