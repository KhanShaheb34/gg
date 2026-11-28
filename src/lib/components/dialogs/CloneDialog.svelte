<!--
  Clone a repository: by its link (an https or ssh address, or owner/name for GitHub, read by
  the core as it is typed), or one of your repositories as gh lists them (filtered as you type,
  ↑ ↓ and Enter; kept for the session). Or, from a pull request's page, that pull request's
  repository alone. A repository gg already has (open, or among the recent ones, by its
  remotes) is switched to or opened instead. Then where it goes: the folder (from Settings ›
  Cloning, or chosen) and the new folder's name (the repository's, editable), with the path it
  makes, refused before anything starts when something not empty is there. While git clones,
  its own progress (the phase, the percent) and Stop, which takes away only what the clone
  made. Done, the repository opens in a tab of its own and joins the recent ones; a failure
  says what happened and leaves everything as typed for another try.
-->
<script module lang="ts">
  /** The way the dialog last showed, for this session: it opens on it again. */
  let last: "link" | "mine" = "mine";
  function lastWay(): "link" | "mine" {
    return last;
  }
  function rememberWay(way: "link" | "mine") {
    last = way;
  }
</script>

<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import { expandHome, filterRepos, knownFor, LIST_PROBLEM, listCaption, readProgress } from "../../clone";
  import { relativeTime, shortenPath, visibleText } from "../../format";
  import { api, onEvent } from "../../ipc";
  import { moveTo } from "../../inbox";
  import { app, openPath } from "../../state/app.svelte";
  import { clonesGoTo, goToKnown, loadRepos, repoList, type CloneStart } from "../../state/clone.svelte";
  import { closeInbox } from "../../state/inbox.svelte";
  import type { CloneProgress, ClonedLink, DestCheck, KnownRepo, ListedRepo, Notice } from "../../types";
  import Dialog from "../Dialog.svelte";
  import NoticeText from "../NoticeText.svelte";
  import Segmented from "../Segmented.svelte";

  let { start, close }: { start: CloneStart; close: () => void } = $props();

  // svelte-ignore state_referenced_locally
  const fixed = start.repo ?? null;
  const uid = `clone-${Math.random().toString(36).slice(2, 8)}`;
  const cloneId = `clone-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  /** Relative times are as of the dialog's opening. */
  const now = Date.now();

  // svelte-ignore state_referenced_locally
  let way = $state<"link" | "mine">(start.way ?? (start.link || repoList.list?.problem === "no_gh" ? "link" : lastWay()));

  // --- The link ---------------------------------------------------------------------------
  // svelte-ignore state_referenced_locally
  let link = $state(start.link ?? "");
  let linkRead = $state<ClonedLink | null>(null);
  let linkAsked = 0;
  let linkTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const text = link.trim();
    const asked = ++linkAsked;
    clearTimeout(linkTimer);
    if (text === "") {
      linkRead = null;
      return;
    }
    linkTimer = setTimeout(() => {
      api.cloneLink(text).then(
        (read) => asked === linkAsked && (linkRead = read),
        () => {},
      );
    }, 60);
  });

  // --- Your repositories --------------------------------------------------------------------
  let filter = $state("");
  let active = $state(0);
  let listEl = $state<HTMLElement>();
  const shownRepos = $derived(filterRepos(repoList.list?.repos ?? [], filter));
  const chosen = $derived<ListedRepo | null>(shownRepos[Math.min(active, shownRepos.length - 1)] ?? null);
  $effect(() => {
    void filter;
    active = 0;
  });

  /** The repositories gg has, by their remotes (asked once, as the dialog opens). */
  let known = $state<KnownRepo[]>([]);

  onMount(() => {
    void api.cloneKnown().then(
      (k) => (known = k),
      () => {},
    );
    if (!fixed) void loadRepos();
  });

  // --- What is cloned ---------------------------------------------------------------------
  /** The repository named, as host, owner and name, when there is one to match against what gg has. */
  const target = $derived.by(() => {
    if (fixed) return fixed;
    if (way === "mine") return chosen;
    const l = linkRead?.link;
    return l && l.host && l.owner && l.name ? { host: l.host, owner: l.owner, name: l.name } : null;
  });
  const hit = $derived(target ? knownFor(known, target.host, target.owner, target.name) : null);
  /** The folder name the repository suggests. */
  const suggested = $derived(fixed ? fixed.name : way === "mine" ? (chosen?.name ?? "") : (linkRead?.link?.folder ?? ""));
  /** What the core clones from: a repository, or the link as typed; null while there is none. */
  const from = $derived.by(() => {
    if (fixed) return fixed;
    if (way === "mine") return chosen ? { host: chosen.host, owner: chosen.owner, name: chosen.name } : null;
    return linkRead?.link ? { link: link.trim() } : null;
  });

  // --- Where it goes ----------------------------------------------------------------------
  /** The folder as the field shows it (the home folder as ~), and as the core takes it. */
  let parentText = $state(shortenPath(clonesGoTo(), app.homeDir));
  const parent = $derived(expandHome(parentText, app.homeDir));
  let folder = $state("");
  /** The user typed the folder's name: the repository's no longer replaces it. */
  let folderTouched = $state(false);
  $effect(() => {
    const name = suggested;
    if (!untrack(() => folderTouched)) folder = name;
  });
  let dest = $state<DestCheck | null>(null);
  /** The folder and name `dest` was asked for. */
  let destFor = $state("");
  let destAsked = 0;
  let destTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const [p, f] = [parent.trim(), folder.trim()];
    const asked = ++destAsked;
    clearTimeout(destTimer);
    if (p === "" || f === "") {
      dest = null;
      return;
    }
    destTimer = setTimeout(() => {
      api.cloneDest(p, f).then(
        (d) => {
          if (asked !== destAsked) return;
          dest = d;
          destFor = `${p}\n${f}`;
        },
        () => {},
      );
    }, 80);
  });
  onDestroy(() => {
    clearTimeout(linkTimer);
    clearTimeout(destTimer);
  });

  async function choose() {
    let picked: string | null;
    try {
      picked = await api.pickFolder({ title: "Choose where to clone", defaultPath: parent.trim() || undefined });
    } catch (e) {
      failed = { title: [{ text: "Could not show the folder picker" }], note: [{ text: e instanceof Error ? e.message : String(e) }], detail: null };
      return;
    }
    if (picked) parentText = shortenPath(picked, app.homeDir);
  }

  // --- Cloning ------------------------------------------------------------------------------
  let running = $state(false);
  let line = $state("");
  const progress = $derived(readProgress(line));
  let failed = $state<{ title: Notice["title"]; note: Notice["note"]; detail: string | null } | null>(null);
  let stopped = $state(false);

  // The primary button is disabled while git runs and the fields go read-only: the focus stays
  // in the dialog (on Stop while it runs, on Clone after), so Esc and Enter keep working.
  let frame = $state<{ keepFocus: () => void }>();
  $effect(() => {
    void running;
    // A list being asked for again replaces the button that asked (Try again, the list's
    // refresh): the filter takes the focus back.
    void repoList.loading;
    void tick().then(() => {
      const lost = document.activeElement === null || document.activeElement === document.body;
      const filterField = document.getElementById(`${uid}-filter`);
      if (lost && filterField && !running) filterField.focus();
      else frame?.keepFocus();
    });
  });

  const stopEvents = onEvent<CloneProgress>("clone-progress", (p) => {
    if (p.cloneId === cloneId) line = p.line;
  });
  onDestroy(stopEvents);

  /** The folder and name as typed, as `destFor` names them. */
  const destNow = $derived(`${parent.trim()}\n${folder.trim()}`);
  /** It can be cloned, as far as known: a source, a folder, and no refusal for this one (one being checked is checked again on submit). */
  const ready = $derived(
    hit !== null || (from !== null && parent.trim() !== "" && folder.trim() !== "" && !(destFor === destNow && dest?.problem)),
  );

  /** Asks the core about the folder now, unless its answer for it is in; whether it can be cloned into. */
  async function destOk(): Promise<boolean> {
    if (destFor !== destNow) {
      const [p, f] = [parent.trim(), folder.trim()];
      const asked = ++destAsked;
      clearTimeout(destTimer);
      try {
        const d = await api.cloneDest(p, f);
        if (asked === destAsked) {
          dest = d;
          destFor = `${p}\n${f}`;
        }
      } catch {
        return false;
      }
    }
    return destFor === destNow && dest !== null && dest.problem === null;
  }

  async function submit() {
    if (running) return;
    if (hit) {
      close();
      await goToKnown(hit);
      return;
    }
    if (!ready || from === null) return;
    running = true;
    if (!(await destOk())) {
      running = false;
      return;
    }
    failed = null;
    stopped = false;
    line = "";
    let cloned;
    try {
      cloned = await api.cloneRepo(cloneId, from, parent.trim(), folder.trim());
    } catch (e) {
      running = false;
      failed = { title: [{ text: e instanceof Error ? e.message : String(e) }], note: [], detail: null };
      return;
    }
    running = false;
    if (cloned.status === "done" && cloned.path) {
      close();
      if (app.inboxOpen) closeInbox();
      await openPath(cloned.path);
      return;
    }
    if (cloned.status === "stopped") {
      stopped = true;
      line = "";
      return;
    }
    failed = cloned.notice
      ? { title: cloned.notice.title, note: cloned.notice.note, detail: cloned.detail }
      : { title: [{ text: cloned.message }], note: [], detail: cloned.detail };
  }

  function cancel() {
    if (running) {
      void api.cloneCancel(cloneId).catch(() => {});
      return;
    }
    close();
  }

  function pickWay(w: "link" | "mine") {
    way = w;
    rememberWay(w);
    failed = null;
    stopped = false;
    void tick().then(() => document.getElementById(w === "link" ? `${uid}-link` : `${uid}-filter`)?.focus());
  }

  function onFilterKeydown(e: KeyboardEvent) {
    if (e.isComposing || !["ArrowDown", "ArrowUp", "PageDown", "PageUp"].includes(e.key)) return;
    const next = moveTo(active, e.key, shownRepos.length, 5);
    if (next === null) return;
    e.preventDefault();
    active = next;
    void tick().then(() => listEl?.querySelector(`#${uid}-repo-${next}`)?.scrollIntoView({ block: "nearest" }));
  }

  const primary = $derived(running ? "Cloning…" : hit ? (hit.repoId !== null ? "Switch to it" : "Open it") : "Clone");
  const title = $derived(fixed ? `Clone ${fixed.owner}/${fixed.name}` : "Clone a repository");
  const list = $derived(repoList.list);
</script>

{#snippet lock()}
  <svg class="lock" width="11" height="11" viewBox="0 0 12 12" aria-label="Private" role="img">
    <rect x="2.2" y="5.2" width="7.6" height="5.4" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.2" />
    <path d="M3.9 5.2V3.8a2.1 2.1 0 0 1 4.2 0v1.4" fill="none" stroke="currentColor" stroke-width="1.2" />
  </svg>
{/snippet}

<Dialog
  bind:this={frame}
  {title}
  {primary}
  wide
  valid={!running && ready}
  once={false}
  cancelLabel={running ? "Stop" : "Cancel"}
  onSubmit={() => void submit()}
  onCancel={cancel}
>
  {#if !fixed}
    <div class="ways">
      <Segmented
        label="Clone"
        testid="clone-way"
        value={way}
        options={[
          { value: "mine", label: "Your repositories", title: "The repositories gh lists for your account" },
          { value: "link", label: "Link", title: "An https or ssh address, or owner/name for GitHub" },
        ]}
        onchange={pickWay}
      />
    </div>
  {/if}

  {#if !fixed && way === "link"}
    <div class="field">
      <label class="field-label" for="{uid}-link">Link</label>
      <input
        id="{uid}-link"
        class="text-input mono-input"
        type="text"
        placeholder="https://github.com/octo/cat or octo/cat"
        bind:value={link}
        readonly={running}
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        aria-invalid={linkRead?.problem ? true : undefined}
        aria-describedby="{uid}-link-hint"
        data-autofocus
        data-testid="clone-link"
      />
      <p class="field-hint" class:problem={!!linkRead?.problem} id="{uid}-link-hint" data-testid="clone-link-hint">
        {#if link.trim() === ""}
          An https or ssh address, or owner/name for GitHub.
        {:else if linkRead?.problem}
          {linkRead.problem}
        {:else if linkRead?.link}
          {@const l = linkRead.link}
          {#if l.owner && l.name}
            {l.owner}/{l.name} on {l.host}{l.shorthand ? ", by the protocol gh uses (https without gh)" : ""}
          {:else}
            A repository named {l.folder}
          {/if}
        {/if}
      </p>
    </div>
  {:else if !fixed}
    <div class="field mine">
      <label class="sr-only" for="{uid}-filter">Filter your repositories</label>
      <div class="filter">
        <svg width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="7" cy="7" r="4.6" fill="none" stroke="currentColor" stroke-width="1.5" />
          <path d="M10.5 10.5 14 14" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
        </svg>
        <input
          id="{uid}-filter"
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls="{uid}-repos"
          aria-autocomplete="list"
          aria-activedescendant={chosen ? `${uid}-repo-${active}` : undefined}
          placeholder={list?.login ? `Filter ${list.login}’s repositories` : "Filter your repositories"}
          bind:value={filter}
          readonly={running}
          autocomplete="off"
          spellcheck="false"
          data-autofocus
          data-testid="clone-filter"
          onkeydown={onFilterKeydown}
        />
        <button
          type="button"
          class="icon-btn small"
          aria-label="List them again"
          title="List them again"
          aria-disabled={repoList.loading || running}
          data-testid="clone-reload"
          onclick={() => {
            if (!running) void loadRepos(true);
          }}
        >
          <svg class:spinning={repoList.loading} width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
            <path d="M13 8a5 5 0 1 1-1.46-3.54M13 2.8v2.9h-2.9" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </button>
      </div>
      {#if list && list.problem === null}
        <!-- The keys are the filter's (aria-activedescendant); rows take clicks. -->
        <div class="repos" id="{uid}-repos" role="listbox" aria-label="Your repositories" tabindex="-1" bind:this={listEl} data-testid="clone-repos">
          {#each shownRepos as repo, i (`${repo.owner}/${repo.name}`)}
            {@const has = knownFor(known, repo.host, repo.owner, repo.name)}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="repo"
              id="{uid}-repo-{i}"
              role="option"
              tabindex="-1"
              aria-selected={i === active}
              data-active={i === active}
              data-repo="{repo.owner}/{repo.name}"
              onpointerdown={(e) => e.preventDefault()}
              onclick={() => {
                if (!running) active = i;
              }}
              ondblclick={() => {
                if (!running) {
                  active = i;
                  void submit();
                }
              }}
            >
              <div class="line one">
                <span class="repo-name"><span class="owner">{repo.owner}/</span>{repo.name}</span>
                {#if repo.private}{@render lock()}{/if}
                {#if repo.fork}<span class="tag">fork</span>{/if}
                {#if repo.archived}<span class="tag">archived</span>{/if}
                {#if has}
                  <span class="have" data-testid="clone-have" title={shortenPath(has.path, app.homeDir)}>{has.repoId !== null ? "Open" : "Cloned"}</span>
                {/if}
                {#if repo.pushedAtMs !== null}<span class="when">{relativeTime(repo.pushedAtMs, now)}</span>{/if}
              </div>
              {#if repo.description}<div class="line two">{visibleText(repo.description)}</div>{/if}
            </div>
          {/each}
          {#if shownRepos.length === 0}
            <p class="none" data-testid="clone-none">
              {list.repos.length === 0
                ? `No repositories on ${list.host}${list.login ? ` for ${list.login}` : ""}.`
                : `None of them match ‘${visibleText(filter.trim())}’.`}
              <button type="button" class="link-btn" onclick={() => pickWay("link")}>Clone by its link</button>
            </p>
          {/if}
        </div>
        <p class="caption" data-testid="clone-caption">{listCaption(list.repos.length, list.total)}</p>
      {:else if list && list.problem !== null && !repoList.loading}
        <div class="state" data-testid="clone-list-state" data-state={list.problem}>
          <p class="lead">{LIST_PROBLEM[list.problem]}</p>
          <p>{list.message}</p>
          {#if list.detail}
            <details class="detail">
              <summary>What gh said</summary>
              <pre>{list.detail}</pre>
            </details>
          {/if}
          <p class="aside">A link still clones, with git alone.</p>
          <div class="state-actions">
            <button type="button" class="link-btn" onclick={() => pickWay("link")}>Clone by its link</button>
            {#if list.problem === "no_gh"}
              <button type="button" class="link-btn" onclick={() => void api.openWeb("link", "https://cli.github.com").catch(() => {})}>Get the GitHub CLI</button>
            {/if}
            <button type="button" class="link-btn" data-testid="clone-list-retry" onclick={() => void loadRepos(true)}>Try again</button>
          </div>
        </div>
      {:else}
        <div class="repos skeleton" aria-hidden="true" data-testid="clone-loading">
          {#each [62, 48, 70, 55, 40] as w (w)}
            <div class="bone-row"><span class="bone" style:width="{w}%"></span><span class="bone short"></span></div>
          {/each}
        </div>
        <p class="caption"><span role="status">Listing your repositories…</span></p>
      {/if}
    </div>
  {/if}

  {#if hit}
    <div class="have-note" role="status" data-testid="clone-known">
      <strong>{target?.owner}/{target?.name}</strong>
      {hit.repoId !== null ? "is open in gg" : "is one of your recent repositories"}, in
      <code>{shortenPath(hit.path, app.homeDir)}</code>. {hit.repoId !== null ? "Switch to its tab" : "Open it"} instead of cloning it again.
    </div>
  {:else}
    <div class="dest" class:quiet={from === null && !fixed}>
      <div class="dest-row">
        <div class="field grow">
          <label class="field-label" for="clone-parent">Into</label>
          <div class="pick">
            <input
              id="clone-parent"
              class="text-input mono-input"
              type="text"
              bind:value={parentText}
              readonly={running}
              autocomplete="off"
              autocapitalize="off"
              spellcheck="false"
              data-testid="clone-parent"
            />
            <button class="button choose" type="button" disabled={running} onclick={() => void choose()}>Choose…</button>
          </div>
        </div>
        <div class="field name">
          <label class="field-label" for="clone-folder">Folder name</label>
          <input
            id="clone-folder"
            class="text-input mono-input"
            type="text"
            bind:value={folder}
            oninput={() => (folderTouched = true)}
            readonly={running}
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
            aria-invalid={dest?.problem ? true : undefined}
            data-testid="clone-folder"
          />
        </div>
      </div>
      {#if running}
        <div class="progress" data-testid="clone-progress" aria-live="polite">
          <div class="phase">
            <span>{progress ? visibleText(progress.phase) : "Starting git…"}</span>
            {#if progress?.percent != null}<span class="pct">{progress.percent}%</span>{/if}
          </div>
          <div class="bar" class:indefinite={progress?.percent == null}>
            <span style:width={progress?.percent != null ? `${progress.percent}%` : undefined}></span>
          </div>
          <p class="rest">{progress?.rest ? visibleText(progress.rest) : " "}</p>
        </div>
      {:else}
        <p class="field-hint" class:problem={!!dest?.problem} aria-live="polite" data-testid="clone-hint">
          {#if parent.trim() === ""}
            Choose the folder to clone into.
          {:else if folder.trim() === ""}
            {from === null && !fixed ? (way === "mine" ? "Choose a repository." : "Paste a link.") : "Name the folder."}
          {:else if dest?.problem}
            {dest.problem}
          {:else if dest}
            <span class="arrow" aria-hidden="true">→</span>
            <code class="path">{shortenPath(dest.path, app.homeDir)}</code>{#if dest.makesParent}<span class="made"
                >{` · ${shortenPath(parent.trim(), app.homeDir)} is made for it`}</span
              >{/if}
          {/if}
        </p>
      {/if}
    </div>
  {/if}

  {#if failed}
    <div class="failure" role="alert" data-testid="clone-failure">
      <p class="headline"><NoticeText line={failed.title} /></p>
      {#if failed.note.length > 0}<p class="note"><NoticeText line={failed.note} /></p>{/if}
      {#if failed.detail}
        <details class="detail">
          <summary>git’s output</summary>
          <pre>{failed.detail}</pre>
        </details>
      {/if}
    </div>
  {:else if stopped}
    <p class="stopped" role="status" data-testid="clone-stopped">Stopped. What the clone had made is gone.</p>
  {/if}
</Dialog>

<style>
  .ways {
    display: flex;
    margin-top: 2px;
  }

  .mono-input {
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .field.mine {
    gap: 0;
  }

  .filter {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 32px;
    padding: 0 4px 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius) var(--radius) 0 0;
    background: var(--field-bg);
    color: var(--text-faint);
  }

  .filter:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
    position: relative;
    z-index: 1;
  }

  .filter input {
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

  .filter input::placeholder {
    color: var(--text-faint);
  }

  .icon-btn.small {
    width: 24px;
    height: 24px;
  }

  .repos {
    height: 236px;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 4px;
    border: 1px solid var(--border-strong);
    border-top: 0;
    border-radius: 0 0 var(--radius) var(--radius);
    background: var(--bg);
    outline: none;
  }

  .repo {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 1px;
    min-height: 44px;
    padding: 5px 9px;
    border-radius: var(--radius);
  }

  .repo[data-active="true"] {
    background: var(--bg-selected);
  }

  .repo:active {
    background: var(--bg-pressed);
  }

  .line {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    white-space: nowrap;
  }

  .repo-name {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 550;
  }

  .owner {
    color: var(--text-muted);
    font-weight: 450;
  }

  .lock {
    flex: none;
    color: var(--text-muted);
  }

  .tag {
    flex: none;
    padding: 0 5px;
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 15px;
  }

  .have {
    flex: none;
    padding: 0 6px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 550;
    line-height: 16px;
  }

  .when {
    flex: none;
    margin-left: auto;
    padding-left: 8px;
    color: var(--text-faint);
    font-size: var(--fs-sm);
  }

  .line.two {
    display: block;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    text-overflow: ellipsis;
  }

  .none {
    padding: 18px 10px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    text-align: center;
  }

  .caption {
    padding: 6px 2px 0;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .link-btn {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
  }

  .link-btn:hover {
    text-decoration: underline;
  }

  .skeleton {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 12px 10px;
  }

  .bone-row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }

  .bone {
    height: 10px;
    border-radius: 5px;
    background: var(--bg-hover);
    animation: pulse 1.2s ease-in-out infinite;
  }

  .bone.short {
    width: 52px;
  }

  .state {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-height: 236px;
    padding: 18px 16px;
    border: 1px solid var(--border-strong);
    border-top: 0;
    border-radius: 0 0 var(--radius) var(--radius);
    background: var(--bg);
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  .state .lead {
    color: var(--text);
    font-size: var(--fs);
    font-weight: 600;
  }

  .state .aside {
    margin-top: 6px;
  }

  .state-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 14px;
    margin-top: 8px;
  }

  .have-note {
    margin-top: 14px;
    padding: 10px 12px;
    border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--border));
    border-radius: 8px;
    background: var(--bg-selected);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }

  .have-note code,
  .path {
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .dest.quiet {
    opacity: 0.55;
  }

  .dest-row {
    display: flex;
    gap: 10px;
  }

  .grow {
    flex: 1 1 auto;
    min-width: 0;
  }

  .field.name {
    flex: 0 0 150px;
  }

  .pick {
    display: flex;
    gap: 6px;
  }

  .pick .text-input {
    flex: 1 1 auto;
    min-width: 0;
  }

  .choose {
    flex: none;
    height: 30px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
  }

  .choose:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .choose:disabled {
    color: var(--text-faint);
  }

  .field-hint {
    margin-top: 6px;
  }

  .arrow {
    color: var(--text-faint);
  }

  .made {
    color: var(--text-faint);
  }

  .progress {
    margin-top: 10px;
  }

  .phase {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    font-size: var(--fs-sm);
    font-weight: 550;
  }

  .pct {
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }

  .bar {
    position: relative;
    height: 4px;
    margin-top: 6px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--bg-hover);
  }

  .bar span {
    display: block;
    height: 100%;
    border-radius: 2px;
    background: var(--accent);
    transition: width 120ms linear;
  }

  .bar.indefinite span {
    width: 30%;
    animation: slide 1.1s var(--ease) infinite;
  }

  .rest {
    min-height: 16px;
    margin-top: 5px;
    color: var(--text-faint);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .failure {
    margin-top: 12px;
    padding: 10px 12px;
    border: 1px solid color-mix(in srgb, var(--danger) 35%, var(--border));
    border-radius: 8px;
    background: var(--danger-tint);
    font-size: var(--fs-sm);
  }

  .failure .headline {
    color: var(--danger);
    font-weight: 600;
  }

  .failure .note {
    margin-top: 2px;
    color: var(--text);
  }

  .stopped {
    margin-top: 10px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  .detail {
    margin-top: 6px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }

  .detail pre {
    max-height: 140px;
    margin: 6px 0 0;
    overflow: auto;
    font-family: var(--font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .spinning {
    animation: spin 0.8s linear infinite;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  @keyframes pulse {
    50% {
      opacity: 0.5;
    }
  }

  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
