<!--
  A commit's ref labels: compact pills tinted with the row's lane colour (`--c`, set on the
  row). A branch at the same commit as its remote branches is one pill (`main | origin`); the
  branch checked out in the active worktree is bold with a ring, like HEAD's node; tags are
  outlined; stashes (`stash@{0}`) have a colour of their own, not the lane's, and a dotted
  edge. A branch checked out in another worktree carries a folder mark (the side panel's), and a
  worktree detached at a commit a folder pill; the active worktree's HEAD stays the boldest. Never wraps; the description cell clips what does not fit. The graph finds the label
  a right-click is on by its `data-slot` (a slot's index, or "more" for the "+k" pill).
-->
<script lang="ts" module>
  /** Label slots per row: rows are recycled while scrolling, and so are their labels. */
  const SLOTS = [0, 1, 2] as const;

  /**
   * How many of `count` labels have a slot of their own: all of up to 3; else 2, and a "+k"
   * pill stands for the rest.
   */
  export function ownLabels(count: number): number {
    return count <= SLOTS.length ? count : SLOTS.length - 1;
  }
</script>

<script lang="ts">
  import PrChip from "../components/PrChip.svelte";
  import { shortenPath } from "../format";
  import type { PullRequest, RefLabel } from "../types";

  type Props = {
    labels: RefLabel[];
    /** HEAD of the active worktree is detached at this commit. */
    detachedHead: boolean;
    /** Paths of other worktrees detached at this commit. */
    detachedWorktrees: string[];
    homeDir: string | null;
    /** "as of 14:05" when the pull-request markers may be out of date. */
    prStale?: string | null;
    onOpenPr?: (pr: PullRequest) => void;
  };

  let { labels, detachedHead, detachedWorktrees, homeDir, prStale = null, onOpenPr = () => {} }: Props = $props();

  /**
   * Labels with a slot of their own: all of up to 3; else 2, and "+k" for the rest, whose
   * tooltip describes each hidden label as a shown one's does.
   */
  const own = $derived(ownLabels(labels.length));
  const rest = $derived(labels.length - own);

  function title(label: RefLabel): string {
    const kinds = { tag: "Tag", remote: "Remote branch", stash: "Stash", branch: "Branch" } as const;
    const parts = [`${kinds[label.kind]} ${label.name}`];
    if (label.current) parts.push("checked out in the active worktree");
    if (label.remotes.length > 0) parts.push(`same commit on ${label.remotes.join(", ")}`);
    if (label.upstream && (label.ahead > 0 || label.behind > 0)) {
      parts.push(`${label.ahead} ahead, ${label.behind} behind ${label.upstream}`);
    }
    if (label.worktreePath && !label.current) parts.push(`checked out in ${shortenPath(label.worktreePath, homeDir)}`);
    return parts.join(" · ");
  }

  /** A label folded into "+k": as a shown one's tooltip, and its pull request, whose marker is folded with it. */
  function hiddenTitle(label: RefLabel): string {
    const pr = label.pr;
    if (!pr) return title(label);
    return `${title(label)} · ${pr.state === "draft" ? "draft " : ""}pull request #${pr.number}${prStale ? ` (${prStale})` : ""}`;
  }

  function aheadBehind(label: RefLabel): string {
    const ahead = label.ahead > 0 ? `↑${label.ahead}` : "";
    const behind = label.behind > 0 ? `↓${label.behind}` : "";
    return ahead && behind ? `${ahead}\u2009${behind}` : ahead + behind;
  }
</script>

<!-- Every element here is always there, shown or hidden, so a recycled row changes text and
     attributes only: scrolling creates and destroys nothing. -->
<span class="ref head current" data-ref="HEAD" title="HEAD of the active worktree (detached)" hidden={!detachedHead}>
  <svg class="icon" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
    <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.7" />
  </svg>
  <span class="name">HEAD</span>
</span>
{#each SLOTS as k (k)}
  {@const label = k < own ? labels[k] : undefined}
  <span
    class="ref {label?.kind ?? ''}"
    class:current={label?.current}
    hidden={!label}
    data-slot={k}
    data-ref={label?.name}
    data-current={label?.current}
    title={label ? title(label) : undefined}
  >
    <svg class="icon" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <g class="i-current">
        <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.7" />
      </g>
      <g class="i-tag">
        <path d="M1.2 1.2h3.6l4 4-3.6 3.6-4-4z" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" />
        <circle cx="3.4" cy="3.4" r="0.8" fill="currentColor" />
      </g>
      <g class="i-stash">
        <path d="M1.3 4.2h7.4v4.3H1.3zM2.2 1.6h5.6l.9 2.6H1.3z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round" />
        <path d="M3.6 6.2h2.8" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" />
      </g>
      <g class="i-branch">
        <circle cx="3" cy="2" r="1.25" fill="currentColor" />
        <circle cx="3" cy="8" r="1.25" fill="currentColor" />
        <circle cx="7.2" cy="3.4" r="1.25" fill="currentColor" />
        <path d="M3 2v6M7.2 3.4c0 2.2-4.2 1.6-4.2 3.8" fill="none" stroke="currentColor" stroke-width="1.1" />
      </g>
    </svg>
    <span class="name">{label?.name ?? ""}</span>
    <span class="remotes">{label ? label.remotes.join(", ") : ""}</span>
    <span class="ab">{label ? aheadBehind(label) : ""}</span>
    <svg
      class="wt"
      class:shown={!!label?.worktreePath && !label.current}
      width="11"
      height="10"
      viewBox="0 0 11 10"
      aria-label="in a worktree"
    >
      <path d="M1 2.2c0-.5.4-.9.9-.9h2.4l1 1.1h3.8c.5 0 .9.4.9.9v4.6c0 .5-.4.9-.9.9H1.9c-.5 0-.9-.4-.9-.9z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round" />
    </svg>
  </span>
  <PrChip pr={label?.pr ?? null} stale={prStale} onOpen={onOpenPr} />
{/each}
<span class="ref more" data-slot="more" hidden={rest === 0} title={rest === 0 ? undefined : labels.slice(own).map(hiddenTitle).join("\n")}
  >+{rest}</span
>
<span
  class="ref worktree"
  hidden={detachedWorktrees.length === 0}
  title={detachedWorktrees.length === 0
    ? undefined
    : `Worktree detached here (not the active one): ${detachedWorktrees.map((p) => shortenPath(p, homeDir)).join(", ")}`}
>
  <svg class="icon" width="11" height="10" viewBox="0 0 11 10" aria-hidden="true">
    <path d="M1 2.2c0-.5.4-.9.9-.9h2.4l1 1.1h3.8c.5 0 .9.4.9.9v4.6c0 .5-.4.9-.9.9H1.9c-.5 0-.9-.4-.9-.9z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round" />
  </svg>
  <span class="name">{detachedWorktrees.length > 1 ? detachedWorktrees.length : ""}</span>
</span>

<style>
  .ref {
    --tint: color-mix(in srgb, var(--c) 13%, var(--bg));
    --ink: color-mix(in srgb, var(--c) 80%, var(--text));
    display: inline-flex;
    align-items: center;
    gap: 4px;
    /* A long name gives way (cut at its end) before its pull-request marker is cut off by
       the cell, once the subject beside it has none left. */
    flex: 0 1 auto;
    min-width: 0;
    /* Squeezed below what it holds (a narrow description column), it cuts its own end rather
       than drawing it over the next pill. */
    overflow: hidden;
    height: 18px;
    padding: 0 6px 0 5px;
    border: 1px solid color-mix(in srgb, var(--c) 34%, transparent);
    border-radius: 5px;
    background: var(--tint);
    color: var(--ink);
    font-size: var(--fs-xs);
    font-weight: 550;
    line-height: 16px;
    white-space: nowrap;
  }

  .ref[hidden] {
    display: none;
  }

  .name {
    min-width: 2.5em;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .remotes,
  .ab,
  .more,
  .head {
    flex: none;
  }

  .icon,
  .wt {
    display: block;
    flex: none;
  }

  /* One icon of three shows: current (a ring), tag, or branch. */
  .icon g {
    display: none;
  }

  .current .i-current,
  .tag:not(.current) .i-tag,
  .stash .i-stash,
  .ref:not(.current):not(.tag):not(.stash) .i-branch {
    display: inline;
  }

  .remotes:empty,
  .ab:empty,
  .worktree .name:empty {
    display: none;
  }

  .more {
    --c: var(--text-muted);
    padding: 0 5px;
  }

  .icon {
    opacity: 0.9;
  }

  .current {
    border-color: color-mix(in srgb, var(--c) 70%, transparent);
    font-weight: 700;
  }

  /* Only on the remote: a quieter fill. */
  .remote {
    --tint: color-mix(in srgb, var(--c) 6%, var(--bg));
    border-style: dashed;
  }

  .tag {
    background: transparent;
  }

  /* A stash belongs to no lane: its own colour, a dotted edge. */
  .stash {
    --c: var(--stash);
    border-style: dotted;
    font-weight: 500;
  }

  .worktree {
    padding: 0 5px;
  }

  .remotes {
    padding-left: 5px;
    border-left: 1px solid color-mix(in srgb, var(--c) 34%, transparent);
    font-style: italic;
    font-weight: 500;
    opacity: 0.85;
  }

  .current .remotes {
    font-weight: 600;
  }

  .ab {
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  /* Checked out in another worktree: a folder mark. */
  .wt {
    display: none;
    opacity: 0.8;
  }

  .wt.shown {
    display: block;
  }
</style>
