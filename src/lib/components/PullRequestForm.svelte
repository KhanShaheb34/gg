<!--
  The form that makes a pull request of a local branch, in the side panel's Pull request tab:
  a title and a description (suggested by the core's `pr_suggest`: the branch's name as words, its commits;
  a suggestion that comes late never replaces what the user typed), the base among the base
  repository's branches (the one last used there to begin with), and Create as draft. The
  primary button names the push it makes first when one is needed, and a line above it says
  where to. What the form holds is kept per repository and branch while the app runs, shown
  or not (pullRequest.svelte.ts), with the request out for it.

  Without gh, or signed out (`compare`), there is no form: the branch is pushed and GitHub's
  compare page opens to finish there; the tab says so and how to fix gh. Where it pushes,
  whether it must, and the base repository come from the core (`pr_suggest`), worked out as
  `pr_create` will.

  While the request is out, the fields are read-only and the step is said (pushing, creating).
  A failure stays here, inline, with Show log (and the compare page with the form's text);
  success leaves a notice with Open and Copy link, and the tab shows the pull request.
-->
<script lang="ts">
  import { visibleText } from "../format";
  import { CoreError, onEvent } from "../ipc";
  import { defaultRemote, PR_REASONS, shown } from "../menus/common";
  import { isMac } from "../platform";
  import { notify, showLog } from "../state/app.svelte";
  import { openExternal } from "../state/editor";
  import { runOp } from "../state/ops.svelte";
  import { copyLink } from "../state/prs.svelte";
  import { untrack } from "svelte";
  import { applySuggestion, defaultBranch, dropPrDraft, prDraft, prNumberOf, rememberMade, type PrFields } from "../state/pullRequest.svelte";
  import type { RepoView } from "../state/repo.svelte";
  import { cancelGeneration, generate, newRequestId } from "../state/ai.svelte";
  import type { OpOutcome, OpProgress, PrSuggestion } from "../types";
  import AiGenerate from "./AiGenerate.svelte";
  import AiProblem from "./AiProblem.svelte";
  import MiddleText from "./MiddleText.svelte";

  type Props = {
    view: RepoView;
    /** The local branch the pull request is made from. */
    branch: string;
    /** What the core suggested for it (null while it is asked). */
    suggestion: PrSuggestion | null;
    /** Why the suggestion could not be had, with Try again. */
    suggestFailed: string | null;
    onRetry: () => void;
    /** gh cannot make it here (not installed, signed out): the compare page instead of the form. */
    compare: "no_gh" | "signed_out" | null;
    /** Asks the core again where the branch goes and whether it must be pushed. */
    onPushPlan: () => void;
  };

  let { view, branch, suggestion, suggestFailed, onRetry, compare, onPushPlan }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  const SUBMIT_KEYS = isMac() ? "⌘↩" : "Ctrl+Enter";
  const repoId = $derived(view.repoId);
  const snap = $derived(view.snapshot);
  const d = $derived(prDraft(snap.rootPath, branch));

  /** A first guess at the base repository's remote until the suggestion says. */
  const guessRemote = $derived(defaultRemote(snap.remotes) || snap.remotes[0] || "origin");
  /**
   * The remote of the repository the pull request goes into: the base repository the core
   * chose (the same rule as the list's). Only its branches can be the base, so changing the
   * base never changes the repository.
   */
  const baseRemote = $derived(suggestion ? suggestion.baseRemote : guessRemote);

  /** The base repository's branches a pull request can go into ("main"), without its HEAD. */
  const bases = $derived.by(() => {
    if (baseRemote === null) return [];
    const prefix = `${baseRemote}/`;
    const list = snap.remoteBranches.filter((b) => b.startsWith(prefix) && b !== `${prefix}HEAD`).map((b) => b.slice(prefix.length));
    if (d.base !== "" && !list.includes(d.base)) list.unshift(d.base);
    return list;
  });

  // A first guess until the suggestion comes: the default remote's default branch, if known.
  $effect(() => {
    if (d.base !== "" || d.baseTouched || suggestion !== null) return;
    const guess = defaultBranch(snap, guessRemote);
    if (guess !== null) {
      d.base = guess;
      d.baseRemote = guessRemote;
    }
  });

  // The suggestion fills what the user has not changed, whenever one comes.
  let applied: PrSuggestion | null = null;
  $effect(() => {
    const s = suggestion;
    if (s === null || s === applied) return;
    applied = s;
    const draft = d;
    untrack(() => applySuggestion(draft, s));
  });

  const loading = $derived(suggestion === null && suggestFailed === null);
  const compareUrl = $derived(suggestion?.compareUrl ?? null);

  // --- The push it makes first, as the core will make it ----------------------------

  /** Where the branch is pushed ("fork/x"), as `pr_create` will push it; null until known. */
  const pushTo = $derived(suggestion?.pushTo ?? null);
  const tipOf = (list: string[], tips: { hash: string }[], name: string) => tips[list.indexOf(name)]?.hash ?? null;
  /**
   * It is pushed first: its namesake there is missing or on another commit, as the repository
   * shows them now, whatever the core said when it suggested (a commit made since, or a push
   * made since: by a try that then failed).
   */
  const needsPush = $derived.by(() => {
    if (pushTo === null) return false;
    const here = tipOf(snap.branches, snap.branchTips, branch);
    // Not known here: as the core said.
    if (here === null) return suggestion?.needsPush ?? false;
    // Its namesake there missing, or on another commit (a commit made since the core said it
    // was level, say): pushed first. Level: nothing to push (pushed since the core said).
    return here !== tipOf(snap.remoteBranches, snap.remoteBranchTips, pushTo);
  });
  /** Why it is pushed: not there at all, or there but behind. */
  const pushWhy = $derived(
    pushTo === null
      ? null
      : snap.remoteBranches.includes(pushTo)
        ? `${pushTo} is behind ${branch}: it is pushed there first`
        : `${branch} is not on ${pushTo} yet: it is pushed there first`,
  );

  // --- Sending ----------------------------------------------------------------------------

  const titleProblem = $derived.by(() => {
    if (d.title.trim() === "") return "A pull request needs a title.";
    if (/[\r\n]/.test(d.title)) return "The title is one line: put the rest in the description.";
    return null;
  });

  const primary = $derived.by(() => {
    if (d.pending) return d.step?.startsWith("Pushing") || (d.step === null && needsPush) ? "Pushing…" : compare ? "Pushing…" : "Creating…";
    if (compare) return needsPush ? "Push and open the compare page" : "Open the compare page";
    return needsPush ? "Push and create PR" : "Create PR";
  });

  /** Why the button cannot be pressed, said under it; null when it can. */
  const reason = $derived.by(() => {
    if (d.pending || loading || suggestFailed !== null) return null;
    if (baseRemote === null) return "This repository has no remote to open a pull request on.";
    if (pushTo === null) return `It is not clear which remote to push ${shown(branch)} to: push it from its menu first.`;
    // Without gh only the push is made (its title is not used).
    if (compare) return needsPush || compareUrl !== null ? null : "GitHub’s compare page is not known for this repository.";
    if (titleProblem !== null) return d.titleTouched || d.title !== "" ? titleProblem : "Write a title.";
    if (d.base === "") return "Choose the branch it goes into.";
    return null;
  });
  /** A "Generate with AI" under way (its request, kept with the draft), or null: the form waits for it. */
  const writing = $derived(d.ai.request);
  const valid = $derived(!d.pending && !loading && suggestFailed === null && reason === null && writing === null);

  function openUrl(url: string) {
    void openExternal(repoId, { target: "url", url });
  }

  /** Says how it went, with the page it made (or the compare page) to open or copy. */
  function done(outcome: OpOutcome) {
    const hint = outcome.hints.find((h) => h.kind === "open_url");
    const url = hint?.kind === "open_url" ? hint.url : null;
    // Without gh (or signed out) it ends on the compare page: opened, as the tab said.
    if (url !== null && outcome.status === "done" && url.includes("/compare/")) openUrl(url);
    const actions =
      url === null
        ? []
        : [
            { label: "Open", run: () => openUrl(url) },
            { label: "Copy link", keepOpen: true, run: () => copyLink(url) },
          ];
    notify({
      kind: "info",
      message: outcome.message,
      actions,
      persistent: actions.length > 0,
      until: actions.length > 0 ? { repoId, opId: outcome.opId } : undefined,
    });
    return url;
  }

  async function create() {
    const remote = baseRemote;
    if (remote === null) return;
    // What is sent is what the form shows now; the form may be gone by the time it is answered
    // (another tab shown): the answer still lands, in the kept draft.
    const draft = d;
    const root = snap.rootPath;
    const sent = { title: draft.title.trim() || branch, body: draft.body, base: draft.base, draft: draft.draft };
    const asCompare = compare !== null;
    draft.failure = null;
    draft.refused = null;
    draft.step = null;
    draft.progress = null;
    draft.pending = true;
    const stopListening = onEvent<OpProgress>("op-progress", (p) => {
      if (p.repoId !== repoId) return;
      if (p.line.endsWith("…") && (p.line.startsWith("Pushing ") || p.line.startsWith("Creating "))) {
        draft.step = p.line;
        draft.progress = null;
      } else {
        draft.progress = p.line;
      }
    });
    try {
      const outcome = await runOp(
        repoId,
        {
          op: "pr_create",
          branch,
          base: sent.base,
          baseRemote: remote,
          title: sent.title,
          body: sent.body,
          draft: sent.draft,
          // What the tab shows is what is done: the compare page it promised never turns into a
          // pull request made through gh.
          comparePage: asCompare,
        },
        { ownErrors: ["bad_request", "bad_name"], ownOutcome: true },
      );
      if (outcome.status === "failed") {
        draft.failure = outcome;
        // The push may have been made before gh failed: the button says what is left to do.
        onPushPlan();
        return;
      }
      const url = done(outcome);
      if (asCompare || (url !== null && url.includes("/compare/"))) return;
      // Made (or there already): shown at once, until the list the core reloads has it.
      const number = url === null ? null : prNumberOf(url);
      if (url !== null && number !== null) {
        rememberMade(outcome.snapshot, branch, {
          number,
          title: sent.title,
          url,
          state: sent.draft ? "draft" : "open",
          head: branch,
          base: sent.base,
        });
      }
      dropPrDraft(root, branch);
    } catch (e) {
      if (e instanceof CoreError && (e.code === "bad_request" || e.code === "bad_name")) draft.refused = e.message;
      // Anything else was reported by runOp.
    } finally {
      stopListening();
      draft.pending = false;
    }
  }

  function submit(e?: Event) {
    e?.preventDefault();
    if (!valid) return;
    if (compare && !needsPush) {
      if (compareUrl !== null) openUrl(compareUrl);
      return;
    }
    void create();
  }

  // --- Generate with AI -------------------------------------------------------------------

  // How it failed and what it replaced are the draft's too (see `PrDraft.ai`).
  const aiFailure = $derived(d.ai.failure);
  const undo = $derived(d.ai.undo);
  let aiControl: AiGenerate | undefined = $state();

  /** Why there is nothing to write from now, or null. */
  const aiBlocked = $derived.by((): string | null => {
    if (d.pending) return "Creating the pull request…";
    if (loading) return "Reading the branch…";
    if (d.base === "") return "Choose the branch it goes into.";
    // The suggestion lists the branch's commits not on its base: none, nothing to describe.
    if (suggestion !== null && suggestion.base === d.base && suggestion.body === "")
      return `Nothing to describe: ${shown(branch)} has no commits that are not on ${shown(d.base)}.`;
    return null;
  });

  async function writeDetails() {
    if (writing !== null || aiBlocked !== null) return;
    const id = newRequestId();
    // The draft, not this form: shown again meanwhile, the form there sees it, and the
    // answer lands in it.
    const draft = d;
    draft.ai.request = id;
    draft.ai.failure = null;
    const before: PrFields = { title: draft.title, body: draft.body, draft: draft.draft };
    const outcome = await generate(repoId, id, {
      kind: "pr",
      branch,
      base: draft.base,
      baseRemote,
      title: before.title,
      body: before.body,
      draft: before.draft,
    });
    // Cancelled, or another took its place: nothing of it lands.
    if (draft.ai.request !== id) return;
    draft.ai.request = null;
    if (outcome.status === "done" && outcome.title !== null) {
      const got: PrFields = { title: outcome.title, body: outcome.body ?? before.body, draft: outcome.draft ?? before.draft };
      const had = before.title.trim() !== "" || before.body.trim() !== "";
      draft.ai.undo = had ? { previous: before, generated: got } : null;
      draft.title = got.title;
      draft.body = got.body;
      draft.draft = got.draft;
      draft.titleTouched = true;
      draft.bodyTouched = true;
    } else if (outcome.status === "failed" && outcome.kind !== "cancelled") {
      draft.ai.failure = outcome;
    }
  }

  function stopWriting() {
    const id = d.ai.request;
    if (id === null) return;
    // Back to the form as it was at once; the tool is ended.
    d.ai.request = null;
    void cancelGeneration(id);
  }

  function undoGenerated() {
    if (undo === null) return;
    d.title = undo.previous.title;
    d.body = undo.previous.body;
    d.draft = undo.previous.draft;
    d.ai.undo = null;
  }

  // Edited since: what was generated is the user's now, and there is no undo.
  $effect(() => {
    if (undo !== null && (d.title !== undo.generated.title || d.body !== undo.generated.body)) d.ai.undo = null;
  });

  function onKeydown(e: KeyboardEvent) {
    const mod = isMac() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
    const field = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
    if (e.key === "Enter" && (isMac() ? e.metaKey : e.ctrlKey) && !e.isComposing) submit(e);
    else if (field && mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "g" && !e.isComposing) {
      e.preventDefault();
      void aiControl?.activate();
    } else if (field && mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "z" && undo !== null) {
      e.preventDefault();
      undoGenerated();
    } else if (e.key === "Escape" && writing !== null) {
      e.preventDefault();
      e.stopPropagation();
      stopWriting();
    }
  }

  const failureUrl = $derived.by(() => {
    const hint = d.failure?.hints.find((h) => h.kind === "open_url");
    return hint?.kind === "open_url" ? hint : null;
  });

  /** The repository it goes into ("octo/cat"), as the core named it (nothing until then). */
  const intoRepo = $derived(suggestion?.repo ?? null);
</script>

{#snippet spinner()}
  <svg class="spin" width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.25" stroke-width="1.8" />
    <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
  </svg>
{/snippet}

{#snippet info()}
  <svg width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.3" fill="none" stroke="currentColor" stroke-width="1.4" />
    <path d="M8 7.2v3.9" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
    <circle cx="8" cy="4.9" r="0.95" fill="currentColor" />
  </svg>
{/snippet}

<!-- Cmd+Enter (Ctrl+Enter) in a field sends it, as in the commit message; Enter in the title too. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form class="pr-form" data-testid="pr-form" onsubmit={submit} onkeydown={onKeydown} novalidate>
  {#if compare}
    <div class="fields compare">
      <div class="box" data-testid="pr-no-gh">
        <p class="box-lead">{PR_REASONS[compare]}.</p>
        <p>
          {#if compare === "no_gh"}Install it (cli.github.com) and sign in, then refresh, to make pull requests here.{:else}Then
            refresh, to make pull requests here.{/if}
        </p>
        <p>
          Meanwhile: {#if needsPush && pushTo !== null}{shown(branch)} is pushed to {shown(pushTo)}, then{/if}
          GitHub’s compare page opens to finish it there.
        </p>
      </div>
    </div>
  {:else}
    <div class="fields">
      <div class="pf-field">
        <div class="label-row">
          <label class="field-label" for="pr-title-{uid}">Title</label>
          <span class="row-end">
            {#if writing !== null}
              <button type="button" class="mini-link" data-testid="pr-ai-cancel" onclick={stopWriting}>Cancel</button>
            {:else if undo !== null}
              <button
                type="button"
                class="mini-link"
                title="Put back the title and description it replaced ({isMac() ? '⌘Z' : 'Ctrl+Z'} in a field)"
                data-testid="pr-ai-undo"
                onclick={undoGenerated}>Undo</button
              >
            {/if}
            <AiGenerate
              bind:this={aiControl}
              kind="pr"
              blocked={aiBlocked}
              running={writing !== null}
              onrun={() => void writeDetails()}
              oncancel={stopWriting}
              testid="pr-ai"
            />
          </span>
        </div>
        <input
          id="pr-title-{uid}"
          class="text-input"
          class:writing={writing !== null}
          type="text"
          bind:value={d.title}
          oninput={() => {
            d.titleTouched = true;
            d.ai.failure = null;
          }}
          readonly={d.pending || writing !== null}
          aria-busy={writing !== null}
          placeholder={loading ? "Suggested from the branch’s commits…" : ""}
          autocomplete="off"
          spellcheck="true"
          aria-invalid={!loading && titleProblem !== null && d.titleTouched}
          data-pr-focus
          data-testid="pr-title"
        />
      </div>
      <div class="pf-field grow">
        <label class="field-label" for="pr-body-{uid}">Description <span class="optional">(optional)</span></label>
        <textarea
          id="pr-body-{uid}"
          class="text-input body"
          class:writing={writing !== null}
          bind:value={d.body}
          oninput={() => {
            d.bodyTouched = true;
            d.ai.failure = null;
          }}
          readonly={d.pending || writing !== null}
          aria-busy={writing !== null}
          spellcheck="true"
          placeholder={writing !== null
            ? "Writing the title and description…"
            : loading
              ? "Listing the branch’s commits…"
              : "What it changes, and why."}
          data-testid="pr-body"
        ></textarea>
      </div>
      {#if aiFailure !== null && writing === null}
        <AiProblem failure={aiFailure} {repoId} onretry={() => void writeDetails()} testid="pr-ai-problem" />
      {/if}
    </div>
  {/if}

  <div class="compose">
    {#if !compare}
      <div class="pf-field">
        <!-- The repository it goes into is the core's (the base repository); only its
             branches are offered. -->
        <label class="field-label into" for="pr-base-{uid}"
          >Into{#if intoRepo}<span class="into-repo" data-testid="pr-repo" title={intoRepo}>{intoRepo}</span>{/if}</label
        >
        <div class="select-wrap">
          <select
            id="pr-base-{uid}"
            class="select"
            bind:value={d.base}
            onchange={() => {
              // Picked among the branches of the repository shown: kept while it is the one.
              d.baseTouched = true;
              d.baseRemote = baseRemote;
            }}
            disabled={d.pending}
            data-testid="pr-base"
          >
            {#if d.base === ""}
              <option value="" disabled>Choose a branch…</option>
            {/if}
            {#each bases as b (b)}
              <option value={b}>{shown(b)}</option>
            {/each}
          </select>
        </div>
      </div>
      <label class="draft-row" title="Not ready for review yet: reviewers are not asked until it is marked ready">
        <input class="checkbox" type="checkbox" bind:checked={d.draft} disabled={d.pending} data-testid="pr-draft" />
        <span>Create as draft</span>
      </label>
    {/if}

    {#if needsPush && pushTo !== null && pushWhy !== null && !d.pending && !compare}
      <p class="push-note" data-testid="push-note" title={pushWhy}>
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
          <path d="M6 9.8V2.6M3.2 5.2 6 2.4l2.8 2.8" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        <span>Pushes first to <MiddleText class="push-where" text={pushTo} keep={14} title={null} /></span>
      </p>
    {/if}

    <button
      class="button primary"
      type="submit"
      disabled={!valid}
      title={compare ? undefined : `Create the pull request (${SUBMIT_KEYS})`}
      data-pr-focus={compare ? true : undefined}
      data-testid="pr-create">{primary}</button
    >

    {#if d.pending}
      <p class="note step" role="status" data-testid="pr-step">
        {@render spinner()}
        <span class="step-text">
          {d.step ?? (needsPush && pushTo !== null ? `Pushing ${shown(branch)} to ${shown(pushTo)}…` : "Creating the pull request…")}
          {#if d.progress}<span class="progress">{d.progress}</span>{/if}
        </span>
      </p>
    {:else if suggestFailed}
      <p class="note problem" role="alert" data-testid="pr-suggest-failed">
        <span>{suggestFailed} <button class="link-btn" type="button" onclick={onRetry}>Try again</button></span>
      </p>
    {:else if loading}
      <p class="note" role="status" data-testid="pr-waiting">
        {@render spinner()}
        <span>Asking gh for the default branch and any open pull request of {shown(branch)}…</span>
      </p>
    {:else if d.refused}
      <p class="note problem" role="alert" data-testid="pr-error"><span>{d.refused}</span></p>
    {:else if d.failure}
      <div class="note problem failure" role="alert" data-testid="pr-error">
        <p>{visibleText(d.failure.message)}</p>
        <div class="failure-actions">
          {#if failureUrl}
            <button class="link-btn" type="button" onclick={() => openUrl(failureUrl.url)}>{failureUrl.label}</button>
          {/if}
          {#if d.failure.logFrom > 0}
            <button class="link-btn" type="button" onclick={() => showLog(repoId, d.failure!.logFrom)}>Show log</button>
          {/if}
        </div>
      </div>
    {:else if reason !== null}
      <p class="note" data-testid="pr-reason">{@render info()}<span>{reason}</span></p>
    {/if}
  </div>
</form>

<style>
  .pr-form {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  /* The title and the description; the description takes the height there is. */
  .fields {
    display: flex;
    flex-direction: column;
    gap: 12px;
    flex: 1 1 0;
    min-height: 0;
    padding: 10px 12px 12px;
    overflow-y: auto;
  }

  .pf-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .pf-field.grow {
    flex: 1 1 auto;
    min-height: 0;
  }

  .field-label {
    margin: 0;
  }

  /* The Title's label, and Generate with AI at its end: one line, as tall as the button. */
  .label-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-width: 0;
  }

  .row-end {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    margin: -4px -4px -4px 0;
  }

  .mini-link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 550;
  }

  .mini-link:hover {
    text-decoration: underline;
  }

  .mini-link:focus-visible {
    border-radius: 3px;
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .writing {
    color: var(--text-muted);
    cursor: progress;
  }

  .text-input,
  .select {
    font-size: var(--fs-sm);
  }

  /* As tall as the panel leaves it (at least a few lines); drawn taller, the fields scroll. */
  .body {
    flex: 1 1 auto;
    min-height: 96px;
    max-height: none;
    font-family: var(--font-ui);
  }

  /* Where it goes and the button, at the bottom, as the commit panel's message and buttons. */
  .compose {
    display: flex;
    flex-direction: column;
    flex: none;
    gap: 8px;
    padding: 10px 12px 12px;
    border-top: 1px solid var(--border);
  }

  .compare + .compose {
    border-top: 0;
  }

  .into {
    display: flex;
    align-items: baseline;
    gap: 5px;
    min-width: 0;
  }

  .into-repo {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .draft-row {
    display: flex;
    align-items: center;
    gap: 7px;
    width: fit-content;
    color: var(--text);
    font-size: var(--fs-sm);
    cursor: default;
    user-select: none;
  }

  .push-note {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    margin: 2px 0 -2px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }

  .push-note svg {
    flex: none;
    color: var(--text-faint);
  }

  .push-note > span {
    display: flex;
    gap: 0.3em;
    min-width: 0;
    white-space: nowrap;
  }

  .push-note :global(.push-where) {
    min-width: 0;
  }

  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
    white-space: nowrap;
    transition:
      background-color 80ms linear,
      border-color 80ms linear;
  }

  .button:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .button:active:not(:disabled) {
    background: var(--bg-pressed);
  }

  .button:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .button:disabled {
    border-color: var(--border);
    background: transparent;
    color: var(--text-faint);
  }

  /* The form's own button: its label may need two lines in a narrow panel (with wide fonts):
     it wraps, balanced, rather than lose words. */
  .button.primary {
    height: auto;
    min-height: 28px;
    padding: 5px 12px;
    line-height: 16px;
    text-align: center;
    text-wrap: balance;
    white-space: normal;
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
    background: var(--bg-pressed);
    color: var(--text-faint);
  }

  /* Under the button: what it waits for, why it cannot be pressed, how it went. */
  .note {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin: 0;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .note > :global(svg) {
    flex: none;
    margin-top: 1.5px;
    color: var(--text-faint);
  }

  .note.problem {
    color: var(--danger);
  }

  .failure {
    flex-direction: column;
    gap: 4px;
  }

  .failure-actions {
    display: flex;
    gap: 12px;
  }

  .step-text {
    min-width: 0;
  }

  .progress {
    display: block;
    color: var(--text-faint);
    font-family: var(--font-mono);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .spin {
    animation: spin 0.8s linear infinite;
  }

  .link-btn {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: inherit;
    font-weight: 550;
    cursor: pointer;
  }

  .link-btn:hover {
    text-decoration: underline;
  }

  .link-btn:focus-visible {
    border-radius: 3px;
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  /* Why there is no form, and what happens instead: as the commit panel's box for another
     worktree. */
  .box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 9px 11px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .box-lead {
    color: var(--text);
  }

  .compare {
    flex: 0 0 auto;
    padding-bottom: 0;
  }
</style>
