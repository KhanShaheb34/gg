<!--
  The branch picker (⌘P, or the branch name in the repository header): a field at the top of
  the window over the active tab's branches, local then remote, newest commit first. Typing
  keeps the names that contain the text (any case). Choosing a local branch checks it out as
  the graph's "Check out" does (asking to fast-forward one behind its upstream); one checked
  out in another worktree makes that worktree active instead; the current one only closes. A
  remote branch checks out the local branch of its name, or makes it, tracking the remote one.
  Below the branches: "Create new branch ‘name’" (at HEAD, checked out) and "Create new branch
  from…", which lists HEAD and the branches to start from, then asks for the name if it was
  not typed. Esc and Backspace in an empty field go back a step (Esc closes from the first);
  a click outside closes. Nothing here runs git or asks the core until a row is chosen:
  everything comes from the snapshot, and only the rows in view are rendered.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    localNameOf,
    pickerIndex,
    pickerList,
    ROW_H,
    rowAt,
    rowKey,
    visibleRange,
    type PickerBranch,
    type PickerRow,
    type PickerStep,
    type StartPoint,
  } from "../branchPicker";
  import { relativeTime, shortHash, shortenPath, visibleText } from "../format";
  import { checkOutBranch } from "../menus/checkout";
  import { inProgressReason, shown } from "../menus/common";
  import { refNameProblem } from "../nameCheck";
  import { matchParts, searchPattern } from "../search";
  import { app, toast } from "../state/app.svelte";
  import { findRepoView } from "../state/repo.svelte";
  import { activateWorktree } from "../state/worktrees.svelte";
  import { NamedSubmit } from "./dialogs/named.svelte";

  let { repoId, close }: { repoId: number; close: () => void } = $props();

  // svelte-ignore state_referenced_locally
  const view = findRepoView(repoId)!;
  // svelte-ignore state_referenced_locally
  const named = new NamedSubmit(repoId, close);
  const uid = `branch-picker-${Math.random().toString(36).slice(2, 8)}`;
  /** Rows a Page Up or Down moves. */
  const PAGE = 8;
  /** Relative times are as of the picker's opening. */
  const now = Date.now();

  const snap = $derived(view.snapshot);
  const index = $derived(pickerIndex(snap));
  let step = $state<PickerStep>({ kind: "list" });
  /** The steps before this one, with what their field held, for going back. */
  const history: { step: PickerStep; text: string }[] = [];
  let text = $state("");
  const typed = $derived(text.trim());
  const list = $derived(pickerList(index, step, text));
  let active = $state(0);
  let listTop = $state(0);
  let listH = $state(10 * ROW_H);
  const range = $derived(visibleRange(list.length, listTop, listH));
  const pattern = $derived(step.kind === "name" ? null : searchPattern(text));
  /** Why the last choice could not go ahead (a name that cannot be a branch's), until the text changes. */
  let refused = $state<string | null>(null);
  /** The row whose request is out. */
  let pendingKey = $state<string | null>(null);
  let input: HTMLInputElement | undefined = $state();
  let listEl: HTMLElement | undefined = $state();

  const rows = $derived.by(() => {
    const out: { row: PickerRow; i: number }[] = [];
    for (let i = range[0]; i < range[1]; i++) out.push({ row: rowAt(list, i), i });
    return out;
  });
  const activeId = $derived(active >= range[0] && active < range[1] ? `${uid}-${active}` : undefined);
  const headPoint = $derived<StartPoint | null>(index.head ? { kind: "head", hash: index.head.hash } : null);
  const problem = $derived(named.problem(typed, refused));

  /** What the step asks, above the field (the first step's field says it itself). */
  const prompt = $derived.by(() => {
    if (step.kind === "from") {
      return step.name ? `Select a ref to create ‘${shown(step.name)}’ from` : "Select a ref to create the branch from";
    }
    if (step.kind === "name") return `Name the new branch, at ${pointName(step.at)}`;
    return null;
  });
  const placeholder = $derived(
    step.kind === "from" ? "Filter HEAD and the branches" : step.kind === "name" ? "Branch name" : "Switch to a branch, or type a name for a new one",
  );

  onMount(() => {
    input?.focus();
  });

  function pointName(at: StartPoint): string {
    return at.kind === "head" ? `HEAD (${shortHash(at.hash)})` : shown(at.branch.name);
  }

  /** Why `name` cannot be the new branch's (null: it can). */
  function nameProblem(name: string): string | null {
    return refNameProblem(name, snap.branches, "branch");
  }

  function setActive(i: number) {
    active = Math.max(0, Math.min(list.length - 1, i));
    if (!listEl) return;
    const top = active * ROW_H;
    if (top < listEl.scrollTop) listEl.scrollTop = top;
    else if (top + ROW_H > listEl.scrollTop + listEl.clientHeight) listEl.scrollTop = top + ROW_H - listEl.clientHeight;
    listTop = listEl.scrollTop;
  }

  function toTop() {
    active = 0;
    listTop = 0;
    if (listEl) listEl.scrollTop = 0;
  }

  function onInput() {
    refused = null;
    toTop();
  }

  /** On to step `next`, its field holding `nextText`. */
  function go(next: PickerStep, nextText: string) {
    history.push({ step, text });
    step = next;
    text = nextText;
    refused = null;
    toTop();
    input?.focus();
  }

  /** Back a step; from the first, the picker closes. */
  function back() {
    if (named.pending) return;
    const prev = history.pop();
    if (!prev) {
      close();
      return;
    }
    step = prev.step;
    text = prev.text;
    refused = null;
    toTop();
  }

  /** Closes the picker, then does `then` once it is gone (a dialog it opens is not under it). */
  async function closeThen(then: () => void) {
    close();
    await tick();
    then();
  }

  function choose(row: PickerRow | undefined) {
    if (!row || named.pending) return;
    switch (row.kind) {
      case "branch":
        if (step.kind === "list") chooseBranch(row.branch);
        else startAt({ kind: "branch", branch: row.branch }, rowKey(row));
        return;
      case "head":
        startAt({ kind: "head", hash: row.hash }, rowKey(row));
        return;
      case "create": {
        const at = row.at ?? headPoint;
        if (at === null) {
          refused = "The repository has no commits yet: there is nothing to start a branch at.";
          return;
        }
        if (row.at === null && row.name === "") go({ kind: "name", at }, "");
        else create(row.name, at, rowKey(row));
        return;
      }
      case "createFrom":
        go({ kind: "from", name: row.name === "" ? null : row.name }, "");
        return;
    }
  }

  /** A start point chosen: the branch is made there if its name was given (and can be used), else asked for. */
  function startAt(at: StartPoint, key: string) {
    const name = step.kind === "from" ? step.name : null;
    if (name !== null && nameProblem(name) === null) create(name, at, key);
    else go({ kind: "name", at }, name ?? "");
  }

  /** Makes branch `name` at `at` and checks it out: one git command (`checkout -b`). `key`: the row chosen. */
  function create(name: string, at: StartPoint, key: string) {
    const why = nameProblem(name);
    if (why !== null) {
      refused = why;
      return;
    }
    const hash = at.kind === "head" ? at.hash : at.branch.hash;
    pendingKey = key;
    void named.submit(name, { op: "create_branch", name, at: hash, checkout: true, expectWorktree: snap.activeWorktree });
  }

  /** A branch chosen in the list: what its label's "Check out" would do, for a local one. */
  function chooseBranch(b: PickerBranch) {
    const s = snap;
    if (b.remote) {
      const local = index.byName.get(localNameOf(b, s.remotes));
      if (local) {
        chooseBranch(local);
        return;
      }
      const busy = inProgressReason(s);
      if (busy) {
        void closeThen(() => toast("info", busy));
        return;
      }
      const localName = localNameOf(b, s.remotes);
      pendingKey = `r:${b.name}`;
      void named.submit(localName, { op: "checkout_remote", remoteBranch: b.name, localName, expectWorktree: s.activeWorktree });
      return;
    }
    if (b.current) {
      close();
      return;
    }
    if (b.worktree !== null) {
      const wt = s.worktrees.find((w) => w.path === b.worktree);
      void closeThen(() => {
        if (wt) activateWorktree(view, wt);
      });
      return;
    }
    const busy = inProgressReason(s);
    void closeThen(() => {
      if (busy) toast("info", busy);
      else checkOutBranch(repoId, b.name, s.activeWorktree);
    });
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    switch (e.key) {
      case "ArrowDown":
        setActive(active + 1);
        break;
      case "ArrowUp":
        setActive(active - 1);
        break;
      case "PageDown":
        setActive(active + PAGE);
        break;
      case "PageUp":
        setActive(active - PAGE);
        break;
      case "Home":
        setActive(0);
        break;
      case "End":
        setActive(list.length - 1);
        break;
      case "Enter":
        if (!e.repeat && list.length > 0) choose(rowAt(list, Math.min(active, list.length - 1)));
        break;
      case "Escape":
        back();
        break;
      case "Backspace":
        if (text !== "" || step.kind === "list") return;
        back();
        break;
      case "Tab":
        // The field keeps the focus: the rows are chosen from it.
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  /** The last folder of `path`, for naming a worktree in a row. */
  function folderOf(path: string): string {
    const parts = path.split(/[\\/]/).filter((p) => p !== "");
    return parts[parts.length - 1] ?? path;
  }

  /** The second line of a create row: where it starts, or why the name typed cannot be used. */
  function createDetail(row: Extract<PickerRow, { kind: "create" }>): { text: string; problem: boolean } {
    const at = row.at ?? headPoint;
    if (at === null) return { text: "The repository has no commits yet", problem: true };
    if (row.name !== "") {
      const why = named.problem(row.name, nameProblem(row.name));
      if (why !== null) return { text: why, problem: true };
    }
    return { text: `At ${pointName(at)}, then checked out`, problem: false };
  }

  /** Said on the row whose request is out: a remote branch checked out from the list, else a branch made. */
  const pendingLabel = $derived(
    named.waitingFor !== null
      ? `Waiting for ${visibleText(named.waitingFor)}…`
      : step.kind === "list" && pendingKey?.startsWith("r:")
        ? "Checking out…"
        : "Creating…",
  );
</script>

{#snippet highlighted(name: string)}
  {#each matchParts(name, pattern) as part, k (k)}{#if part.match}<mark>{visibleText(part.text)}</mark>{:else}{visibleText(part.text)}{/if}{/each}
{/snippet}

{#snippet branchIcon()}
  <svg class="icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
    <circle cx="4" cy="3" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" />
    <circle cx="4" cy="11" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" />
    <circle cx="10.5" cy="4.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3" />
    <path d="M4 4.6v4.8M10.5 6.1c0 2.4-2.6 2.6-5.5 3.6" fill="none" stroke="currentColor" stroke-width="1.3" />
  </svg>
{/snippet}

{#snippet plusIcon()}
  <svg class="icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
    <path d="M7 2.6v8.8M2.6 7h8.8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
  </svg>
{/snippet}

{#snippet spinner()}
  <svg class="spinner" width="12" height="12" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.22" stroke-width="1.8" />
    <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
  </svg>
{/snippet}

{#snippet entry(row: PickerRow, i: number)}
  {@const key = rowKey(row)}
  {@const working = named.pending && pendingKey === key}
  <!-- The keys are the field's (aria-activedescendant); options take clicks only. -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="option"
    class:separated={(row.kind === "branch" && row.first && i > 0) || (row.kind !== "branch" && row.kind !== "head" && i > 0 && rowAt(list, i - 1).kind === "branch")}
    id="{uid}-{i}"
    role="option"
    tabindex="-1"
    aria-selected={i === active}
    data-active={i === active}
    data-kind={row.kind}
    data-name={row.kind === "branch" ? row.branch.name : undefined}
    style:transform="translateY({i * ROW_H}px)"
    onpointerdown={(e) => e.preventDefault()}
    onpointermove={() => {
      if (!named.pending) active = i;
    }}
    onclick={() => {
      active = i;
      choose(row);
    }}
  >
    {#if row.kind === "branch"}
      {@const b = row.branch}
      <div class="line one">
        {@render branchIcon()}
        <span class="name" class:current={b.current}>{@render highlighted(b.name)}</span>
        {#if b.current}
          <svg class="head" width="10" height="10" viewBox="0 0 10 10" aria-label="checked out here" role="img">
            <circle cx="5" cy="5" r="3.1" fill="none" stroke="currentColor" stroke-width="1.6" />
          </svg>
        {/if}
        {#if b.worktree !== null}
          <span class="chip" title="Checked out in the worktree at {shortenPath(b.worktree, app.homeDir)}: choosing it makes that worktree active"
            >in worktree {folderOf(b.worktree)}</span
          >
        {/if}
        {#if working}
          <span class="when working">{@render spinner()}{pendingLabel}</span>
        {:else if b.time > 0}
          <span class="when">{relativeTime(b.time * 1000, now)}</span>
        {/if}
        {#if row.first}
          <span class="group-label">{row.group === "local" ? "branches" : "remote branches"}</span>
        {/if}
      </div>
      <div class="line two">
        {#if b.author !== "" || b.subject !== ""}
          <span class="author">{visibleText(b.author)}</span><span class="dot" aria-hidden="true">•</span><span class="subject"
            >{visibleText(b.subject)}</span
          >
        {/if}
      </div>
    {:else if row.kind === "head"}
      <div class="line one">
        <svg class="icon" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
          <circle cx="7" cy="7" r="3.1" fill="none" stroke="currentColor" stroke-width="1.6" />
        </svg>
        <span class="name">{@render highlighted("HEAD")}</span>
        <span class="hash">{shortHash(row.hash)}</span>
        {#if row.branch && row.branch.time > 0}
          <span class="when">{relativeTime(row.branch.time * 1000, now)}</span>
        {/if}
      </div>
      <div class="line two">
        {#if row.branch}
          <span class="author">{visibleText(row.branch.author)}</span><span class="dot" aria-hidden="true">•</span><span class="subject"
            >{visibleText(row.branch.subject)}</span
          >
        {:else}
          <span class="subject">The commit checked out (HEAD is detached)</span>
        {/if}
      </div>
    {:else if row.kind === "create"}
      {@const detail = createDetail(row)}
      <div class="line one">
        {@render plusIcon()}
        <span class="action">{row.name === "" ? "Create new branch…" : `Create new branch ‘${visibleText(row.name)}’`}</span>
        {#if working}
          <span class="when working">{@render spinner()}{pendingLabel}</span>
        {/if}
      </div>
      <div class="line two" class:problem={detail.problem}><span class="subject">{detail.text}</span></div>
    {:else}
      <div class="line one">
        {@render plusIcon()}
        <span class="action">Create new branch from…</span>
      </div>
      <div class="line two">
        <span class="subject"
          >{row.name === "" ? "Choose the commit it starts at: HEAD or a branch" : `Choose where ‘${visibleText(row.name)}’ starts: HEAD or a branch`}</span
        >
      </div>
    {/if}
  </div>
{/snippet}

<!-- Over the window, which takes no input while it is open; a press outside closes it. -->
<div
  class="backdrop"
  role="presentation"
  onpointerdown={(e) => {
    if (e.target === e.currentTarget && !named.pending) close();
  }}
>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="picker" role="dialog" aria-modal="true" aria-label="Switch branch" tabindex="-1" data-testid="branch-picker" onkeydown={onKeydown}>
    {#if prompt !== null}
      <p class="prompt" id="{uid}-prompt">{prompt}</p>
    {/if}
    <div class="query" class:refused={problem !== null}>
      {#if step.kind !== "list"}
        <button
          class="back"
          aria-label="Back"
          title="Back (Esc)"
          tabindex="-1"
          disabled={named.pending}
          onpointerdown={(e) => e.preventDefault()}
          onclick={back}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
            <path d="M7.4 2.6 4 6l3.4 3.4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </button>
      {:else}
        {@render branchIcon()}
      {/if}
      <input
        type="text"
        role="combobox"
        aria-label={prompt ?? placeholder}
        aria-expanded="true"
        aria-controls="{uid}-list"
        aria-autocomplete="list"
        aria-activedescendant={activeId}
        aria-invalid={problem !== null}
        aria-describedby={problem !== null ? `${uid}-problem` : undefined}
        {placeholder}
        spellcheck="false"
        autocomplete="off"
        readonly={named.pending}
        bind:this={input}
        bind:value={text}
        oninput={onInput}
      />
    </div>
    {#if problem !== null}
      <p class="problem-line" id="{uid}-problem" role="alert">{problem}</p>
    {/if}
    <div
      class="list"
      id="{uid}-list"
      role="listbox"
      aria-label={step.kind === "list" ? "Branches" : step.kind === "from" ? "Where the new branch starts" : "New branch"}
      tabindex="-1"
      bind:this={listEl}
      bind:clientHeight={listH}
      onscroll={() => (listTop = listEl!.scrollTop)}
    >
      <div class="space" style:height="{list.length * ROW_H}px">
        {#each rows as { row, i } (rowKey(row))}
          {@render entry(row, i)}
        {/each}
      </div>
    </div>
    {#if list.length === 0}
      <p class="none">{typed === "" ? "No branches yet" : `No branch matches ‘${visibleText(typed)}’`}</p>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: calc(var(--titlebar-h) + 6px) 16px 16px;
  }

  .picker {
    display: flex;
    flex-direction: column;
    width: min(600px, 100%);
    max-height: calc(100vh - var(--titlebar-h) - 24px);
    padding: 6px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    background: var(--bg-elevated);
    box-shadow: var(--shadow-menu);
    outline: none;
    animation: drop 120ms var(--ease);
  }

  .query {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--field-bg);
    color: var(--text-faint);
  }

  .query:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .query.refused,
  .query.refused:focus-within {
    border-color: var(--danger);
    box-shadow: 0 0 0 3px var(--danger-tint);
  }

  .query input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
  }

  .query input::placeholder {
    color: var(--text-faint);
  }

  .back {
    display: grid;
    place-items: center;
    flex: none;
    width: 20px;
    height: 20px;
    margin-left: -4px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
  }

  .back:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .prompt {
    flex: none;
    padding: 2px 4px 7px;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    font-weight: 500;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .problem-line {
    flex: none;
    padding: 6px 4px 0;
    color: var(--danger);
    font-size: var(--fs-sm);
  }

  .list {
    position: relative;
    flex: 0 1 auto;
    min-height: 0;
    max-height: calc(10 * 42px);
    margin-top: 6px;
    overflow-y: auto;
    overscroll-behavior: contain;
    outline: none;
  }

  .space {
    position: relative;
  }

  .option {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 1px;
    height: 42px;
    padding: 0 10px;
    border-radius: var(--radius);
  }

  /* Between the groups: a hairline, as between a menu's sections. */
  .option.separated::before {
    content: "";
    position: absolute;
    top: 0;
    left: 6px;
    right: 6px;
    border-top: 1px solid var(--border);
  }

  .option[data-active="true"] {
    background: var(--bg-selected);
  }

  .option[data-active="true"]::before,
  .option[data-active="true"] + .option::before {
    border-top-color: transparent;
  }

  .option:active {
    background: var(--bg-pressed);
  }

  .line {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    white-space: nowrap;
  }

  .line.one {
    font-size: var(--fs);
  }

  .line.two {
    /* Under the name, past the icon. */
    padding-left: 21px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  .line.two.problem {
    color: var(--danger);
  }

  .icon {
    flex: none;
    color: var(--text-muted);
  }

  [data-kind="create"] .icon,
  [data-kind="createFrom"] .icon {
    color: var(--accent);
  }

  .name,
  .action {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .name.current {
    font-weight: 600;
  }

  .action {
    font-weight: 500;
  }

  .head {
    flex: none;
    color: var(--accent);
  }

  .hash {
    flex: none;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .chip {
    flex: none;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    padding: 0 6px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 500;
    line-height: 16px;
  }

  .when {
    flex: none;
    color: var(--text-faint);
    font-size: var(--fs-sm);
  }

  .when.working {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--accent);
  }

  .spinner {
    animation: spin 0.8s linear infinite;
  }

  .group-label {
    flex: none;
    margin-left: auto;
    padding-left: 8px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .author {
    flex: 0 1 auto;
    min-width: 0;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dot {
    flex: none;
    color: var(--text-faint);
  }

  .subject {
    flex: 1 1 0;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  mark {
    border-radius: 2px;
    background: var(--hit-mark);
    color: var(--hit-mark-text);
  }

  .none {
    padding: 8px 10px 6px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
</style>
