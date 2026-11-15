<!--
  The command log (spec §5.7): every git command the app ran for this repository, newest
  last, so the user can see how and why things work. By default the operations they asked
  for, each under its title with its commands; "Background reads" shows the reads the app
  makes by itself too. A command shows its time, its command line (quoted as a shell would
  need it; Copy copies it), how long it took and its exit code; opened, git's output (stdout
  of a write, stderr always), with progress lines shown as they ended. "Show log" on a notice
  opens the panel at that operation, highlighted, its failed commands opened.

  Docked at the bottom of the repository view, below the details panel, across the whole
  width; resizable from its top edge. It asks the core only while it is open (see
  CommandLogStore). It renders the newest RENDERED entries (more on request) and the first
  OUTPUT_LINES lines of an output (all on request), so a large log or a 16 KiB output never
  stalls the window. All text is shown as text, with control characters visible and each
  command line in a bidi isolate.

  Keyboard: the entries are one tab stop (arrow keys move, Home and End jump, Enter or Space
  opens and closes, C copies); Escape closes the panel and puts the focus back where it was.
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import { LOG_MIN_H, panelHeight } from "../detailsLayout";
  import { clock } from "../state/prs.svelte";
  import { commandLine, formatClock, formatDuration, outputLines, shortenPath, visibleText } from "../format";
  import { isMac } from "../platform";
  import { app, toast } from "../state/app.svelte";
  import {
    commandLog,
    focusCameFrom,
    logPrefs,
    saveLogPrefs,
    stoppedOps,
    takeLogAutofocus,
    type OpEntry,
  } from "../state/commandLog.svelte";
  import type { CommandRecord } from "../types";
  import Splitter from "./Splitter.svelte";

  type Props = {
    repoId: number;
    /** The repository view's height, px. */
    hostH: number;
    /** The most this panel may take, px. */
    room: number;
    /** Closes the panel (Escape, its close button). */
    onclose: () => void;
    /**
     * A commit is selected but the window is too short for its details beside the log
     * (detailsRoom): the header says so, and how to get them back.
     */
    detailsHidden?: boolean;
  };

  let { repoId, hostH, room, onclose, detailsHidden = false }: Props = $props();

  const mac = isMac();
  const KEYS = mac ? "⌘⇧L" : "Ctrl+Shift+L";

  /** Entries rendered at first, and added by "Show earlier". */
  const RENDERED = 200;
  /** Output lines shown before "Show all". */
  const OUTPUT_LINES = 200;
  const DEFAULT_H = 220;
  const DEFAULT_SHARE = 0.35;

  // --- Data: this repository, polled while the panel is open ------------------------------

  $effect(() => {
    commandLog.show(repoId, !logPrefs.reads);
    // Starting again for another repository or filter; stopping when the panel closes.
    return untrack(() => commandLog.start());
  });

  let limit = $state(RENDERED);
  const items = $derived(commandLog.items);
  const hidden = $derived(Math.max(0, items.length - limit));
  const shownItems = $derived(hidden > 0 ? items.slice(hidden) : items);

  // --- Size --------------------------------------------------------------------------------

  const height = $derived(
    panelHeight(logPrefs.height ?? Math.min(DEFAULT_H, Math.round(hostH * DEFAULT_SHARE)), LOG_MIN_H, room),
  );
  let resizeFrom = 0;
  const resizer = {
    onstart: () => (resizeFrom = height),
    onmove: (dy: number) =>
      (logPrefs.height = Math.max(LOG_MIN_H, Math.min(Math.max(room, LOG_MIN_H), Math.round(resizeFrom - dy)))),
    onend: saveLogPrefs,
    onreset: () => {
      logPrefs.height = null;
      saveLogPrefs();
    },
  };

  // Notices stand above the panel while it is open (Toast.svelte reads --log-h).
  $effect(() => {
    const root = document.documentElement;
    root.style.setProperty("--log-h", `${height}px`);
    return () => root.style.removeProperty("--log-h");
  });

  function setReads(reads: boolean) {
    logPrefs.reads = reads;
    saveLogPrefs();
    limit = RENDERED;
  }

  // --- What is open ------------------------------------------------------------------------

  /** Operations whose commands are folded away (they show by default). */
  let folded = $state<Record<number, boolean>>({});
  /** Commands whose output shows. */
  let opened = $state<Record<number, boolean>>({});
  /** Commands whose whole output shows (past OUTPUT_LINES). */
  let whole = $state<Record<number, boolean>>({});
  /** The operation (or command) "Show log" went to. */
  let target = $state<string | null>(null);

  const failed = (r: CommandRecord) => r.exitCode !== 0;
  /** The operation ended stopped on conflicts: a git command of it exiting non-zero is how. */
  const stopped = (opId: number | null) => opId !== null && stoppedOps.ids.has(opId);
  const hasOutput = (r: CommandRecord) => r.stdout !== "" || r.stderr !== "";

  // --- Scrolling: newest last, the bottom followed while the user is there -------------------

  let scroller: HTMLElement | undefined = $state();
  /** The list shows its end: new entries keep it there. */
  let atEnd = true;
  /**
   * "Show log" went to an entry: the list stays there (not following new entries) until the
   * user is back at the end or a new operation comes in. The highlight stays either way.
   */
  let holding = false;
  /** The newest operation's key when "Show log" went to its entry. */
  let heldNewest: string | null = null;

  function onScroll() {
    if (!scroller) return;
    atEnd = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;
    if (atEnd) holding = false;
  }

  /** The newest operation's entry, if any. */
  const newestOp = $derived(items.findLast((i) => i.kind === "op")?.key ?? null);

  $effect(() => {
    void commandLog.count;
    void shownItems.length;
    if (holding && newestOp !== heldNewest) {
      holding = false;
      atEnd = true;
    }
    if (!scroller || !atEnd || holding) return;
    const el = scroller;
    void tick().then(() => (el.scrollTop = el.scrollHeight));
  });

  // --- "Show log" ----------------------------------------------------------------------------

  /** Goes to the entry holding command `id`, once it is fetched; false if it is not kept. */
  async function goTo(id: number): Promise<boolean> {
    if (commandLog.lastId < id || !commandLog.loaded) await commandLog.fetchNow();
    const entry = commandLog.find(id);
    if (!entry) return false;
    const at = items.indexOf(entry);
    if (at >= 0 && at < items.length - limit) limit = items.length - at;
    target = entry.key;
    holding = true;
    heldNewest = newestOp;
    atEnd = false;
    if (entry.kind === "op") {
      folded[entry.opId] = false;
      for (const r of entry.records) if (failed(r) && hasOutput(r)) opened[r.id] = true;
    } else if (hasOutput(entry.record)) {
      opened[entry.record.id] = true;
    }
    await tick();
    const el = scroller?.querySelector<HTMLElement>(`[data-key="${entry.key}"]`);
    el?.scrollIntoView({ block: "nearest" });
    el?.querySelector<HTMLElement>(".row")?.focus({ preventScroll: true });
    if (el) active = el.querySelector<HTMLElement>(".row")?.dataset.row ?? active;
    return true;
  }

  $effect(() => {
    const request = app.logRequest;
    if (!request || request.repoId !== repoId) return;
    untrack(() => {
      app.logRequest = null;
      void goTo(request.from).then((found) => {
        if (!found) toast("info", "That operation's commands are no longer in the log.");
      });
    });
  });

  // --- Keyboard ------------------------------------------------------------------------------

  /** The row that takes the tab stop (data-row of a .row button). */
  let active = $state<string | null>(null);
  let panel: HTMLElement | undefined = $state();

  function rows(): HTMLElement[] {
    return [...(scroller?.querySelectorAll<HTMLElement>(".row") ?? [])];
  }

  /** The keys of the rows rendered (a command's, or a folded operation's). */
  const renderedRows = $derived.by(() => {
    const keys = new Set<string>();
    for (const item of shownItems) {
      if (item.kind === "cmd") keys.add(rowKey(item.record));
      else {
        keys.add(item.key);
        if (!folded[item.opId]) for (const r of item.records) keys.add(rowKey(r));
      }
    }
    return keys;
  });
  /** The row with the tab stop: the active one if it is still there, else the newest. */
  const tabRow = $derived(active !== null && renderedRows.has(active) ? active : null);

  function onFocusIn(e: FocusEvent) {
    const from = e.relatedTarget as HTMLElement | null;
    if (from && panel && !panel.contains(from)) focusCameFrom(from);
    const row = (e.target as HTMLElement).closest<HTMLElement>(".row");
    if (row?.dataset.row) active = row.dataset.row;
  }

  /** Closes the panel; the focus goes back where it came from (see setLogOpen). */
  function close() {
    onclose();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    const row = (e.target as HTMLElement).closest<HTMLElement>(".row");
    if (!row) return;
    const all = rows();
    const at = all.indexOf(row);
    let next: HTMLElement | undefined;
    if (e.key === "ArrowDown") next = all[at + 1];
    else if (e.key === "ArrowUp") next = all[at - 1];
    else if (e.key === "Home") next = all[0];
    else if (e.key === "End") next = all[all.length - 1];
    else if (e.key === "c" && !e.metaKey && !e.ctrlKey && !e.altKey) {
      const id = Number(row.dataset.cmd);
      const record = Number.isFinite(id) ? findRecord(id) : null;
      if (record) {
        e.preventDefault();
        copy(record);
      }
      return;
    } else return;
    e.preventDefault();
    if (next) {
      next.focus();
      next.scrollIntoView({ block: "nearest" });
    }
  }

  function findRecord(id: number): CommandRecord | null {
    const entry = commandLog.find(id);
    if (!entry) return null;
    return entry.kind === "cmd" ? entry.record : (entry.records.find((r) => r.id === id) ?? null);
  }

  // Opened from the keyboard: to the newest entry once the first answer is in (the panel
  // itself until then); only when this opening asked for it, not when the panel mounts again
  // for another tab. Where the focus was is where closing puts it back (setLogOpen).
  $effect(() => {
    if (!untrack(takeLogAutofocus)) return;
    panel?.focus({ preventScroll: true });
    void commandLog.fetchNow().then(async () => {
      await tick();
      if (!panel?.contains(document.activeElement)) return;
      const all = rows();
      all[all.length - 1]?.focus({ preventScroll: true });
    });
  });

  // --- Copy ----------------------------------------------------------------------------------

  function copy(r: CommandRecord) {
    navigator.clipboard.writeText(commandLine(r.args, r.program)).then(
      () => toast("info", "Copied the command line."),
      () => toast("error", "Could not copy the command line."),
    );
  }

  // --- Words ---------------------------------------------------------------------------------

  const commands = (n: number) => `${n} command${n === 1 ? "" : "s"}`;

  function opSpan(op: OpEntry): number {
    const first = op.records[0];
    const last = op.records[op.records.length - 1];
    return last.startedAtMs + last.durationMs - first.startedAtMs;
  }

  /** The exit code of the operation's first write that failed, if any. */
  function opExit(op: OpEntry): number | null {
    return op.records.find((r) => r.kind === "write" && r.exitCode !== 0)?.exitCode ?? null;
  }

  const empty = $derived(commandLog.loaded && items.length === 0);
  const rowKey = (r: CommandRecord) => `c${r.id}`;
  const isTab = (key: string, fallback: boolean) => (tabRow === null ? fallback : tabRow === key);
  const lastKey = $derived.by(() => {
    const last = shownItems[shownItems.length - 1];
    if (!last) return null;
    if (last.kind === "cmd") return rowKey(last.record);
    return folded[last.opId] ? last.key : rowKey(last.records[last.records.length - 1]);
  });
</script>

{#snippet command(r: CommandRecord, inOp: boolean)}
  {@const open = !!opened[r.id]}
  {@const line = commandLine(r.args, r.program)}
  <div class="cmd" class:in-op={inOp} class:read={r.kind === "read"} data-key={inOp ? undefined : `c${r.id}`}>
    <button
      class="row"
      data-row={rowKey(r)}
      data-cmd={r.id}
      tabindex={isTab(rowKey(r), rowKey(r) === lastKey) ? 0 : -1}
      aria-expanded={hasOutput(r) ? open : undefined}
      aria-controls={open ? `out-${r.id}` : undefined}
      title={hasOutput(r) ? undefined : "No output"}
      onclick={() => hasOutput(r) && (opened[r.id] = !open)}
    >
      <span class="chev" class:open class:none={!hasOutput(r)} aria-hidden="true"></span>
      <span class="time">{formatClock(r.startedAtMs, app.settings.dateFormat, clock.now)}</span>
      <span class="line"><bdi>{visibleText(line)}</bdi></span>
      <span class="dur">{formatDuration(r.durationMs)}</span>
      <span class="exit" class:bad={failed(r) && !stopped(r.opId)}>{failed(r) ? `exit ${r.exitCode}` : "ok"}</span>
    </button>
    <button
      class="copy"
      tabindex={isTab(rowKey(r), rowKey(r) === lastKey) ? 0 : -1}
      aria-label="Copy command line"
      title="Copy the command line"
      onclick={() => copy(r)}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
        ><rect x="3.75" y="3.75" width="6.5" height="6.5" rx="1.4" fill="none" stroke="currentColor" stroke-width="1.1" /><path
          d="M8.25 2.4v-.15c0-.55-.45-1-1-1H2.6c-.55 0-1 .45-1 1v4.65c0 .55.45 1 1 1h.15"
          fill="none"
          stroke="currentColor"
          stroke-width="1.1"
          stroke-linecap="round"
        /></svg
      >
    </button>
    {#if open}
      <div class="out selectable" id="out-{r.id}" role="region" aria-label="Output of {visibleText(line)}">
        <div class="where">in <bdi>{visibleText(shortenPath(r.cwd, app.homeDir))}</bdi></div>
        {#each [{ name: "stdout", text: r.stdout }, { name: "stderr", text: r.stderr }] as stream (stream.name)}
          {#if stream.text !== ""}
            {@const lines = outputLines(stream.text)}
            {@const cut = !whole[r.id] && lines.length > OUTPUT_LINES}
            <div class="stream">
              <span class="label">{stream.name}</span>
              <pre class={stream.name}>{(cut ? lines.slice(0, OUTPUT_LINES) : lines).join("\n")}</pre>
              {#if cut}
                <button class="more" onclick={() => (whole[r.id] = true)}>Show all {lines.length} lines</button>
              {/if}
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
{/snippet}

<!-- The panel takes Escape (close) and the rows' arrow keys for everything inside it. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section
  class="log"
  aria-label="Command log"
  data-testid="command-log"
  style:height="{height}px"
  bind:this={panel}
  onfocusin={onFocusIn}
  onkeydown={onKeydown}
  tabindex="-1"
>
  <Splitter label="Resize command log" edge="top" value={height} {...resizer} />
  <header class="head">
    <h2 class="title">Command log</h2>
    <span class="count">{logPrefs.reads ? "All commands" : "Operations"}</span>
    <span class="spacer"></span>
    {#if detailsHidden}
      <span class="hidden-details" data-testid="details-hidden">Details hidden: make the window taller, or close the log</span>
    {/if}
    <label class="toggle">
      <input type="checkbox" class="checkbox" checked={logPrefs.reads} onchange={(e) => setReads(e.currentTarget.checked)} />
      <span>Background reads</span>
    </label>
    <button class="icon-btn close" aria-label="Close command log" title="Close ({KEYS} or Esc)" onclick={close}>
      <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
        <path d="M3.5 3.5l7 7M10.5 3.5l-7 7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
      </svg>
    </button>
  </header>

  <div class="scroller" bind:this={scroller} onscroll={onScroll}>
    {#if commandLog.error !== null && !commandLog.loaded}
      <p class="empty" role="alert">
        Could not read the command log: {visibleText(commandLog.error)}
        <button class="retry" onclick={() => void commandLog.fetchNow()}>Try again</button>
      </p>
    {:else if empty}
      <p class="empty">
        {logPrefs.reads
          ? "No commands yet."
          : "No operations yet. What you do in the graph shows here, with the git commands it ran and their output."}
      </p>
    {:else}
      {#if hidden > 0}
        <button class="earlier" onclick={() => (limit += RENDERED)}>Show {Math.min(hidden, RENDERED)} earlier</button>
      {/if}
      <ol class="entries" role="log" aria-live="off" aria-label="Commands, newest last">
        {#each shownItems as item (item.key)}
          {#if item.kind === "op"}
            {@const open = !folded[item.opId]}
            {@const exit = opExit(item)}
            <li class="op" class:target={target === item.key} data-key={item.key} data-op={item.opId}>
              <button
                class="row op-head"
                data-row={item.key}
                tabindex={isTab(item.key, item.key === lastKey) ? 0 : -1}
                aria-expanded={open}
                onclick={() => (folded[item.opId] = open)}
              >
                <span class="chev" class:open aria-hidden="true"></span>
                <span class="time">{formatClock(item.records[0].startedAtMs, app.settings.dateFormat, clock.now)}</span>
                <span class="op-title"><bdi>{visibleText(item.title ?? "Operation")}</bdi></span>
                <span class="meta">{commands(item.records.length)}</span>
                <span class="dur">{formatDuration(opSpan(item))}</span>
                {#if stopped(item.opId)}
                  <span class="exit stopped">stopped on conflicts</span>
                {:else}
                  <span class="exit" class:bad={exit !== null}>{exit !== null ? `exit ${exit}` : ""}</span>
                {/if}
              </button>
              {#if open}
                <div class="op-cmds">
                  {#each item.records as r (r.id)}
                    {@render command(r, true)}
                  {/each}
                </div>
              {/if}
            </li>
          {:else}
            <li class="single" class:target={target === item.key}>{@render command(item.record, false)}</li>
          {/if}
        {/each}
      </ol>
    {/if}
  </div>
</section>

<style>
  .log {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: none;
    min-height: 0;
    overflow: hidden;
    border-top: 1px solid var(--border);
    background: var(--bg);
    outline: none;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
    height: 32px;
    padding: 0 6px 0 14px;
    border-bottom: 1px solid var(--border);
    font-size: var(--fs-sm);
  }

  .title {
    font-size: var(--fs-sm);
    font-weight: 600;
  }

  .count {
    color: var(--text-faint);
  }

  .hidden-details {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .spacer {
    flex: 1;
  }

  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 24px;
    padding: 0 8px;
    border-radius: var(--radius);
    color: var(--text-muted);
    white-space: nowrap;
  }

  .toggle:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .close {
    width: 26px;
    height: 26px;
  }

  .scroller {
    flex: 1;
    min-height: 0;
    overflow: auto;
    overscroll-behavior: contain;
    padding: 4px 0 8px;
  }

  .empty {
    padding: 10px 14px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
  }

  .retry {
    margin-left: 6px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }

  .retry:hover {
    text-decoration: underline;
  }

  .earlier {
    display: block;
    margin: 2px 14px 6px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: var(--fs-xs);
    cursor: pointer;
  }

  .entries {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .op {
    position: relative;
  }

  .op + .op,
  .single + .op,
  .op + .single {
    margin-top: 2px;
  }

  /* "Show log" went here: an accent bar and a tint, until the next jump. */
  .target {
    background: color-mix(in srgb, var(--accent) 7%, transparent);
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .cmd {
    position: relative;
  }

  .row {
    display: grid;
    grid-template-columns: 12px 58px minmax(0, 1fr) auto 52px;
    align-items: center;
    column-gap: 8px;
    width: 100%;
    height: 24px;
    padding: 0 40px 0 10px;
    border: 0;
    background: none;
    color: var(--text);
    font-size: var(--fs-sm);
    text-align: left;
    cursor: default;
  }

  .op-head {
    grid-template-columns: 12px 58px minmax(0, 1fr) auto auto 52px;
    height: 26px;
  }

  .in-op .row {
    padding-left: 30px;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .row:focus-visible,
  .copy:focus-visible,
  .more:focus-visible,
  .earlier:focus-visible,
  .retry:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: -2px;
  }

  .chev {
    width: 12px;
    height: 12px;
    display: grid;
    place-items: center;
  }

  .chev::before {
    content: "";
    width: 5px;
    height: 5px;
    border-right: 1.3px solid var(--text-faint);
    border-bottom: 1.3px solid var(--text-faint);
    transform: rotate(-45deg);
    transition: transform 80ms linear;
  }

  .chev.open::before {
    transform: rotate(45deg) translate(-1px, -1px);
  }

  .chev.none::before {
    visibility: hidden;
  }

  .time {
    color: var(--text-faint);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }

  .line {
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-mono);
    /* "--" stays two hyphens. */
    font-variant-ligatures: none;
    font-size: var(--fs-xs);
    white-space: pre;
    text-overflow: ellipsis;
  }

  .read .line {
    color: var(--text-muted);
  }

  .op-title {
    min-width: 0;
    overflow: hidden;
    font-weight: 600;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .meta {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }

  .dur {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .exit {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .exit.bad {
    color: var(--danger);
    font-weight: 600;
  }

  .exit.stopped {
    color: var(--warning);
    font-weight: 600;
  }

  .copy {
    position: absolute;
    top: 2px;
    right: 10px;
    display: grid;
    place-items: center;
    width: 22px;
    height: 20px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--text-faint);
    opacity: 0;
    cursor: pointer;
  }

  .cmd:hover > .copy,
  .copy:focus-visible,
  .row:focus-visible + .copy {
    opacity: 1;
  }

  .copy:hover {
    background: var(--bg-hover);
    color: var(--accent);
  }

  .out {
    margin: 2px 14px 8px 52px;
    padding: 6px 10px 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--text) 3%, var(--bg));
  }

  .in-op .out {
    margin-left: 72px;
  }

  .where {
    margin-bottom: 4px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .stream + .stream {
    margin-top: 6px;
  }

  .label {
    display: block;
    color: var(--text-faint);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  pre {
    margin: 2px 0 0;
    overflow-x: auto;
    font-family: var(--font-mono);
    font-variant-ligatures: none;
    font-size: var(--fs-xs);
    line-height: 1.5;
    white-space: pre;
    tab-size: 4;
  }

  .more {
    margin-top: 4px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: var(--fs-xs);
    cursor: pointer;
  }
</style>
