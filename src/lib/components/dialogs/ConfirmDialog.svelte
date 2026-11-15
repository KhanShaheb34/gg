<!--
  The confirmation the core asks for before a dangerous operation runs: what it is (the
  operation's title), and each danger it found, in its words, with what would be lost listed
  below it (commits, files). The red button goes ahead; Cancel, which has the focus, does not.
  A round whose dangers lose none of the user's work (ignored files a worktree takes with it,
  stale entries prune forgets: see dangers.ts) is asked in the ordinary style: no red, no
  warning mark; one red danger makes the whole round red.
  For an operation on a repository other than the active tab's, `repository` names it. Asked
  again (`again`: the user confirmed, but what it is about changed before it ran, R39), it
  says so above the dangers, which are as they are now.
-->
<script lang="ts">
  import { confirmationIsRed, isRed } from "../../dangers";
  import { homeInText, visibleText } from "../../format";
  import { app } from "../../state/app.svelte";
  import type { Danger, DangerKind } from "../../types";
  import Dialog from "../Dialog.svelte";

  type Props = {
    title: string;
    dangers: Danger[];
    repository?: string | null;
    again?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  };

  let { title, dangers, repository = null, again = false, onConfirm, onCancel }: Props = $props();

  const VERB: Record<DangerKind, string> = {
    force_push: "Force push",
    hard_reset: "Reset and discard",
    delete_unmerged_branch: "Delete branch",
    delete_remote_branch: "Delete remote branch",
    drop_stash: "Drop stash",
    abort: "Abort",
    skip_discards: "Skip and discard",
    remove_worktree_with_work: "Delete worktree",
    remove_worktree_ignored_files: "Delete worktree",
    delete_unmerged_branch_with_worktree: "Delete branch",
    prune_worktrees: "Prune",
    prune_worktrees_loses_commits: "Prune",
  };

  const primary = $derived(dangers.length === 1 ? VERB[dangers[0].kind] : "Go ahead");
  const red = $derived(confirmationIsRed(dangers));
  /** Paths under the home folder as ~/…, as everywhere else. */
  const home = (text: string) => homeInText(text, app.homeDir);
</script>

<Dialog title={visibleText(home(title))} {primary} danger={red} onSubmit={onConfirm} {onCancel}>
  {#if repository}
    <p class="repository" data-testid="confirm-repository">In <bdi class="name">{visibleText(repository)}</bdi></p>
  {/if}
  {#if again}
    <p class="again" class:red data-testid="confirm-again">This changed since you confirmed it. Here it is as it is now.</p>
  {/if}
  <ul class="dangers" data-testid="dangers">
    {#each dangers as danger (danger.kind)}
      <li class="danger" data-severity={isRed(danger.kind) ? "red" : "ordinary"}>
        {#if isRed(danger.kind)}
          <svg class="mark" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
            <path d="M8 1.8 15 14H1z" fill="currentColor" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" />
            <path d="M8 6v3.6" stroke="var(--bg-elevated)" stroke-width="1.5" stroke-linecap="round" />
            <circle cx="8" cy="11.7" r="0.85" fill="var(--bg-elevated)" />
          </svg>
        {:else}
          <!-- Nothing of the user's is lost: a quiet note mark, not a warning. -->
          <svg class="mark ordinary" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
            <circle cx="8" cy="8" r="6.3" fill="none" stroke="currentColor" stroke-width="1.3" />
            <path d="M8 7.2v3.9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
            <circle cx="8" cy="4.9" r="0.9" fill="currentColor" />
          </svg>
        {/if}
        <div class="text">
          <p class="message selectable"><bdi>{visibleText(home(danger.message))}</bdi></p>
          {#if danger.detail}
            <pre class="detail selectable">{#each danger.detail.replace(/\n+$/, "").split("\n") as line, i (i)}<bdi
                  >{visibleText(home(line))}</bdi
                >{"\n"}{/each}</pre>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
</Dialog>

<style>
  .again {
    margin-bottom: 10px;
    font-weight: 600;
  }

  .again.red {
    color: var(--danger);
  }

  .repository {
    margin-bottom: 10px;
    color: var(--text-muted);
  }

  .repository .name {
    color: var(--text);
    font-weight: 600;
  }

  .dangers {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .danger {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr);
    gap: 10px;
  }

  .mark {
    display: block;
    margin-top: 1px;
    color: var(--danger);
  }

  .mark.ordinary {
    color: var(--text-muted);
  }

  .message {
    line-height: 18px;
    overflow-wrap: anywhere;
  }

  .detail {
    max-height: 168px;
    margin: 8px 0 0;
    padding: 7px 9px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--field-bg);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    line-height: 1.5;
    white-space: pre;
  }
</style>
