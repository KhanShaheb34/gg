<!--
  Delete a worktree: a plain confirmation of what goes (its folder) and what stays (its
  branch, unless "Also delete branch" is ticked, off to begin with). The core reads the
  worktree again in its turn: if anything of the user's would be lost (changed or untracked
  files, a detached HEAD's commits, an unmerged branch), it asks again, in red, listing it;
  ignored files alone are asked about in the ordinary style. The active worktree hands the
  activity to the main one first.
-->
<script lang="ts">
  import { shortHash, shortenPath, visibleText } from "../../format";
  import { commits, shown } from "../../menus/common";
  import type { WorktreeInfo } from "../../types";
  import Dialog from "../Dialog.svelte";
  import { closeAndRun } from "./run";

  type Props = {
    repoId: number;
    worktree: WorktreeInfo;
    homeDir: string | null;
    close: () => void;
  };

  let { repoId, worktree, homeDir, close }: Props = $props();

  let deleteBranch = $state(false);

  // svelte-ignore state_referenced_locally
  const wt = worktree;
  const folder = wt.path.split(/[\\/]/).filter((p) => p !== "").at(-1) ?? wt.path;
  const work = wt.status === null ? null : wt.status.changed + wt.status.untracked;
  const kept =
    wt.branch === null
      ? "Its commits stay in the repository where a branch or tag has them."
      : `The branch ${shown(wt.branch)} stays, with its commits.`;
</script>

<Dialog
  title="Delete worktree {visibleText(folder)}"
  primary="Delete worktree"
  onSubmit={() => closeAndRun(close, repoId, { op: "worktree_remove", path: wt.path, deleteBranch })}
  onCancel={close}
>
  <div class="dialog-target">
    <span class="dialog-target-label">Folder</span>
    <span class="where selectable path-tail" title={wt.path}><bdi>{shortenPath(wt.path, homeDir)}</bdi></span>
  </div>
  <p class="dialog-text first">The folder is deleted with everything in it. {deleteBranch ? "" : kept}</p>
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
      <input class="checkbox" type="checkbox" bind:checked={deleteBranch} />
      <span>
        Also delete branch <span class="ref">{shown(wt.branch)}</span>
        <span class="check-note">If it has commits not merged or not pushed anywhere, you are asked before they are lost.</span>
      </span>
    </label>
  {:else if wt.head !== null}
    <p class="dialog-text muted">HEAD there is detached at <span class="mono">{shortHash(wt.head)}</span>.</p>
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

  .ref {
    font-weight: 600;
  }
</style>
