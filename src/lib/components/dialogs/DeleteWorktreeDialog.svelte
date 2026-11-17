<!--
  Delete a worktree: a plain confirmation of what goes (its folder) and what stays (its
  branch, unless "Also delete branch" is ticked, off to begin with), saying up front what is
  unusual there (a merge or rebase stopped in it, a detached HEAD, uncommitted changes). The
  core reads the worktree again in its turn: if anything of the user's would be lost (changed
  or untracked files, a detached HEAD's commits, an unmerged branch), it asks again, in red,
  listing it, in a confirmation of its own; ignored files alone are asked about here, in this
  dialog, listed (one dialog, not two in a row). The active worktree hands the activity to the
  main one first.
-->
<script lang="ts">
  import { confirmationIsRed } from "../../dangers";
  import { homeInText, shortHash, shortenPath, visibleText } from "../../format";
  import { commits, shown } from "../../menus/common";
  import { app } from "../../state/app.svelte";
  import { runOp } from "../../state/ops.svelte";
  import type { Danger, OpOutcome, WorktreeInfo } from "../../types";
  import Dialog from "../Dialog.svelte";

  type Props = {
    repoId: number;
    worktree: WorktreeInfo;
    homeDir: string | null;
    close: () => void;
  };

  let { repoId, worktree, homeDir, close }: Props = $props();

  let deleteBranch = $state(false);
  /** Sent: waiting for the core. */
  let pending = $state(false);
  /** The core asks about ignored files: they are listed here, and Delete goes ahead. */
  let asking = $state<{ dangers: Danger[]; again: boolean; answer: (yes: boolean) => void } | null>(null);
  let open = true;

  // svelte-ignore state_referenced_locally
  const wt = worktree;
  const folder = wt.path.split(/[\\/]/).filter((p) => p !== "").at(-1) ?? wt.path;
  const work = wt.status === null ? null : wt.status.changed + wt.status.untracked;
  const kept =
    wt.branch === null
      ? "Its commits stay in the repository where a branch or tag has them."
      : `The branch ${shown(wt.branch)} stays, with its commits.`;
  const STOPPED = { merge: "A merge", rebase: "A rebase", "cherry-pick": "A cherry-pick", revert: "A revert" } as const;

  function finish() {
    open = false;
    close();
  }

  /** Ignored files alone are asked here; anything red in a confirmation of its own. */
  function confirmHere(outcome: OpOutcome, again: boolean): Promise<boolean> | null {
    if (!open || confirmationIsRed(outcome.dangers)) {
      finish();
      return null;
    }
    return new Promise((answer) => {
      asking = { dangers: outcome.dangers, again, answer };
      pending = false;
    });
  }

  async function submit() {
    if (asking) {
      const { answer } = asking;
      asking = null;
      pending = true;
      answer(true);
      return;
    }
    pending = true;
    try {
      // What the dialog showed (final review M6): another worktree at that path by now, or
      // another branch there, is not deleted.
      const expectHead = { branch: wt.branch, hash: wt.head };
      await runOp(repoId, { op: "worktree_remove", path: wt.path, deleteBranch, expectHead }, { confirmWith: confirmHere });
    } catch {
      // Reported by runOp.
    }
    if (open) finish();
  }

  function cancel() {
    if (pending) return;
    asking?.answer(false);
    asking = null;
    finish();
  }
</script>

<Dialog
  title="Delete worktree {visibleText(folder)}"
  primary={pending ? "Deleting…" : "Delete worktree"}
  valid={!pending}
  once={false}
  cancellable={!pending}
  onSubmit={() => void submit()}
  onCancel={cancel}
>
  <div class="dialog-target">
    <span class="dialog-target-label">Folder</span>
    <span class="where selectable path-tail" title={wt.path}><bdi>{shortenPath(wt.path, homeDir)}</bdi></span>
  </div>
  {#if wt.inProgress}
    <p class="dialog-text first stopped" data-testid="delete-in-progress">
      {STOPPED[wt.inProgress]} is stopped there; deleting the worktree abandons it.
    </p>
  {/if}
  {#if wt.branch === null && wt.head !== null}
    <p class="dialog-text" class:first={!wt.inProgress}>
      HEAD there is detached at <span class="mono">{shortHash(wt.head)}</span>: no branch keeps what is checked out.
    </p>
  {/if}
  <p class="dialog-text" class:first={!wt.inProgress && !(wt.branch === null && wt.head !== null)}>
    The folder is deleted with everything in it. {deleteBranch ? "" : kept}
  </p>
  {#if wt.active}
    <p class="dialog-text muted">It is the active worktree: the main worktree becomes active first.</p>
  {/if}
  {#if work !== null && work > 0}
    <p class="dialog-text muted" data-testid="delete-work-note">
      It has {work} uncommitted change{work === 1 ? "" : "s"}: you are asked again, with the list, before {work === 1
        ? "it is"
        : "they are"} lost.
    </p>
  {:else if wt.branch !== null && !deleteBranch && (wt.unpushed ?? 0) > 0 && wt.upstream === null}
    <p class="dialog-text muted">{commits(wt.unpushed!)} on no remote; the branch keeps them.</p>
  {/if}
  {#if wt.branch !== null}
    <label class="check-row">
      <input class="checkbox" type="checkbox" bind:checked={deleteBranch} disabled={pending || asking !== null} />
      <span>
        Also delete branch <span class="ref">{shown(wt.branch)}</span>
        <span class="check-note">If it has commits not merged or not pushed anywhere, you are asked before they are lost.</span>
      </span>
    </label>
  {/if}
  {#if asking}
    <!-- What the core found: files git deletes with the folder, none of them the user's work. -->
    <div class="asked" data-testid="dangers">
      {#if asking.again}
        <p class="again" data-testid="confirm-again">This changed since you confirmed it. Here it is as it is now.</p>
      {/if}
      {#each asking.dangers as danger (danger.kind)}
        <div class="danger" data-severity="ordinary">
          <p class="message selectable"><bdi>{visibleText(homeInText(danger.message, app.homeDir))}</bdi></p>
          {#if danger.detail}
            <pre class="detail selectable">{#each danger.detail.replace(/\n+$/, "").split("\n") as line, i (i)}<bdi
                  >{visibleText(homeInText(line, app.homeDir))}</bdi
                >{"\n"}{/each}</pre>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</Dialog>

<style>
  .where {
    min-width: 0;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .first {
    margin-top: 12px;
  }

  .stopped {
    color: var(--warning);
    font-weight: 550;
  }

  .ref {
    font-weight: 600;
  }

  .asked {
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }

  .again {
    margin-bottom: 8px;
    font-weight: 600;
  }

  .message {
    line-height: 18px;
    overflow-wrap: anywhere;
  }

  .detail {
    max-height: 150px;
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
