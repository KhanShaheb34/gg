<!--
  Create pull request (spec §5.5) for a local branch. Opens at once and asks the core what to
  offer (`pr_suggest`: a title and description from the branch's commits, the base, any open
  pull request of the branch, GitHub's compare page), which can take a few seconds: it says
  what it waits for, and Cancel works meanwhile. The fields can be filled in already; a
  suggestion that comes late never replaces what the user typed.

  Then, as the answer and the repository's pull-request state say: the form (title, description,
  base among the remote branches, draft), with a primary button that names the push it makes
  first when one is needed (R38); "Pull request #N already exists" with Open and Copy link; or
  why nothing can be made (not GitHub). Without gh, or signed out, the branch is pushed and
  the compare page opens to finish there. Where it pushes, whether it must, and the base
  repository come from the core (`pr_suggest`), worked out as `pr_create` will.

  While the request is out, Cancel is off and the step is said (pushing, creating; R27). A
  failure stays here, inline, with Show log; success closes it and leaves a notice with Open
  and Copy link (R30).
-->
<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import { visibleText } from "../../format";
  import { api, CoreError, onEvent } from "../../ipc";
  import { defaultRemote, PR_REASONS, shown } from "../../menus/common";
  import { notify, showLog } from "../../state/app.svelte";
  import { openExternal } from "../../state/editor";
  import { runOp } from "../../state/ops.svelte";
  import { copyLink } from "../../state/prs.svelte";
  import { findRepoView } from "../../state/repo.svelte";
  import type { OpOutcome, OpProgress, PrSuggestion, PullRequest } from "../../types";
  import Dialog from "../Dialog.svelte";

  type Props = {
    repoId: number;
    /** The local branch the pull request is made from, as it was when the menu opened. */
    branch: string;
    close: () => void;
  };

  let { repoId, branch, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  // svelte-ignore state_referenced_locally
  const view = findRepoView(repoId);
  const snap = $derived(view?.snapshot);
  const prs = $derived(snap?.prs ?? null);

  let open = true;
  onDestroy(() => (open = false));

  // --- What the core suggests ------------------------------------------------------------

  let suggestion = $state<PrSuggestion | null>(null);
  let suggestFailed = $state<string | null>(null);

  let title = $state("");
  let body = $state("");
  let base = $state("");
  let draft = $state(false);
  /** The user changed these: a suggestion arriving later leaves them as they are. */
  let titleTouched = $state(false);
  let bodyTouched = false;
  let baseTouched = false;

  // svelte-ignore state_referenced_locally
  /** A first guess at the base repository's remote until the suggestion says. */
  const guessRemote = defaultRemote(snap?.remotes ?? []) || snap?.remotes[0] || "origin";
  /**
   * The remote of the repository the pull request goes into: the base repository the core
   * chose (the same rule as the list's). Only its branches can be the base, so changing the
   * base never changes the repository (Task 9 review minor 4).
   */
  const baseRemote = $derived(suggestion ? suggestion.baseRemote : guessRemote);

  /** The base repository's branches a pull request can go into ("main"), without its HEAD. */
  const bases = $derived.by(() => {
    if (baseRemote === null) return [];
    const prefix = `${baseRemote}/`;
    const list = (snap?.remoteBranches ?? [])
      .filter((b) => b.startsWith(prefix) && b !== `${prefix}HEAD`)
      .map((b) => b.slice(prefix.length));
    if (base !== "" && !list.includes(base)) list.unshift(base);
    return list;
  });

  // svelte-ignore state_referenced_locally
  {
    // A first guess until the suggestion comes: the default remote's main, if it has one.
    if ((snap?.remoteBranches ?? []).includes(`${guessRemote}/main`)) base = "main";
  }

  void (async () => {
    // Not known yet whether gh can be asked, or it could not be (not installed, signed out:
    // the user may have fixed that a moment ago): a refresh the user would have asked for
    // loads them, and asks gh again at once (final boundary review I1).
    if (prs?.status === "unknown" || prs?.status === "unavailable") {
      api.refresh(repoId, true).then(
        (s) => view?.applySnapshot(s),
        () => {},
      );
    }
    try {
      const s = await api.prSuggest(repoId, branch);
      if (!open) return;
      suggestion = s;
      if (!titleTouched) title = s.title;
      if (!bodyTouched) body = s.body;
      // Another repository than guessed: its base, whatever was picked from the guess.
      if (s.baseRemote !== guessRemote || !baseTouched) base = s.baseRemote === null ? "" : s.base;
    } catch (e) {
      if (open) suggestFailed = e instanceof Error ? e.message : String(e);
    }
  })();

  const loading = $derived(suggestion === null && suggestFailed === null);
  const existing: PullRequest | null = $derived(suggestion?.existing ?? null);
  const compareUrl = $derived(suggestion?.compareUrl ?? null);

  /**
   * The repository's pull-request state as last settled: while the refresh the dialog asked
   * for loads them, what was known stays shown (no form flashing up and away).
   */
  // svelte-ignore state_referenced_locally
  let settled = $state(prs);
  $effect(() => {
    if (prs !== null && prs.status !== "loading") settled = prs;
  });
  /** Why gh cannot make it, when the repository's pull requests say so. */
  const unavailable = $derived(
    settled?.status === "unavailable" && settled.reason !== null && settled.reason !== "failed" ? settled.reason : null,
  );
  /**
   * gh cannot make it here (not installed, or signed out), but GitHub can: the branch is
   * pushed and GitHub's compare page opens to finish there (Task 9 review I5).
   */
  const compareOnly = $derived(unavailable === "no_gh" || unavailable === "signed_out");

  // --- The push it makes first (R38), as the core will make it ----------------------------

  /** Where the branch is pushed ("fork/x"), as `pr_create` will push it; null until known. */
  const pushTo = $derived(suggestion?.pushTo ?? null);
  /**
   * It is pushed first: its namesake there is missing or behind it, as the core said; unless
   * the repository now shows it level with it there (pushed since: by a try that then failed).
   */
  const needsPush = $derived.by(() => {
    if (!(suggestion?.needsPush ?? false)) return false;
    const label = view?.loadedLabel(branch);
    const level = pushTo !== null && label?.upstream === pushTo && label.ahead === 0 && (snap?.remoteBranches ?? []).includes(pushTo);
    return !level;
  });
  /** Branch names in button labels: cut in the middle so the button stays a button. */
  const short = (name: string) => shown(name, 22);
  const pushLabel = $derived(`Push ${short(branch)} to ${short(pushTo ?? "")}`);
  /** Why it is pushed: not there at all, or there but behind. */
  const pushNote = $derived(
    pushTo === null ? null : (snap?.remoteBranches ?? []).includes(pushTo) ? `${shown(pushTo)} is behind` : `not pushed to ${shown(pushTo)} yet`,
  );

  // --- Sending ----------------------------------------------------------------------------

  let pending = $state(false);
  /** The step the request is at, as the core says it (pushing, creating). */
  let step = $state<string | null>(null);
  /** git's own latest progress line, while pushing. */
  let progress = $state<string | null>(null);
  let failure = $state<OpOutcome | null>(null);
  /** A request the core refused outright (a bad title, say). */
  let refused = $state<string | null>(null);

  let stopListening: (() => void) | null = null;
  onDestroy(() => stopListening?.());

  const titleProblem = $derived.by(() => {
    if (title.trim() === "") return "A pull request needs a title.";
    if (/[\r\n]/.test(title)) return "The title is one line: put the rest in the description.";
    return null;
  });

  type Mode = "loading" | "existing" | "unavailable" | "form";
  const mode: Mode = $derived(
    loading ? "loading" : existing !== null ? "existing" : unavailable !== null && !compareOnly ? "unavailable" : "form",
  );

  const primary = $derived.by(() => {
    if (pending) return step?.startsWith("Pushing") ? "Pushing…" : step === null && needsPush ? "Pushing…" : "Creating…";
    if (mode === "existing") return "Open";
    if (mode === "unavailable") return "Create pull request";
    if (compareOnly) return needsPush && pushTo !== null ? `${pushLabel} and open the compare page` : "Open the compare page";
    return needsPush && pushTo !== null ? `${pushLabel} and create pull request` : "Create pull request";
  });

  const valid = $derived.by(() => {
    if (pending) return false;
    if (mode === "loading") return false;
    if (mode === "existing") return true;
    if (mode === "unavailable") return false;
    // Without gh only the push is made (its title is not used): the base must be one.
    if (compareOnly) return needsPush ? baseRemote !== null && pushTo !== null : compareUrl !== null;
    return titleProblem === null && baseRemote !== null && base !== "" && pushTo !== null;
  });

  // Task 9 review I4: the form's fields gone (the pull request exists, or none can be made),
  // the focus is not lost with them: Enter and Escape keep working.
  let frame: Dialog | undefined = $state();
  $effect(() => {
    if (mode === "existing" || mode === "unavailable") void tick().then(() => frame?.keepFocus());
  });

  function openUrl(url: string) {
    void openExternal(repoId, { target: "url", url });
  }

  /** Says how it went, with the page it made (or the compare page) to open or copy. */
  function done(outcome: OpOutcome) {
    const hint = outcome.hints.find((h) => h.kind === "open_url");
    const url = hint?.kind === "open_url" ? hint.url : null;
    // Without gh (or signed out) it ends on the compare page: opened, as the dialog said.
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
  }

  async function create() {
    const remote = baseRemote;
    if (remote === null) return;
    failure = null;
    refused = null;
    step = null;
    progress = null;
    pending = true;
    stopListening = onEvent<OpProgress>("op-progress", (p) => {
      if (p.repoId !== repoId) return;
      if (p.line.endsWith("…") && (p.line.startsWith("Pushing ") || p.line.startsWith("Creating "))) {
        step = p.line;
        progress = null;
      } else {
        progress = p.line;
      }
    });
    try {
      const outcome = await runOp(
        repoId,
        {
          op: "pr_create",
          branch,
          base,
          baseRemote: remote,
          title: title.trim() || branch,
          body,
          draft,
          // What the dialog shows is what is done: the compare page it promised never turns
          // into a pull request made through gh (final boundary review I1).
          comparePage: compareOnly,
        },
        { ownErrors: ["bad_request", "bad_name"], ownOutcome: true },
      );
      if (!open) return;
      if (outcome.status === "failed") {
        failure = outcome;
        // The push may have been made before gh failed: the button says what is left to do.
        void refreshPushPlan();
        return;
      }
      close();
      done(outcome);
    } catch (e) {
      if (!open) return;
      if (e instanceof CoreError && (e.code === "bad_request" || e.code === "bad_name")) refused = e.message;
      else close(); // reported by runOp
    } finally {
      stopListening?.();
      stopListening = null;
      pending = false;
    }
  }

  /** Asks the core again where the branch goes and whether it must be pushed. */
  async function refreshPushPlan() {
    try {
      const s = await api.prSuggest(repoId, branch);
      if (open && suggestion !== null) {
        suggestion = { ...suggestion, pushRemote: s.pushRemote, pushTo: s.pushTo, needsPush: s.needsPush };
      }
    } catch {
      // The button keeps what it said; the core decides again when it is pressed.
    }
  }

  function submit() {
    if (!valid) return;
    if (mode === "existing" && existing !== null) {
      openUrl(existing.url);
      close();
    } else if (compareOnly && !needsPush) {
      if (compareUrl !== null) openUrl(compareUrl);
      close();
    } else {
      void create();
    }
  }

  const failureUrl = $derived.by(() => {
    const hint = failure?.hints.find((h) => h.kind === "open_url");
    return hint?.kind === "open_url" ? hint : null;
  });

  /** The repository it goes into ("octo/cat"), as the core named it; its remote until then. */
  const intoRepo = $derived(suggestion?.repo ?? baseRemote);

  const dialogTitle = $derived(mode === "existing" && existing ? `Pull request #${existing.number} already exists` : "Create pull request");
</script>

<Dialog
  bind:this={frame}
  title={dialogTitle}
  {primary}
  {valid}
  once={false}
  wide
  cancellable={!pending}
  secondary={mode === "existing" ? "Copy link" : undefined}
  onSecondary={mode === "existing" && existing ? () => void copyLink(existing.url) : undefined}
  onSubmit={submit}
  onCancel={close}
>
  <div class="dialog-target from">
    <span class="dialog-target-label">From</span>
    <span class="dialog-ref" title={branch}>{shown(branch)}</span>
    {#if mode === "form" && needsPush && pushNote !== null}
      <span class="push-note" data-testid="push-note" title={pushNote}>{pushNote}</span>
    {/if}
  </div>

  {#if mode === "loading"}
    <p class="waiting" role="status" data-testid="pr-waiting">
      <svg class="spin" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.25" stroke-width="1.8" />
        <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
      </svg>
      <span>Asking gh for the default branch and any open pull request of {shown(branch)}…</span>
    </p>
  {:else if mode === "existing" && existing}
    <div class="existing" data-testid="pr-existing">
      <p class="pr-title">
        <span class="pr-number">#{existing.number}</span>
        {visibleText(existing.title)}
        {#if existing.state === "draft"}<span class="draft-badge">Draft</span>{/if}
      </p>
      <p class="pr-flow">{shown(existing.base)} ← {shown(existing.head)}</p>
      <p class="dialog-text muted">Each branch has one open pull request on GitHub; new commits pushed to it go there.</p>
    </div>
  {:else if mode === "unavailable" && unavailable}
    <p class="reason" data-testid="pr-unavailable">{PR_REASONS[unavailable]}.</p>
    <p class="dialog-text muted">Pull requests are made on GitHub only.</p>
  {/if}

  {#if mode === "form" || (mode === "loading" && !suggestFailed)}
    {#if compareOnly && mode === "form" && unavailable}
      <p class="nogh" data-testid="pr-no-gh">
        {PR_REASONS[unavailable]}: {#if needsPush && pushTo !== null}{shown(branch)} is pushed to {shown(pushTo)}, then{/if}
        GitHub’s compare page opens to finish it there.
      </p>
    {/if}
    {#if !compareOnly}
      <div class="field">
        <label class="field-label" for="pr-title-{uid}">Title</label>
        <input
          id="pr-title-{uid}"
          class="text-input"
          type="text"
          bind:value={title}
          oninput={() => (titleTouched = true)}
          readonly={pending}
          placeholder={loading ? "Suggested from the branch’s commits…" : ""}
          autocomplete="off"
          aria-invalid={!loading && titleProblem !== null && titleTouched}
          data-autofocus
          data-testid="pr-title"
        />
        {#if !loading && titleProblem !== null && titleTouched}
          <p class="field-hint problem">{titleProblem}</p>
        {/if}
      </div>
      <div class="field">
        <label class="field-label" for="pr-body-{uid}">Description <span class="optional">(optional)</span></label>
        <textarea
          id="pr-body-{uid}"
          class="text-input body"
          bind:value={body}
          oninput={() => (bodyTouched = true)}
          readonly={pending}
          rows="6"
          placeholder={loading ? "Listing the branch’s commits…" : "What it changes, and why."}
          data-testid="pr-body"
        ></textarea>
      </div>
      <div class="row">
        <div class="field base">
          <!-- The repository it goes into is the core's (the base repository); only its
               branches are offered. -->
          <label class="field-label" for="pr-base-{uid}"
            >Into{#if intoRepo}&nbsp;<span class="into-repo" data-testid="pr-repo" title={intoRepo}>{shown(intoRepo)}</span>{/if}</label
          >
          <div class="select-wrap">
            <select
              id="pr-base-{uid}"
              class="select"
              bind:value={base}
              onchange={() => (baseTouched = true)}
              disabled={pending}
              data-testid="pr-base"
            >
              {#if base === ""}
                <option value="" disabled>Choose a branch…</option>
              {/if}
              {#each bases as b (b)}
                <option value={b}>{shown(b)}</option>
              {/each}
            </select>
          </div>
        </div>
        <label class="check-row draft">
          <input class="checkbox" type="checkbox" bind:checked={draft} disabled={pending} data-testid="pr-draft" />
          <span>
            Draft
            <span class="check-note">Not ready for review yet.</span>
          </span>
        </label>
      </div>
    {/if}
  {/if}

  {#if suggestFailed}
    <p class="problem-text" role="alert">{suggestFailed}</p>
  {/if}

  {#if pending}
    <p class="waiting step" role="status" data-testid="pr-step">
      <svg class="spin" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.25" stroke-width="1.8" />
        <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
      </svg>
      <span class="step-text">
        {step ?? (needsPush && pushTo !== null ? `Pushing ${shown(branch)} to ${shown(pushTo)}…` : "Creating the pull request…")}
        {#if progress}<span class="progress">{progress}</span>{/if}
      </span>
    </p>
  {/if}

  {#if refused}
    <p class="problem-text" role="alert" data-testid="pr-error">{refused}</p>
  {:else if failure}
    <div class="failure" role="alert" data-testid="pr-error">
      <p class="problem-text">{visibleText(failure.message)}</p>
      <div class="failure-actions">
        {#if failureUrl}
          <button class="link-btn" type="button" onclick={() => openUrl(failureUrl.url)}>{failureUrl.label}</button>
        {/if}
        {#if failure.logFrom > 0}
          <button class="link-btn" type="button" onclick={() => showLog(repoId, failure!.logFrom)}>Show log</button>
        {/if}
      </div>
    </div>
  {/if}
</Dialog>

<style>
  .from {
    align-items: center;
  }

  /* One line each: a long branch is cut (its name in full in the tooltip), never broken over
     several lines that push the base branch and the buttons out of a small window. */
  .from .dialog-ref {
    flex: 0 1 auto;
    overflow: hidden;
    overflow-wrap: normal;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .push-note {
    margin-left: auto;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    color: var(--text-faint);
    font-size: var(--fs-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .waiting {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin-top: 12px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }

  .waiting svg {
    flex: none;
    margin-top: 1px;
  }

  .spin {
    animation: spin 0.8s linear infinite;
  }

  .step-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .progress {
    display: block;
    color: var(--text-faint);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .body {
    min-height: 116px;
    font-family: var(--font-ui);
  }

  .row {
    display: flex;
    align-items: flex-end;
    gap: 16px;
  }

  .base {
    flex: 1;
    min-width: 0;
  }

  .draft {
    flex: none;
    margin-bottom: 2px;
  }

  .existing {
    margin-top: 12px;
  }

  .pr-title {
    font-weight: 600;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }

  .pr-number {
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }

  .draft-badge {
    margin-left: 4px;
    padding: 0 5px;
    border: 1px dashed var(--border-strong);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    font-weight: 500;
  }

  .pr-flow {
    margin: 2px 0 10px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    overflow-wrap: anywhere;
  }

  .reason {
    margin: 12px 0 4px;
    font-weight: 600;
    line-height: 1.45;
  }

  .nogh {
    margin-top: 12px;
    padding: 8px 10px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--info) 8%, transparent);
    color: var(--text);
    font-size: var(--fs-sm);
    line-height: 1.5;
    overflow-wrap: anywhere;
  }

  .problem-text {
    margin-top: 12px;
    color: var(--danger);
    font-size: var(--fs-sm);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .failure-actions {
    display: flex;
    gap: 12px;
    margin-top: 4px;
  }

  .link-btn {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--fs-sm);
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

  .into-repo {
    color: var(--text);
    font-family: var(--font-mono);
    font-weight: 500;
  }
</style>
