<!--
  The banner above the graph while git is stopped in a merge, rebase, cherry-pick or revert
  (spec §5.6), started here or outside the app: what is in progress, the conflicted files
  (each opens in the editor; the list scrolls past six), and Abort, Open in editor and
  Continue. Continue stays disabled, saying why, while a file still has conflict markers;
  it asks first, listing the files it stages. The app never edits the files.

  A quieter form shows conflicts nothing is in progress for (left by applying a stash): the
  files and Open in editor only, and that they need resolving and staging.

  Resolving a file in an editor changes nothing git watches, so when the pointer comes to
  the banner (or to Continue) while files are unresolved, it has the repository looked at
  again (at most once a second). It takes its height from the graph's top; the graph keeps
  its rows where they were.
-->
<script lang="ts">
  import { visibleText } from "../format";
  import { shown } from "../menus/common";
  import { openInEditor } from "../state/editor";
  import { showDialog } from "../state/overlays.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import type { InProgress } from "../types";
  import AbortDialog from "./dialogs/AbortDialog.svelte";
  import ContinueDialog from "./dialogs/ContinueDialog.svelte";

  let { view }: { view: RepoView } = $props();

  const uid = Math.random().toString(36).slice(2, 8);

  const snap = $derived(view.snapshot);
  const progress = $derived(snap.inProgress);
  /** Conflicts with nothing in progress: the quieter form. */
  const leftover = $derived(progress === null && snap.status.conflicted > 0);

  const files = $derived(progress ? progress.conflicted : snap.status.conflictedPaths);
  const unresolved = $derived(new Set(progress ? progress.unresolved : snap.status.conflictedPaths));
  /** Files the status counts beyond those it names. */
  const unnamed = $derived(leftover ? Math.max(0, snap.status.conflicted - files.length) : 0);
  const left = $derived(progress ? progress.unresolved.length : 0);

  const files_ = (n: number) => `${n} file${n === 1 ? "" : "s"}`;
  const has = (n: number) => `${files_(n)} ${n === 1 ? "has" : "have"} conflicts`;

  function headline(p: InProgress | null, branch: string | null): string {
    if (p === null) return has(snap.status.conflicted);
    const on = branch === null ? null : shown(branch);
    switch (p.kind) {
      case "merge":
        return on ? `Merging into ${on}` : "Merging";
      case "rebase":
        return p.step !== null && p.total !== null ? `Rebasing, step ${p.step} of ${p.total}` : "Rebasing";
      case "cherry-pick":
        return on ? `Cherry-picking onto ${on}` : "Cherry-picking";
      case "revert":
        return on ? `Reverting on ${on}` : "Reverting";
    }
  }

  const title = $derived(headline(progress, snap.head.branch));

  const state = $derived.by(() => {
    if (progress === null) {
      return "Left by applying a stash or an earlier step. Resolve them in your editor, then stage them: until then nothing that changes the files can run.";
    }
    const n = progress.conflicted.length;
    if (n === 0) return "Nothing is conflicted: ready to continue.";
    if (left === 0) return "All conflicts are resolved: continue stages the files and goes on.";
    if (left === n) return `${has(n)}. Resolve them in your editor, then continue.`;
    return `${n - left} of ${files_(n)} resolved. Resolve the rest in your editor, then continue.`;
  });

  const continueReason = $derived(left > 0 ? `Resolve the conflicts in ${files_(left)} first.` : null);

  /** The user comes back to the banner: look again if a file may have been resolved meanwhile. */
  function recheck() {
    if (left > 0) view.recheck();
  }

  function abort() {
    if (progress) showDialog(AbortDialog, { repoId: view.repoId, kind: progress.kind, branch: snap.head.branch });
  }

  function continueOp() {
    if (progress && left === 0) {
      showDialog(ContinueDialog, { repoId: view.repoId, kind: progress.kind, files: progress.conflicted });
    }
  }
</script>

{#if progress !== null || leftover}
  <section
    class="banner"
    class:leftover
    class:ready={progress !== null && left === 0}
    data-testid="conflict-banner"
    aria-labelledby="banner-title-{uid}"
    onpointerenter={recheck}
  >
    <div class="head">
      <span class="mark" aria-hidden="true"></span>
      <div class="text">
        <h2 class="title" id="banner-title-{uid}">{title}</h2>
        {#if progress?.subject}
          <p class="subject" title={visibleText(progress.subject)}>
            <bdi>{visibleText(progress.subject)}</bdi>
          </p>
        {/if}
        <p class="state" class:ready={progress !== null && left === 0}>{state}</p>
      </div>
      <div class="actions">
        {#if progress !== null}
          <button class="button" onclick={abort}>Abort…</button>
        {/if}
        <button class="button" onclick={() => void openInEditor(view.repoId, null)}>Open in editor</button>
        {#if progress !== null}
          <span class="continue-wrap" role="presentation" onpointerenter={recheck} title={continueReason ?? undefined}>
            <button
              class="button primary"
              disabled={continueReason !== null}
              aria-describedby={continueReason ? `continue-why-${uid}` : undefined}
              onfocus={recheck}
              onclick={continueOp}>Continue…</button
            >
          </span>
          {#if continueReason}
            <span class="visually-hidden" id="continue-why-{uid}">{continueReason}</span>
          {/if}
        {/if}
      </div>
    </div>
    {#if files.length > 0}
      <ul class="files" aria-label="Conflicted files">
        {#each files as file (file)}
          {@const open = unresolved.has(file)}
          <li class="file-row">
            <span class="icon" class:open aria-hidden="true">
              {#if open}
                <svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1.5 11 10.5H1Z" /></svg>
              {:else}
                <svg viewBox="0 0 12 12" width="12" height="12"><path d="M2.5 6.2 5 8.6 9.6 3.6" /></svg>
              {/if}
            </span>
            <button class="file path-tail" title="Open {visibleText(file)} in the editor" onclick={() => void openInEditor(view.repoId, file)}
              ><bdi>{visibleText(file)}</bdi></button
            >
            {#if progress !== null}
              <span class="file-state" class:open>{open ? "Conflicted" : "Resolved"}</span>
            {/if}
          </li>
        {/each}
        {#if unnamed > 0}
          <li class="file-row more">and {files_(unnamed)} more</li>
        {/if}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .banner {
    --tone: var(--warning);
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 14px 10px 12px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--tone) 6%, var(--bg));
    box-shadow: inset 2px 0 0 color-mix(in srgb, var(--tone) 85%, var(--bg));
  }

  /* Nothing left to resolve: calm, the way on. */
  .banner.ready {
    --tone: var(--accent);
  }

  .banner.leftover {
    --tone: var(--text-faint);
    background: color-mix(in srgb, var(--text-faint) 5%, var(--bg));
  }

  .head {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    min-width: 0;
  }

  .mark {
    flex: none;
    width: 8px;
    height: 8px;
    margin: 5px 0 0 2px;
    border-radius: 50%;
    background: var(--tone);
  }

  .text {
    flex: 1;
    min-width: 0;
  }

  .title {
    font-size: var(--fs);
    font-weight: 650;
    line-height: 18px;
  }

  .subject {
    overflow: hidden;
    color: var(--text);
    font-size: var(--fs-sm);
    line-height: 17px;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .state {
    margin-top: 1px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 17px;
  }

  .state.ready {
    color: var(--success);
  }

  .actions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .continue-wrap {
    display: inline-flex;
  }

  .button {
    height: 26px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 500;
    white-space: nowrap;
    transition:
      background-color 80ms linear,
      border-color 80ms linear;
  }

  .button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--text) 6%, var(--bg-elevated));
  }

  .button:active:not(:disabled) {
    background: color-mix(in srgb, var(--text) 10%, var(--bg-elevated));
  }

  .button:focus-visible,
  .file:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .primary:hover:not(:disabled) {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .primary:disabled {
    border-color: var(--border-strong);
    background: color-mix(in srgb, var(--text) 5%, var(--bg));
    color: var(--text-faint);
    cursor: default;
  }

  .files {
    /* Six rows show, and half of the next when there are more: the list scrolls. */
    max-height: calc(6.5 * 22px + 4px);
    overflow-y: auto;
    margin: 0 0 0 20px;
    padding: 4px 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg);
    list-style: none;
    max-width: 640px;
    overscroll-behavior: contain;
  }

  .file-row {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 22px;
    padding: 0 10px 0 9px;
  }

  .file-row.more {
    color: var(--text-muted);
    font-size: var(--fs-sm);
    padding-left: 29px;
  }

  .icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 12px;
    color: var(--success);
  }

  .icon svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .icon.open {
    color: var(--warning);
  }

  .leftover .icon.open {
    color: var(--text-muted);
  }

  .file {
    flex: 0 1 auto;
    min-width: 0;
    padding: 0 2px;
    border: none;
    border-radius: 3px;
    background: none;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    line-height: 18px;
    cursor: pointer;
  }

  .file:hover {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .file-state {
    flex: none;
    margin-left: auto;
    padding-left: 12px;
    color: var(--success);
    font-size: var(--fs-xs);
  }

  .file-state.open {
    color: var(--text-muted);
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
