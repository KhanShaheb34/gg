<!--
  Settings (Cmd/Ctrl+,, the app menu's Settings…, the gear in the tab bar): a tall sheet over
  the window, its sections listed on the left. Every field saves as it changes (text after a
  pause) and says "Saved" quietly beside its name; a value the core refuses keeps its text and
  shows why under it. Each one shows what it does with real data, live: the date format on a
  date, the commands as they will be run on the open repository's folder, where a new
  worktree would go. When the settings file cannot be written, the top says so, once.
-->
<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { formatDate, formatDateLong, shellQuote, shortenPath, type DateFormat } from "../format";
  import { api } from "../ipc";
  import { isMac, isWindows } from "../platform";
  import { app, notify, setTheme } from "../state/app.svelte";
  import { openCliDialog, removeCli } from "../state/cli.svelte";
  import { showDialog } from "../state/overlays.svelte";
  import { findRepoView } from "../state/repo.svelte";
  import {
    commandWords,
    FETCH_CHOICES,
    intervalName,
    SettingField,
    TEXT_DELAY_MS,
    type SaveStatus,
  } from "../state/settings.svelte";
  import type { ThemeSetting } from "../theme";
  import ResetSettingsDialog from "./dialogs/ResetSettingsDialog.svelte";
  import Segmented from "./Segmented.svelte";
  import Sheet from "./Sheet.svelte";

  let { close }: { close: () => void } = $props();

  const mac = isMac();
  const windows = isWindows();

  // --- The fields ---------------------------------------------------------------------

  const fields = {
    fetch: new SettingField("fetchIntervalSecs"),
    dates: new SettingField("dateFormat"),
    editor: new SettingField("editorCommand", TEXT_DELAY_MS),
    terminal: new SettingField("terminalCommand", TEXT_DELAY_MS),
    pattern: new SettingField("worktreePattern", TEXT_DELAY_MS),
  };
  /** A command field, as the snippet that draws it uses it. */
  type CommandField = { status: SaveStatus; error: string | null; set: (v: string | null) => void; flush: () => Promise<boolean> };
  // What is typed, kept whatever the core says of it.
  let editor = $state(untrack(() => app.settings.editorCommand ?? ""));
  let terminal = $state(untrack(() => app.settings.terminalCommand ?? ""));
  let pattern = $state(untrack(() => app.settings.worktreePattern));

  // --- The open repository, for the previews and Test ----------------------------------

  const repoId = $derived(app.activeRepoId);
  const view = $derived(repoId === null ? undefined : findRepoView(repoId));
  /** The folder the commands would get: the open repository's active worktree. */
  const folder = $derived(view?.snapshot.activeWorktree || null);
  const shownFolder = $derived(folder === null ? null : shortenPath(folder, app.homeDir));

  /** The command as gg will run it on the open folder (one word per argument, quoted for reading). */
  function preview(command: string): string | null {
    if (command.trim() === "") return null;
    const words = commandWords(command, shownFolder ?? (windows ? "C:\\repos\\app" : "~/repos/app"));
    return words === null ? null : words.map(shellQuote).join(" ");
  }

  // --- Dates --------------------------------------------------------------------------

  const DATE_OPTIONS: { value: DateFormat; label: string }[] = [
    { value: "standard", label: "Standard" },
    { value: "relative", label: "Relative" },
    { value: "iso", label: "ISO 8601" },
    { value: "locale", label: "System" },
  ];
  /** A date to show the format on: a little over two days ago. */
  const sampleSecs = Math.floor((Date.now() - (2 * 86_400 + 3 * 3600) * 1000) / 60_000) * 60;

  // --- Fetching -----------------------------------------------------------------------

  const fetchOptions = $derived.by(() => {
    const list: { value: number; label: string; title?: string }[] = FETCH_CHOICES.map((secs) => ({
      value: secs,
      label: intervalName(secs),
    }));
    const now = app.settings.fetchIntervalSecs;
    if (!list.some((o) => o.value === now)) list.push({ value: now, label: intervalName(now), title: "Set in the settings file" });
    return list;
  });

  // --- Worktree location preview ------------------------------------------------------

  const SAMPLE_BRANCH = "feature/login";
  let where = $state<string | null>(null);
  $effect(() => {
    // Asked again once the pattern is saved (the core works it out from the saved one).
    void app.settings.worktreePattern;
    const id = repoId;
    if (id === null) {
      where = null;
      return;
    }
    let live = true;
    api.worktreeDefaultPath(id, SAMPLE_BRANCH).then(
      (p) => live && (where = shortenPath(p.path, app.homeDir)),
      () => live && (where = null),
    );
    return () => (live = false);
  });

  // --- Test ---------------------------------------------------------------------------

  let tested = $state<{ tool: "editor" | "terminal"; ok: boolean; text: string } | null>(null);

  async function test(tool: "editor" | "terminal") {
    if (repoId === null) return;
    const field = tool === "editor" ? fields.editor : fields.terminal;
    tested = null;
    if (!(await field.flush())) return;
    try {
      const { note } = await api.openExternal(repoId, { target: tool });
      tested = { tool, ok: true, text: note ?? `Started for ${shownFolder}.` };
    } catch (e) {
      tested = { tool, ok: false, text: e instanceof Error ? e.message : String(e) };
    }
  }

  // --- About --------------------------------------------------------------------------

  async function openLink(link: "home" | "source" | "settingsFile") {
    try {
      const { note } = await api.openAppLink(link);
      if (note) notify({ kind: "info", message: note });
    } catch (e) {
      notify({ kind: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }

  function askReset() {
    showDialog(ResetSettingsDialog, {
      onReset: () => {
        for (const field of Object.values(fields)) field.reset();
        editor = app.settings.editorCommand ?? "";
        terminal = app.settings.terminalCommand ?? "";
        pattern = app.settings.worktreePattern;
        tested = null;
        notify({ kind: "info", message: "Settings are back to their defaults." });
      },
    });
  }

  // --- Sections -----------------------------------------------------------------------

  const SECTIONS = [
    ["appearance", "Appearance"],
    ["fetching", "Fetching"],
    ["tools", "Tools"],
    ["worktrees", "Worktrees"],
    ["command", "Command line"],
    ["about", "About"],
  ] as const;
  let scroller: HTMLDivElement;
  let current = $state<string>("appearance");

  function go(id: string) {
    const el = scroller.querySelector<HTMLElement>(`#settings-${id}`);
    if (!el) return;
    current = id;
    scroller.scrollTo({ top: el.offsetTop - 8, behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  }

  function onScroll() {
    const top = scroller.scrollTop + 24;
    let found: string = SECTIONS[0][0];
    for (const [id] of SECTIONS) {
      const el = scroller.querySelector<HTMLElement>(`#settings-${id}`);
      if (el && el.offsetTop <= top) found = id;
    }
    // At the bottom, the last one.
    if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2) found = SECTIONS.at(-1)![0];
    current = found;
  }

  onMount(() => {
    // Whatever is typed is saved when the sheet closes.
    return () => {
      for (const field of [fields.editor, fields.terminal, fields.pattern]) void field.flush();
    };
  });

  const EDITOR_PLACEHOLDER = mac ? 'open -a "Visual Studio Code"' : "code";
  const TERMINAL_PLACEHOLDER = mac ? "open -a iTerm" : windows ? "wt -d {path}" : "kitty --directory {path}";
  const EDITOR_DEFAULT = mac
    ? "Empty: files open as text in the system’s text editor, folders in Finder."
    : `Empty: the folder opens in ${windows ? "File Explorer" : "the file manager"}.`;
  const TERMINAL_DEFAULT = mac
    ? "Empty: Terminal."
    : windows
      ? "Empty: Windows Terminal, else Command Prompt."
      : "Empty: the first of x-terminal-emulator, GNOME Terminal, Konsole and xterm.";
</script>

{#snippet status(field: { status: string })}
  {#if field.status === "saved"}
    <span class="saved" role="status">Saved</span>
  {:else if field.status === "saving"}
    <span class="saved saving" role="status">Saving…</span>
  {/if}
{/snippet}

{#snippet arrow()}
  <svg class="arrow" width="12" height="10" viewBox="0 0 12 10" aria-hidden="true">
    <path d="M1 5h9M6.5 1.5 10 5 6.5 8.5" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
{/snippet}

{#snippet tool(kind: "editor" | "terminal", label: string, value: string, field: CommandField, placeholder: string, explain: string, empty: string)}
  {@const runs = preview(value)}
  <div class="setting">
    <div class="name">
      <label for="settings-{kind}">{label}</label>
      {@render status(field)}
    </div>
    <div class="control">
      <p class="explain">{explain}</p>
      <div class="input-row">
        <input
          id="settings-{kind}"
          class="text-input mono-input"
          type="text"
          spellcheck="false"
          autocomplete="off"
          {placeholder}
          {value}
          aria-invalid={field.status === "error"}
          aria-describedby="settings-{kind}-note"
          data-testid="settings-{kind}"
          oninput={(e) => {
            const v = e.currentTarget.value;
            if (kind === "editor") editor = v;
            else terminal = v;
            tested = null;
            field.set(v.trim() === "" ? null : v);
          }}
          onblur={() => void field.flush()}
        />
        <button
          type="button"
          class="button"
          disabled={repoId === null || field.status === "error"}
          title={repoId === null ? "Open a repository to try it on" : `Open ${shownFolder} with it`}
          data-testid="settings-{kind}-test"
          onclick={() => void test(kind)}>Test</button
        >
      </div>
      <div class="note" id="settings-{kind}-note">
        {#if field.status === "error" && field.error}
          <p class="problem" role="alert">{field.error}</p>
        {:else if runs !== null}
          <p class="runs" data-testid="settings-{kind}-runs">{@render arrow()}<code>{runs}</code></p>
        {:else if value.trim() === ""}
          <p class="hint">{empty}</p>
        {/if}
        {#if tested?.tool === kind}
          <p class={tested.ok ? "hint" : "problem"} role="status" data-testid="settings-{kind}-tested">{tested.text}</p>
        {/if}
      </div>
    </div>
  </div>
{/snippet}

<Sheet title="Settings" width={800} onClose={close} testid="settings">
  <nav class="nav" aria-label="Settings sections">
    {#each SECTIONS as [id, label] (id)}
      <button type="button" class="nav-item" aria-current={current === id ? "true" : undefined} onclick={() => go(id)}>{label}</button>
    {/each}
  </nav>
  <div class="body" bind:this={scroller} onscroll={onScroll}>
    {#if app.settingsProblem}
      <div class="unsaved" role="alert" data-testid="settings-problem">
        <strong>Settings are not being saved.</strong>
        {app.settingsProblem}. Changes here last until gg quits.
      </div>
    {/if}

    <section id="settings-appearance" aria-labelledby="h-appearance">
      <h3 class="section" id="h-appearance">Appearance</h3>
      <div class="setting">
        <div class="name"><span id="l-theme">Theme</span></div>
        <div class="control">
          <Segmented
            label="Theme"
            testid="settings-theme"
            options={[
              { value: "system", label: "System" },
              { value: "light", label: "Light" },
              { value: "dark", label: "Dark" },
            ] as { value: ThemeSetting; label: string }[]}
            value={app.settings.theme}
            onchange={(t) => void setTheme(t)}
          />
        </div>
      </div>
      <div class="setting">
        <div class="name"><span>Dates</span>{@render status(fields.dates)}</div>
        <div class="control">
          <Segmented
            label="Date format"
            testid="settings-dates"
            options={DATE_OPTIONS}
            value={app.settings.dateFormat}
            onchange={(f) => {
              app.settings.dateFormat = f;
              fields.dates.set(f);
            }}
          />
          <p class="runs" data-testid="settings-date-example">
            {@render arrow()}<span class="sample">{formatDate(sampleSecs, app.settings.dateFormat)}</span><span class="sample-long"
              >{formatDateLong(sampleSecs, app.settings.dateFormat)}</span
            >
          </p>
          <p class="hint">In the graph, the details and the command log; the full date is in their tooltips.</p>
          {#if fields.dates.error}<p class="problem" role="alert">{fields.dates.error}</p>{/if}
        </div>
      </div>
    </section>

    <section id="settings-fetching" aria-labelledby="h-fetching">
      <h3 class="section" id="h-fetching">Fetching</h3>
      <div class="setting">
        <div class="name"><span>Fetch in the background</span>{@render status(fields.fetch)}</div>
        <div class="control">
          <Segmented
            label="Fetch in the background"
            testid="settings-fetch"
            options={fetchOptions}
            value={app.settings.fetchIntervalSecs}
            onchange={(secs) => fields.fetch.set(secs)}
          />
          <p class="hint">
            {app.settings.fetchIntervalSecs === 0
              ? `Only when you fetch (${mac ? "⌘⇧F" : "Ctrl+Shift+F"}).`
              : `Every remote of each open repository, while gg’s window is in use; pull requests load with it.`}
          </p>
          {#if fields.fetch.error}<p class="problem" role="alert">{fields.fetch.error}</p>{/if}
        </div>
      </div>
    </section>

    <section id="settings-tools" aria-labelledby="h-tools">
      <h3 class="section" id="h-tools">Tools</h3>
      {@render tool(
        "editor",
        "Editor",
        editor,
        fields.editor,
        EDITOR_PLACEHOLDER,
        "Opens a worktree, or a file in conflict. The path goes where you write {path}, as a word of its own, else last.",
        EDITOR_DEFAULT,
      )}
      {@render tool(
        "terminal",
        "Terminal",
        terminal,
        fields.terminal,
        TERMINAL_PLACEHOLDER,
        "Opens a terminal in a worktree. The folder goes where you write {path}, as a word of its own, else last.",
        TERMINAL_DEFAULT,
      )}
      <p class="aside">Run as the program and its arguments, never through a shell. Test opens the open repository’s active worktree.</p>
    </section>

    <section id="settings-worktrees" aria-labelledby="h-worktrees">
      <h3 class="section" id="h-worktrees">Worktrees</h3>
      <div class="setting">
        <div class="name">
          <label for="settings-pattern">Location</label>
          {@render status(fields.pattern)}
        </div>
        <div class="control">
          <p class="explain">
            Where Create worktree puts a new one, relative to the repository’s folder. <code class="ph">{"{repo}"}</code> is that
            folder’s name, <code class="ph">{"{branch}"}</code> the branch, made safe for a folder name.
          </p>
          <input
            id="settings-pattern"
            class="text-input mono-input"
            type="text"
            spellcheck="false"
            autocomplete="off"
            placeholder="../{'{repo}'}.worktrees/{'{branch}'}"
            value={pattern}
            aria-invalid={fields.pattern.status === "error"}
            data-testid="settings-pattern"
            oninput={(e) => {
              pattern = e.currentTarget.value;
              fields.pattern.set(pattern);
            }}
            onblur={() => void fields.pattern.flush()}
          />
          <div class="note">
            {#if fields.pattern.status === "error" && fields.pattern.error}
              <p class="problem" role="alert">{fields.pattern.error}</p>
            {:else if repoId === null}
              <p class="hint">Empty: beside the repository, in a folder named after it. Open a repository to see where.</p>
            {:else if where !== null}
              <p class="runs" data-testid="settings-pattern-preview">
                <code>{SAMPLE_BRANCH}</code>{@render arrow()}<code>{where}</code>
              </p>
            {/if}
          </div>
        </div>
      </div>
    </section>

    <section id="settings-command" aria-labelledby="h-command">
      <h3 class="section" id="h-command">Command line</h3>
      <div class="setting">
        <div class="name"><span>The <code>gg</code> command</span></div>
        <div class="control">
          <p class="explain">Opens the repository of the folder you are in: <code>gg</code>, or <code>gg path/to/repo</code>.</p>
          {#if app.cli.installed}
            <p class="state" data-testid="settings-cli-state">
              <span class="dot on" aria-hidden="true"></span>Installed at <code>{shortenPath(app.cli.installed, app.homeDir)}</code>
            </p>
          {:else}
            <p class="state" data-testid="settings-cli-state"><span class="dot" aria-hidden="true"></span>Not installed</p>
          {/if}
          {#if app.cli.shadowedBy}
            <p class="warn" role="status">
              Your shell already defines <code>gg</code> as {app.cli.shadowedBy}; that wins over this command until you remove it.
            </p>
          {/if}
          <div class="buttons">
            <button type="button" class="button" data-testid="settings-cli-install" onclick={() => void openCliDialog()}
              >{app.cli.installed ? "Install elsewhere…" : "Install…"}</button
            >
            {#if app.cli.installed}
              <button type="button" class="button" data-testid="settings-cli-remove" onclick={() => void removeCli()}>Remove</button>
            {/if}
          </div>
        </div>
      </div>
    </section>

    <section id="settings-about" aria-labelledby="h-about">
      <h3 class="section" id="h-about">About</h3>
      <div class="setting">
        <div class="name"><span>gg</span></div>
        <div class="control">
          <p class="version" data-testid="settings-version">Version {app.version}</p>
          <div class="links">
            <button type="button" class="link-btn" onclick={() => void openLink("home")}>gg.shakirul.com</button>
            <button type="button" class="link-btn" onclick={() => void openLink("source")}>Source code</button>
          </div>
        </div>
      </div>
      <div class="setting">
        <div class="name"><span>Settings file</span></div>
        <div class="control">
          <p class="file"><code class="selectable" data-testid="settings-file">{shortenPath(app.settingsFile, app.homeDir)}</code></p>
          <div class="buttons">
            <button type="button" class="button" onclick={() => void openLink("settingsFile")}
              >{mac ? "Reveal in Finder" : windows ? "Show in File Explorer" : "Open containing folder"}</button
            >
            <button type="button" class="button" data-testid="settings-reset" onclick={askReset}>Reset to defaults…</button>
          </div>
        </div>
      </div>
    </section>
  </div>
</Sheet>

<style>
  .nav {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: none;
    width: 168px;
    padding: 12px 8px;
    border-right: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg-chrome) 55%, var(--bg-elevated));
  }

  .nav-item {
    height: 28px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--fs);
    text-align: left;
  }

  .nav-item:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .nav-item[aria-current="true"] {
    background: var(--bg-selected);
    color: var(--text);
    font-weight: 600;
  }

  .body {
    flex: 1 1 auto;
    min-width: 0;
    overflow-y: auto;
    padding: 4px 24px 28px;
    scroll-padding-top: 8px;
  }

  .unsaved {
    margin-top: 14px;
    padding: 10px 12px;
    border: 1px solid color-mix(in srgb, var(--danger) 40%, var(--border));
    border-radius: 8px;
    background: var(--danger-tint);
    font-size: var(--fs-sm);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  section + section {
    border-top: 1px solid var(--border);
  }

  section {
    padding-bottom: 10px;
  }

  .section {
    padding: 18px 0 2px;
    font-size: 13px;
    font-weight: 650;
  }

  /* A setting: its name on the left, the control and what it does on the right. */
  .setting {
    display: grid;
    grid-template-columns: 150px minmax(0, 1fr);
    column-gap: 18px;
    padding-top: 14px;
  }

  .name {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-top: 4px;
    color: var(--text);
    font-size: var(--fs);
    font-weight: 500;
  }

  .saved {
    color: var(--success);
    font-size: var(--fs-xs);
    font-weight: 500;
    animation: saved 160ms var(--ease);
  }

  .saved.saving {
    color: var(--text-faint);
  }

  @keyframes saved {
    from {
      opacity: 0;
    }
  }

  .control {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    min-width: 0;
  }

  .explain,
  .hint,
  .aside {
    color: var(--text-muted);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }

  .aside {
    margin: 12px 0 0 168px;
    color: var(--text-faint);
  }

  .input-row {
    display: flex;
    gap: 8px;
    align-self: stretch;
  }

  .input-row .text-input {
    flex: 1 1 auto;
    min-width: 0;
  }

  .control > .text-input {
    align-self: stretch;
  }

  .mono-input {
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .note {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-height: 18px;
    align-self: stretch;
  }

  .problem {
    color: var(--danger);
    font-size: var(--fs-sm);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  /* What a setting does, with real data: the command as run, a date, a folder. */
  .runs {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 6px;
    min-width: 0;
    color: var(--text);
    font-size: 12px;
    line-height: 1.5;
  }

  .runs code,
  .sample {
    font-family: var(--font-mono);
    overflow-wrap: anywhere;
  }

  .sample {
    font-weight: 600;
  }

  .sample-long {
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }

  .arrow {
    flex: none;
    align-self: center;
    color: var(--accent);
  }

  code {
    font-family: var(--font-mono);
    font-size: 0.95em;
  }

  .ph {
    padding: 0 3px;
    border-radius: 4px;
    background: var(--bg-hover);
    color: var(--text);
  }

  .state {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: var(--fs);
    overflow-wrap: anywhere;
  }

  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--border-strong);
  }

  .dot.on {
    background: var(--success);
  }

  .warn {
    color: var(--warning);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }

  .buttons,
  .links {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 2px;
  }

  .links {
    gap: 14px;
  }

  .version {
    padding-top: 4px;
    font-size: var(--fs);
  }

  .file {
    max-width: 100%;
    padding-top: 4px;
    overflow-wrap: anywhere;
  }

  .button {
    flex: none;
    height: 30px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs);
    font-weight: 500;
    transition: background-color 80ms linear;
  }

  .button:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .button:active:not(:disabled) {
    background: var(--bg-pressed);
  }

  .button:disabled {
    color: var(--text-faint);
  }

  .link-btn {
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--accent);
    font-size: var(--fs);
    font-weight: 500;
    text-decoration: underline;
    text-decoration-color: color-mix(in srgb, var(--accent) 40%, transparent);
    text-underline-offset: 2px;
  }

  .link-btn:hover {
    color: var(--accent-hover);
    text-decoration-color: currentColor;
  }

  /* A narrow window: the sections' list goes, and each setting's name sits above it. */
  @media (max-width: 760px) {
    .nav {
      display: none;
    }

    .setting {
      grid-template-columns: minmax(0, 1fr);
      row-gap: 6px;
    }

    .name {
      flex-direction: row;
      align-items: baseline;
      gap: 8px;
    }

    .aside {
      margin-left: 0;
    }
  }
</style>
