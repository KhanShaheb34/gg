<!--
  A commit's ref labels: compact pills tinted with the row's lane colour (`--c`, set on the
  row). A branch at the same commit as its remote branches is one pill (`main | origin`); the
  branch checked out in the active worktree is bold with a ring, like HEAD's node; tags are
  outlined. Never wraps; the description cell clips what does not fit.
-->
<script lang="ts">
  import { shortenPath } from "../format";
  import type { RefLabel } from "../types";

  type Props = {
    labels: RefLabel[];
    /** HEAD of the active worktree is detached at this commit. */
    detachedHead: boolean;
    /** Paths of other worktrees detached at this commit. */
    detachedWorktrees: string[];
    homeDir: string | null;
  };

  let { labels, detachedHead, detachedWorktrees, homeDir }: Props = $props();

  function title(label: RefLabel): string {
    const parts = [label.kind === "tag" ? `Tag ${label.name}` : label.kind === "remote" ? `Remote branch ${label.name}` : `Branch ${label.name}`];
    if (label.current) parts.push("checked out");
    if (label.remotes.length > 0) parts.push(`same commit on ${label.remotes.join(", ")}`);
    if (label.upstream && (label.ahead > 0 || label.behind > 0)) {
      parts.push(`${label.ahead} ahead, ${label.behind} behind ${label.upstream}`);
    }
    if (label.worktreePath && !label.current) parts.push(`checked out in ${shortenPath(label.worktreePath, homeDir)}`);
    return parts.join(" · ");
  }
</script>

{#if detachedHead}
  <span class="ref head current" data-ref="HEAD" title="HEAD (detached)">
    <svg class="icon" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.7" />
    </svg>
    <span class="name">HEAD</span>
  </span>
{/if}
{#each labels as label}
  <span
    class="ref {label.kind}"
    class:current={label.current}
    data-ref={label.name}
    data-current={label.current}
    title={title(label)}
  >
    <svg class="icon" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      {#if label.current}
        <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.7" />
      {:else if label.kind === "tag"}
        <path d="M1.2 1.2h3.6l4 4-3.6 3.6-4-4z" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" />
        <circle cx="3.4" cy="3.4" r="0.8" fill="currentColor" />
      {:else}
        <circle cx="3" cy="2" r="1.25" fill="currentColor" />
        <circle cx="3" cy="8" r="1.25" fill="currentColor" />
        <circle cx="7.2" cy="3.4" r="1.25" fill="currentColor" />
        <path d="M3 2v6M7.2 3.4c0 2.2-4.2 1.6-4.2 3.8" fill="none" stroke="currentColor" stroke-width="1.1" />
      {/if}
    </svg>
    <span class="name">{label.name}</span>
    {#if label.remotes.length > 0}
      <span class="remotes">{label.remotes.join(", ")}</span>
    {/if}
    {#if label.ahead > 0 || label.behind > 0}
      <span class="ab">
        {#if label.ahead > 0}↑{label.ahead}{/if}{#if label.ahead > 0 && label.behind > 0}&thinsp;{/if}{#if label.behind > 0}↓{label.behind}{/if}
      </span>
    {/if}
    {#if label.worktreePath && !label.current}
      <svg class="wt" width="11" height="10" viewBox="0 0 11 10" aria-label="in a worktree">
        <path d="M1 2.2c0-.5.4-.9.9-.9h2.4l1 1.1h3.8c.5 0 .9.4.9.9v4.6c0 .5-.4.9-.9.9H1.9c-.5 0-.9-.4-.9-.9z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round" />
      </svg>
    {/if}
  </span>
{/each}
{#if detachedWorktrees.length > 0}
  <span
    class="ref worktree"
    title="Worktree detached here: {detachedWorktrees.map((p) => shortenPath(p, homeDir)).join(', ')}"
  >
    <svg class="icon" width="11" height="10" viewBox="0 0 11 10" aria-hidden="true">
      <path d="M1 2.2c0-.5.4-.9.9-.9h2.4l1 1.1h3.8c.5 0 .9.4.9.9v4.6c0 .5-.4.9-.9.9H1.9c-.5 0-.9-.4-.9-.9z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round" />
    </svg>
    {#if detachedWorktrees.length > 1}<span class="name">{detachedWorktrees.length}</span>{/if}
  </span>
{/if}

<style>
  .ref {
    --tint: color-mix(in srgb, var(--c) 13%, var(--bg));
    --ink: color-mix(in srgb, var(--c) 80%, var(--text));
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex: none;
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

  .icon,
  .wt {
    display: block;
    flex: none;
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

  .wt {
    opacity: 0.8;
  }
</style>
