<!--
  The banner above the graph while git is stopped in a merge, rebase, cherry-pick or revert
  (spec §5.6), started here or outside the app: what is in progress ("Merging feature/x into
  main", "Rebasing topic onto main, step 2 of 5"), the conflicted files with how far each is
  resolved (each opens in the editor; the list scrolls, fading at its bottom edge), and Abort,
  Open in editor and Continue. Continue is enabled once every file is resolved; until then it
  says why, and a quieter "Continue anyway…" asks first, naming the files that still have
  markers and those git kept one side of (R31). When the commit being applied has no changes
  left (the core's `empty`: continuing would be refused), the banner says so and offers Skip
  (that commit) in place of Continue (R40). The app never edits the files.

  A quieter form shows conflicts nothing is in progress for (left by applying a stash): the
  files and Open in editor only, and what clears them.

  Resolving a file in an editor changes nothing git watches, so when the pointer or the focus
  comes to the banner, it has the repository looked at again (at most once a second). A
  polite status announces the headline when the banner appears or what it shows changes
  (another kind, another step), never on a recheck. It takes its height from the graph's
  top; the graph keeps its rows where they were.
-->
<script lang="ts">
  import { repeatsMergeName, visibleText } from "../format";
  import { shown } from "../menus/common";
  import { openInEditor } from "../state/editor";
  import { runOp } from "../state/ops.svelte";
  import { lazyDialog, showDialog } from "../state/overlays.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import type { ConflictFile, InProgress } from "../types";
  const ContinueDialog = lazyDialog(() => import("./dialogs/ContinueDialog.svelte"));

  /** `compact`: a panel is open below the graph; the file list shows about three rows. */
  let { view, compact = false }: { view: RepoView; compact?: boolean } = $props();

  const uid = Math.random().toString(36).slice(2, 8);

  const snap = $derived(view.snapshot);
  const progress = $derived(snap.inProgress);
  /** Conflicts with nothing in progress: the quieter form. */
  const leftover = $derived(progress === null && snap.status.conflicted > 0);

  const files = $derived<ConflictFile[]>(
    progress ? progress.files : snap.status.conflictedPaths.map((path) => ({ path, state: "markers" as const })),
  );
  const total = $derived(progress ? progress.fileCount : snap.status.conflicted);
  /** Files counted beyond those named. */
  const unnamed = $derived(Math.max(0, total - files.length));
  const resolved = $derived(files.filter((f) => f.state === "resolved").length);
  /** Every file is resolved (and every one was looked at). */
  const ready = $derived(progress !== null && unnamed === 0 && resolved === files.length);
  /** The commit being applied has no changes left: the way on is to skip it (R40). */
  const empty = $derived(progress !== null && progress.empty && progress.commit !== null);

  const files_ = (n: number) => `${n} file${n === 1 ? "" : "s"}`;
  const has = (n: number) => `${files_(n)} ${n === 1 ? "has" : "have"} conflicts`;

  function headline(p: InProgress | null, branch: string | null): string {
    if (p === null) return has(snap.status.conflicted);
    const on = branch === null ? null : shown(branch);
    const what = p.incoming === null ? null : shown(p.incoming);
    switch (p.kind) {
      case "merge":
        if (what && on) return `Merging ${what} into ${on}`;
        return what ? `Merging ${what}` : on ? `Merging into ${on}` : "Merging";
      case "rebase": {
        const who = p.rebasing === null ? null : shown(p.rebasing);
        const head = who && what ? `Rebasing ${who} onto ${what}` : what ? `Rebasing onto ${what}` : "Rebasing";
        return p.step !== null && p.total !== null ? `${head}, step ${p.step} of ${p.total}` : head;
      }
      case "cherry-pick":
        return ["Cherry-picking", what, on && `onto ${on}`].filter(Boolean).join(" ");
      case "revert":
        return ["Reverting", what, on && `on ${on}`].filter(Boolean).join(" ");
    }
  }

  const title = $derived(headline(progress, snap.head.branch));

  /**
   * The line under the headline: the commit being applied. A merge's message only repeats
   * what the headline says ("Merge branch 'feature/x'") once it names what comes in.
   */
  const subject = $derived.by(() => {
    if (!progress?.subject) return null;
    if (progress.kind === "merge" && progress.incoming !== null && repeatsMergeName(progress.subject, progress.incoming)) return null;
    return progress.subject;
  });

  const stateLine = $derived.by(() => {
    if (progress === null) {
      const one = snap.status.conflicted === 1;
      return `Left by applying a stash or an earlier step. Resolve ${one ? "it" : "them"} in your editor. The banner goes once git sees no conflict: stage the ${one ? "file" : "files"} (git add), or reset to the current commit (mixed).`;
    }
    const n = progress.fileCount;
    if (empty) return `This commit has no changes left (they are already here): skip it, or abort the ${progress.kind}.`;
    if (n === 0) return "Nothing is conflicted: ready to continue.";
    if (ready) return "All conflicts are resolved: continue stages the files and goes on.";
    if (resolved === 0) return `${has(n)}. Resolve ${n === 1 ? "it" : "them"} in your editor, then continue.`;
    return `${resolved} of ${files_(n)} resolved. Resolve the rest in your editor, then continue.`;
  });

  const continueReason = $derived(ready ? null : `${files_(total - resolved)} not resolved yet.`);

  const STATE_WORDS: Record<ConflictFile["state"], string> = {
    markers: "Conflict markers",
    undecided: "Check this file",
    resolved: "Resolved",
  };
  const STATE_TIPS: Record<ConflictFile["state"], string> = {
    markers: "The file still has conflict markers",
    undecided:
      "No conflict markers to go by: binary, deleted or added on one side (git kept one side), or too large to read here. Only you can say it is right.",
    resolved: "No conflict markers left",
  };

  /** The user comes back to the banner: look again if a file may have been resolved meanwhile. */
  function recheck() {
    if (progress !== null && !ready) view.recheck();
  }

  /** The core asks to confirm, saying what goes back (R39): no dialog of its own. */
  function abort() {
    if (progress) runOp(view.repoId, { op: "abort", expectHead: snap.head, expectWorktree: snap.activeWorktree }).catch(() => {}); // reported by runOp
  }

  /** Leaves out the commit the banner shows as empty; the core checks git is still stopped on it. */
  function skip() {
    if (progress?.commit) {
      runOp(view.repoId, { op: "skip", expectHead: snap.head, commit: progress.commit, expectWorktree: snap.activeWorktree }).catch(() => {}); // reported by runOp
    }
  }

  function continueOp(force: boolean) {
    if (!progress || (!force && !ready)) return;
    showDialog(ContinueDialog, {
      repoId: view.repoId,
      kind: progress.kind,
      files: progress.files,
      fileCount: progress.fileCount,
      force,
      head: snap.head,
      worktree: snap.activeWorktree,
    });
  }

  // --- What is announced -------------------------------------------------------------------

  /** The text of the polite status: the headline, set only when the stop is new or moved on. */
  let announced = $state("");
  let announcedKey = "";
  $effect(() => {
    const key = progress === null ? (leftover ? "leftover" : "") : `${progress.kind}:${progress.step ?? ""}`;
    if (key === announcedKey) return;
    announcedKey = key;
    announced = key === "" ? "" : title;
  });

  // --- The list's fade -------------------------------------------------------------------------

  let list: HTMLElement | undefined = $state();
  /** The list has more below what shows. */
  let more = $state(false);

  function checkMore() {
    if (list) more = list.scrollHeight - list.scrollTop - list.clientHeight > 1;
  }

  $effect(() => {
    void files;
    if (!list) return;
    checkMore();
    const observer = new ResizeObserver(checkMore);
    observer.observe(list);
    return () => observer.disconnect();
  });
</script>

<span class="visually-hidden" role="status" data-testid="banner-announce">{announced}</span>

{#if progress !== null || leftover}
  <section
    class="banner"
    class:leftover
    class:ready
    data-testid="conflict-banner"
    aria-labelledby="banner-title-{uid}"
    onpointerenter={recheck}
    onfocusin={recheck}
  >
    <div class="head">
      <span class="mark" aria-hidden="true"></span>
      <div class="text">
        <h2 class="title" id="banner-title-{uid}"><bdi>{visibleText(title)}</bdi></h2>
        {#if subject}
          <p class="subject" title={visibleText(subject)}>
            <bdi>{visibleText(subject)}</bdi>
          </p>
        {/if}
        <p class="state" class:ready>{stateLine}</p>
      </div>
      <div class="actions">
        {#if progress !== null}
          <button class="button" onclick={abort}>Abort…</button>
        {/if}
        <button class="button" onclick={() => void openInEditor(view.repoId, null)}>Open in editor</button>
        {#if empty}
          <button class="button primary" onclick={skip}>Skip this commit</button>
        {:else if progress !== null}
          {#if !ready}
            <button class="button quiet" onclick={() => continueOp(true)}>Continue anyway…</button>
          {/if}
          <span class="continue-wrap" role="presentation" onpointerenter={recheck} title={continueReason ?? undefined}>
            <!-- aria-disabled, not disabled: it stays reachable, and its reason is read out. -->
            <button
              class="button primary"
              aria-disabled={continueReason !== null}
              aria-describedby={continueReason ? `continue-why-${uid}` : undefined}
              onclick={() => continueOp(false)}>Continue…</button
            >
          </span>
          {#if continueReason}
            <span class="visually-hidden" id="continue-why-{uid}">{continueReason}</span>
          {/if}
        {/if}
      </div>
    </div>
    {#if files.length > 0}
      <div class="files-wrap" class:more class:compact>
        <ul class="files" aria-label="Conflicted files" bind:this={list} onscroll={checkMore}>
          {#each files as file (file.path)}
            <li class="file-row">
              <span class="icon {file.state}" aria-hidden="true">
                {#if file.state === "resolved"}
                  <svg viewBox="0 0 12 12" width="12" height="12"><path d="M2.5 6.2 5 8.6 9.6 3.6" /></svg>
                {:else}
                  <svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1.5 11 10.5H1Z" /></svg>
                {/if}
              </span>
              <button
                class="file path-tail"
                title="Open {visibleText(file.path)} in the editor"
                onclick={() => void openInEditor(view.repoId, file.path)}><bdi>{visibleText(file.path)}</bdi></button
              >
              {#if progress !== null}
                <span class="file-state {file.state}" title={STATE_TIPS[file.state]}>{STATE_WORDS[file.state]}</span>
              {/if}
            </li>
          {/each}
          {#if unnamed > 0}
            <li class="file-row more-row">and {files_(unnamed)} more</li>
          {/if}
        </ul>
      </div>
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
    overflow: hidden;
    font-size: var(--fs);
    font-weight: 650;
    line-height: 18px;
    white-space: nowrap;
    text-overflow: ellipsis;
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

  .button:hover:not([aria-disabled="true"]) {
    background: color-mix(in srgb, var(--text) 6%, var(--bg-elevated));
  }

  .button:active:not([aria-disabled="true"]) {
    background: color-mix(in srgb, var(--text) 10%, var(--bg-elevated));
  }

  .button:focus-visible,
  .file:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  /* A way past the check, not the way on: no frame until the pointer comes. */
  .button.quiet {
    border-color: transparent;
    background: transparent;
    color: var(--text-muted);
  }

  .button.quiet:hover {
    color: var(--text);
  }

  .primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .primary:hover:not([aria-disabled="true"]) {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .primary[aria-disabled="true"] {
    border-color: var(--border-strong);
    background: color-mix(in srgb, var(--text) 5%, var(--bg));
    color: var(--text-faint);
    cursor: default;
  }

  .files-wrap {
    align-self: flex-start;
    min-width: min(360px, 100%);
    max-width: min(640px, 100%);
    margin-left: 20px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg);
  }

  .files {
    /* Six rows and a bit; past that it scrolls, fading at the bottom edge (never more than a quarter of the window). */
    max-height: min(151px, 25vh);
    overflow-y: auto;
    padding: 4px 0;
    list-style: none;
    overscroll-behavior: contain;
  }

  /* With a panel open below the graph: three rows and a bit. */
  .files-wrap.compact .files {
    max-height: 85px;
  }

  .files-wrap.more .files {
    -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 16px), transparent);
    mask-image: linear-gradient(to bottom, #000 calc(100% - 16px), transparent);
  }

  .file-row {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 22px;
    padding: 0 12px 0 9px;
  }

  .file-row.more-row {
    color: var(--text-muted);
    font-size: var(--fs-sm);
    padding-left: 29px;
  }

  .icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 12px;
    color: var(--warning);
  }

  .icon svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .icon.resolved {
    color: var(--success);
  }

  .leftover .icon {
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

  /* Right after the name, so the eye does not travel to find it. */
  .file-state {
    flex: none;
    padding-left: 4px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }

  .file-state.resolved {
    color: var(--success);
  }

  .file-state.undecided {
    color: var(--warning);
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
