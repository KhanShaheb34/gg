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
  import { conflictWord, lineFailed, linkNote, turnOffCommand } from "../cli";
  import { formatDate, formatDateLong, homeInText, shellQuote, shortenPath, type DateFormat } from "../format";
  import { api } from "../ipc";
  import { isMac, isWindows } from "../platform";
  import { app, notify, setTheme } from "../state/app.svelte";
  import { openCliDialog, refreshCli, removeCli, removeShellLine, turnOffShellGg } from "../state/cli.svelte";
  import { showDialog } from "../state/overlays.svelte";
  import { findRepoView } from "../state/repo.svelte";
  import { ai, modelFor, refreshAi, VARIABLE_HINTS } from "../state/ai.svelte";
  import type { AiTool, AiToolInfo } from "../types";
  import { brewHint, checkNow, openDownloadPage, restartToUpdate, statusLine, updates } from "../state/updates.svelte";
  import {
    commandWords,
    FETCH_CHOICES,
    intervalName,
    SettingField,
    saving,
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

  // The command's state as it is now: a line of gg's in a shell file may have been taken out
  // by hand, and what the shell said of `gg` may have come before the window listened.
  onMount(() => void refreshCli());

  /** Turning the shell's `gg` off or back on is under way: its buttons wait. */
  let lineBusy = $state(false);
  async function lineAction(action: () => Promise<void>) {
    if (lineBusy) return;
    lineBusy = true;
    try {
      await action();
    } finally {
      lineBusy = false;
    }
  }

  // --- The fields ---------------------------------------------------------------------

  const fields = {
    fetch: new SettingField("fetchIntervalSecs"),
    dates: new SettingField("dateFormat"),
    editor: new SettingField("editorCommand", TEXT_DELAY_MS),
    terminal: new SettingField("terminalCommand", TEXT_DELAY_MS),
    pattern: new SettingField("worktreePattern", TEXT_DELAY_MS),
    updates: new SettingField("checkForUpdates"),
    prTab: new SettingField("openPrTabAfterPush"),
    aiTool: new SettingField("aiTool"),
    claudeModel: new SettingField("aiModels", TEXT_DELAY_MS),
    codexModel: new SettingField("aiModels", TEXT_DELAY_MS),
    commitPrompt: new SettingField("aiCommitPrompt", TEXT_DELAY_MS),
    prPrompt: new SettingField("aiPrPrompt", TEXT_DELAY_MS),
  };

  // --- AI -----------------------------------------------------------------------------

  // Where the tools are, and their versions: looked for each time the settings open.
  onMount(() => void refreshAi({ versions: true }));
  const modelField = (tool: AiTool) => (tool === "claude" ? fields.claudeModel : fields.codexModel);
  /** What is typed in each tool's model field. */
  let models = $state<Record<string, string | null>>({});
  function modelValue(info: AiToolInfo): string {
    return models[info.tool] ?? modelFor(info, app.settings);
  }
  function setModel(info: AiToolInfo, value: string | null) {
    models = { ...models, [info.tool]: value };
    // null: back to gg's default for it; "" runs the tool's own default.
    modelField(info.tool).set({ [info.tool]: value === null ? null : value.trim() } as unknown as Record<string, string>);
  }
  let commitPrompt = $state(untrack(() => app.settings.aiCommitPrompt));
  let prPrompt = $state(untrack(() => app.settings.aiPrPrompt));
  /** The template a field shows: the one set, else gg's default. */
  const shownPrompt = (set: string | null, fallback: string | undefined) => set ?? fallback ?? "";
  function setPrompt(kind: "commit" | "pr", value: string) {
    const fallback = kind === "commit" ? ai.tools?.commitPrompt : ai.tools?.prPrompt;
    // The default as it is, or nothing: gg's default (it follows gg's own changes).
    const saved = value.trim() === "" || value === fallback ? null : value;
    if (kind === "commit") {
      commitPrompt = saved;
      fields.commitPrompt.set(saved);
    } else {
      prPrompt = saved;
      fields.prPrompt.set(saved);
    }
  }
  /** A template without the changes: the tool would see nothing to describe. */
  function promptWarning(kind: "commit" | "pr", value: string | null): string | null {
    if (value === null) return null;
    const needs = kind === "commit" ? ["{stagedPatch}", "{basePrompt}"] : ["{patch}", "{basePrompt}"];
    return needs.some((n) => value.includes(n))
      ? null
      : `Without ${needs[0]} (or {basePrompt}) the tool does not see the changes.`;
  }
  let checking = $state(false);
  async function checkTools() {
    checking = true;
    try {
      await refreshAi({ versions: true, recheck: true });
    } finally {
      checking = false;
    }
  }
  async function installPage(tool: AiTool) {
    await openLink(tool === "claude" ? "installClaude" : "installCodex");
  }
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
    const words = commandWords(command, folder ?? (windows ? "C:\\repos\\app" : "/repos/app"));
    // Each argument as a shell would need it written (for reading only), home as ~.
    return words === null ? null : homeInText(words.map(shellQuote).join(" "), app.homeDir);
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

  /** Why `kind`'s Test button cannot be pressed (said beside it, not only in its tooltip). */
  function testWhy(kind: "editor" | "terminal"): string | null {
    if (repoId === null) return "Open a repository to try it on.";
    const field = kind === "editor" ? fields.editor : fields.terminal;
    if (field.status === "error") return "It was not saved, so Test waits until it is fixed.";
    return null;
  }

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

  async function openLink(link: "home" | "source" | "settingsFile" | "installClaude" | "installCodex") {
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
        models = {};
        commitPrompt = app.settings.aiCommitPrompt;
        prPrompt = app.settings.aiPrPrompt;
        tested = null;
        notify({ kind: "info", message: "Settings are back to their defaults." });
      },
    });
  }

  // --- Updates ------------------------------------------------------------------------

  /** A minute's tick, so "Checked 2 min ago" moves on while the sheet is open. */
  let now = $state(Date.now());
  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(timer);
  });
  const update = $derived(updates.status);
  const updateLine = $derived(statusLine(update, app.settings.checkForUpdates, app.settings.dateFormat, now));
  /**
   * Restart to update, from here: what is typed is saved first. A field that could not be
   * saved (its message is beside it) keeps gg from restarting, so nothing typed is lost.
   */
  async function restartFromHere() {
    for (const field of [fields.editor, fields.terminal, fields.pattern]) {
      if (!(await field.flush())) {
        notify({ kind: "info", message: "gg did not restart: a setting you typed could not be saved. Fix it or clear it, then restart." });
        return;
      }
    }
    await restartToUpdate();
  }
  /** A check, download or install runs: Check now waits. */
  const updateBusy = $derived(["checking", "downloading", "installing"].includes(update.state));

  // --- Sections -----------------------------------------------------------------------

  const SECTIONS = [
    ["appearance", "Appearance"],
    ["fetching", "Fetching"],
    ["tools", "Tools"],
    ["worktrees", "Worktrees"],
    ["prs", "Pull requests"],
    ["ai", "AI"],
    ["command", "Command line"],
    ["about", "About"],
  ] as const;
  let scroller: HTMLDivElement;
  let current = $state<string>("appearance");

  /** Where section `id` starts in the scrolled content, px. */
  function topOf(id: string): number | null {
    const el = scroller.querySelector<HTMLElement>(`#settings-${id}`);
    if (!el) return null;
    return el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
  }

  function go(id: string) {
    const top = topOf(id);
    if (top === null) return;
    current = id;
    scroller.scrollTo({ top, behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  }

  function onScroll() {
    const top = scroller.scrollTop + 24;
    let found: string = SECTIONS[0][0];
    for (const [id] of SECTIONS) {
      const at = topOf(id);
      if (at !== null && at <= top) found = id;
    }
    // At the bottom, the last one.
    if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2) found = SECTIONS.at(-1)![0];
    current = found;
  }

  onMount(() => {
    // Whatever is typed is saved when the sheet closes.
    return () => {
      for (const field of [
        fields.editor,
        fields.terminal,
        fields.pattern,
        fields.claudeModel,
        fields.codexModel,
        fields.commitPrompt,
        fields.prPrompt,
      ])
        void field.flush();
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
      <p class="explain">
        {#each explain.split("{path}") as part, i (i)}{#if i > 0}<code class="ph">{"{path}"}</code>{/if}{part}{/each}
      </p>
      <div class="input-row">
        <input
          id="settings-{kind}"
          class="text-input mono-input"
          type="text"
          spellcheck="false"
          autocomplete="off"
          {placeholder}
          {value}
          aria-invalid={field.status === "error" && field.error !== null}
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
          aria-describedby={testWhy(kind) === null ? undefined : `settings-${kind}-test-why`}
          data-testid="settings-{kind}-test"
          onclick={() => void test(kind)}>Test</button
        >
      </div>
      <div class="note" id="settings-{kind}-note">
        <!-- The specific reason first, then what it means for Test. -->
        {#if field.status === "error" && field.error}
          <p class="problem" role="alert">{field.error}</p>
        {/if}
        {#if testWhy(kind) !== null}
          <p class="hint" id="settings-{kind}-test-why" data-testid="settings-{kind}-test-why">{testWhy(kind)}</p>
        {/if}
        {#if field.status === "error" && field.error}
          <!-- Said above. -->
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
    {#if saving.problem}
      <div class="unsaved" role="alert" data-testid="settings-save-problem">
        <strong>The last change was not saved.</strong>
        {saving.problem}
      </div>
    {/if}
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
              >tooltip: {formatDateLong(sampleSecs, app.settings.dateFormat)}</span
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
            aria-invalid={fields.pattern.status === "error" && fields.pattern.error !== null}
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

    <section id="settings-prs" aria-labelledby="h-prs">
      <h3 class="section" id="h-prs">Pull requests</h3>
      <div class="setting">
        <div class="name"><span>After a push</span>{@render status(fields.prTab)}</div>
        <div class="control">
          <label class="check-row update-auto">
            <input
              class="checkbox"
              type="checkbox"
              checked={app.settings.openPrTabAfterPush}
              data-testid="settings-pr-tab"
              onchange={(e) => {
                const on = e.currentTarget.checked;
                app.settings.openPrTabAfterPush = on;
                fields.prTab.set(on);
              }}
            />
            <span>
              Open the Pull request tab after a push when the branch has no pull request
              <span class="check-note"
                >After Push or Commit & Push, the side panel shows the branch’s pull request form, without taking the focus. Not for
                the default branch, nor where pull requests cannot be made (gh missing or signed out, a remote not on GitHub).</span
              >
            </span>
          </label>
          {#if fields.prTab.error}<p class="problem" role="alert">{fields.prTab.error}</p>{/if}
        </div>
      </div>
    </section>

    <section id="settings-ai" aria-labelledby="h-ai" data-testid="settings-ai">
      <h3 class="section" id="h-ai">AI</h3>
      <div class="setting">
        <div class="name"><span>Generate with AI</span>{@render status(fields.aiTool)}</div>
        <div class="control">
          <p class="explain">
            Writes a commit message, or a pull request’s title and description, with an AI command-line tool installed on this computer,
            under your own account with it. Only when you click it: the tool sends the changes to its provider; gg itself sends nothing
            anywhere. It runs with no tools (Claude Code) or with its commands off, read-only (Codex), in a temporary folder, never in the
            repository.
          </p>
          <Segmented
            label="AI tool"
            testid="settings-ai-tool"
            options={[
              { value: "auto", label: "Automatic" },
              { value: "claude", label: "Claude Code" },
              { value: "codex", label: "Codex" },
            ] as { value: "auto" | AiTool; label: string }[]}
            value={app.settings.aiTool}
            onchange={(t) => {
              app.settings.aiTool = t;
              fields.aiTool.set(t);
            }}
          />
          <p class="hint">Automatic: Claude Code if it is installed, else Codex.</p>
          <ul class="tools" data-testid="settings-ai-tools">
            {#each ai.tools?.tools ?? [] as info (info.tool)}
              <li class="state tool-row" data-tool={info.tool} data-installed={info.installed}>
                <span class="dot" class:on={info.installed} aria-hidden="true"></span>
                <span class="tool-name">{info.name}</span>
                {#if info.installed}
                  <span class="tool-meta" title={info.path ?? undefined}>{info.version ?? "Installed"}{info.path ? ` · ${shortenPath(info.path, app.homeDir)}` : ""}</span>
                {:else}
                  <span class="tool-meta">Not installed</span>
                  <button type="button" class="link-btn small-link" onclick={() => void installPage(info.tool)}>Get {info.name} ↗</button>
                {/if}
              </li>
            {:else}
              <li class="state tool-row"><span class="dot busy" aria-hidden="true"></span><span class="tool-meta">Looking for the tools…</span></li>
            {/each}
          </ul>
          <div class="buttons">
            <button type="button" class="button" disabled={checking} data-testid="settings-ai-check" onclick={() => void checkTools()}
              >{checking ? "Checking…" : "Check again"}</button
            >
          </div>
          {#if fields.aiTool.error}<p class="problem" role="alert">{fields.aiTool.error}</p>{/if}
        </div>
      </div>
      {#each ai.tools?.tools ?? [] as info (info.tool)}
        {@const field = modelField(info.tool)}
        {@const value = modelValue(info)}
        <div class="setting">
          <div class="name">
            <label for="settings-ai-model-{info.tool}">{info.name} model</label>
            {@render status(field)}
          </div>
          <div class="control">
            <div class="input-row">
              <input
                id="settings-ai-model-{info.tool}"
                class="text-input mono-input"
                type="text"
                spellcheck="false"
                autocomplete="off"
                list="settings-ai-models-{info.tool}"
                placeholder="{info.name}’s own default"
                {value}
                aria-invalid={field.status === "error" && field.error !== null}
                data-testid="settings-ai-model-{info.tool}"
                oninput={(e) => setModel(info, e.currentTarget.value)}
                onblur={() => void field.flush()}
              />
              <datalist id="settings-ai-models-{info.tool}">
                {#each info.models as m (m)}<option value={m}></option>{/each}
              </datalist>
            </div>
            <div class="note">
              {#if field.status === "error" && field.error}
                <p class="problem" role="alert">{field.error}</p>
              {:else}
                <p class="hint">
                  {#if value.trim() === ""}Empty: {info.name}’s own default model.{:else}Runs it with <code>--model {value.trim()}</code>.{/if}
                  {#if info.tool === "codex"}gg runs Codex without your <code>config.toml</code> (its MCP servers stay off), so a model set there does not apply.{/if}
                  {#if info.defaultModel !== ""}
                    gg’s default is <code>{info.defaultModel}</code>, fast and cheap.
                    {#if value.trim() !== info.defaultModel}
                      <button type="button" class="link-btn small-link" data-testid="settings-ai-model-{info.tool}-default" onclick={() => setModel(info, null)}
                        >Use it</button
                      >
                    {/if}
                  {/if}
                </p>
              {/if}
            </div>
          </div>
        </div>
      {/each}
      {#if ai.tools}
        {#each [{ kind: "commit", label: "Commit message prompt", set: commitPrompt, fallback: ai.tools.commitPrompt, vars: ai.tools.commitVariables, field: fields.commitPrompt }, { kind: "pr", label: "Pull request prompt", set: prPrompt, fallback: ai.tools.prPrompt, vars: ai.tools.prVariables, field: fields.prPrompt }] as p (p.kind)}
          {@const kind = p.kind as "commit" | "pr"}
          <div class="setting">
            <div class="name"><span>{p.label}</span>{@render status(p.field)}</div>
            <div class="control">
              <details class="prompt" data-testid="settings-ai-prompt-{kind}">
                <summary>{p.set === null ? "gg’s default (from Orca)" : "Your own"} · edit</summary>
                <textarea
                  class="text-input mono-input prompt-text"
                  rows="12"
                  spellcheck="false"
                  aria-label={p.label}
                  value={shownPrompt(p.set, p.fallback)}
                  data-testid="settings-ai-prompt-{kind}-text"
                  oninput={(e) => setPrompt(kind, e.currentTarget.value)}
                  onblur={() => void p.field.flush()}
                ></textarea>
                <div class="vars">
                  {#each p.vars as v (v)}
                    <span class="var" title={VARIABLE_HINTS[v] ?? ""}><code class="ph">{`{${v}}`}</code> {VARIABLE_HINTS[v] ?? ""}</span>
                  {/each}
                </div>
                {#if promptWarning(kind, p.set) !== null}<p class="warn">{promptWarning(kind, p.set)}</p>{/if}
                {#if p.field.error}<p class="problem" role="alert">{p.field.error}</p>{/if}
                <div class="buttons">
                  <button
                    type="button"
                    class="button"
                    disabled={p.set === null}
                    data-testid="settings-ai-prompt-{kind}-reset"
                    onclick={() => setPrompt(kind, "")}>Reset to default</button
                  >
                </div>
              </details>
            </div>
          </div>
        {/each}
      {/if}
    </section>

    <section id="settings-command" aria-labelledby="h-command">
      <h3 class="section" id="h-command">Command line</h3>
      <div class="setting">
        <div class="name"><span>The <code>gg</code> command</span></div>
        <div class="control">
          <p class="explain">Opens the repository of the folder you are in: <code>gg</code>, or <code>gg path/to/repo</code>.</p>
          {#if app.cli.installed && app.cli.link === "homebrew"}
            <p class="state" data-testid="settings-cli-state">
              <span class="dot on" aria-hidden="true"></span>Installed by Homebrew at <code
                >{shortenPath(app.cli.installed, app.homeDir)}</code
              >
            </p>
          {:else if app.cli.installed && app.cli.link === "other"}
            <p class="state" data-testid="settings-cli-state">
              <span class="dot on" aria-hidden="true"></span>Installed at <code>{shortenPath(app.cli.installed, app.homeDir)}</code>,
              a link to gg’s launcher
            </p>
          {:else if app.cli.installed}
            <p class="state" data-testid="settings-cli-state">
              <span class="dot on" aria-hidden="true"></span>Installed at <code>{shortenPath(app.cli.installed, app.homeDir)}</code>
            </p>
          {:else}
            <p class="state" data-testid="settings-cli-state"><span class="dot" aria-hidden="true"></span>Not installed</p>
          {/if}
          {#if app.cli.shadowedBy}
            <p class="warn" role="status" data-testid="settings-cli-shadowed">
              Your shell already defines <code>gg</code> as {app.cli.shadowedBy}; that wins over this command until you remove it.
            </p>
          {/if}
          <!-- gg's line in a shell file: added only when the user asks here or in the install dialog. -->
          {#if !windows && app.cli.shellLine !== null}
            <p class="explain" data-testid="settings-cli-line">
              {#if lineFailed(app.cli)}
                gg’s line in <code>{shortenPath(app.cli.shellLine, app.homeDir)}</code> does not turn it off: something that runs after
                that line defines <code>gg</code> again.
              {:else}
                gg added one line to the end of <code>{shortenPath(app.cli.shellLine, app.homeDir)}</code> that turns off your shell’s
                own <code>gg</code>.
              {/if}
            </p>
          {/if}
          {#if !windows && app.cli.turnOff !== null}
            <p class="explain" data-testid="settings-cli-turn-off-note">
              gg can add one line, marked as gg’s, to the end of <code>{shortenPath(app.cli.turnOff.file, app.homeDir)}</code>:
              <code>{turnOffCommand(app.cli.turnOff.conflict)}</code>. It takes effect in new terminal windows.
            </p>
          {/if}
          {#if !windows && (app.cli.turnOff !== null || app.cli.shellLine !== null)}
            <div class="buttons">
              {#if app.cli.turnOff !== null}
                <button
                  type="button"
                  class="button"
                  disabled={lineBusy}
                  data-testid="settings-cli-turn-off"
                  onclick={() => void lineAction(turnOffShellGg)}>Turn off that {conflictWord(app.cli.turnOff.conflict)} for me</button
                >
              {/if}
              {#if app.cli.shellLine !== null}
                <button
                  type="button"
                  class="button"
                  disabled={lineBusy}
                  data-testid="settings-cli-line-remove"
                  onclick={() => void lineAction(removeShellLine)}>Take the line out</button
                >
              {/if}
            </div>
          {/if}
          {#if linkNote(app.cli.link) !== null}
            <!-- A link made outside the app (Homebrew's): nothing here installs or removes it. -->
            <p class="explain" data-testid="settings-cli-link">{linkNote(app.cli.link)}</p>
          {:else}
            <div class="buttons">
              <button type="button" class="button" data-testid="settings-cli-install" onclick={() => void openCliDialog()}
                >{app.cli.installed ? "Install elsewhere…" : "Install…"}</button
              >
              {#if app.cli.installed}
                <button type="button" class="button" data-testid="settings-cli-remove" onclick={() => void removeCli()}>Remove</button>
              {/if}
            </div>
          {/if}
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
        <div class="name"><span>Updates</span>{@render status(fields.updates)}</div>
        <div class="control">
          <label class="check-row update-auto">
            <input
              class="checkbox"
              type="checkbox"
              checked={app.settings.checkForUpdates}
              data-testid="settings-check-updates"
              onchange={(e) => {
                const on = e.currentTarget.checked;
                app.settings.checkForUpdates = on;
                fields.updates.set(on);
              }}
            />
            <span>
              Check for updates automatically
              <span class="check-note">Shortly after gg starts, then once a day, gg asks GitHub for a small file describing the latest release. It carries nothing about you or your repositories.</span>
            </span>
          </label>
          {#if fields.updates.error}<p class="problem" role="alert">{fields.updates.error}</p>{/if}
          <p class="state update-state" role="status" data-testid="settings-update-status" data-state={update.state}>
            <span class="dot {updateLine.tone}" aria-hidden="true"></span>{updateLine.text}
          </p>
          {#if update.state === "failed" && update.detail}
            <p class="update-detail selectable" data-testid="settings-update-detail">{update.detail}</p>
          {:else if update.state === "available"}
            <p class="explain" data-testid="settings-update-reason">{update.reason}</p>
            {#if brewHint()}
              <p class="explain" data-testid="settings-update-brew">Or update it with Homebrew: <code class="selectable">{brewHint()}</code></p>
            {/if}
          {/if}
          {#if update.state !== "unavailable"}
            <div class="buttons">
              {#if update.state === "ready" || update.state === "installing"}
                <button
                  type="button"
                  class="button primary"
                  disabled={update.state === "installing"}
                  data-testid="settings-update-restart"
                  onclick={() => void restartFromHere()}>Restart to update</button
                >
              {:else if update.state === "available"}
                <button type="button" class="button primary" data-testid="settings-update-download" onclick={() => void openDownloadPage()}
                  >Download page</button
                >
              {/if}
              <button type="button" class="button" disabled={updateBusy} data-testid="settings-update-check" onclick={() => void checkNow()}
                >Check now</button
              >
            </div>
          {/if}
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

  .dot.on,
  .dot.ok {
    background: var(--success);
  }

  .dot.busy {
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }

  .dot.warn {
    background: var(--warning);
  }

  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .dot.busy {
      animation: none;
    }
  }

  .tools {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 4px 0 0;
    padding: 0;
    list-style: none;
  }

  .tool-row {
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 7px;
  }

  .tool-row .dot {
    align-self: center;
  }

  .tool-row .link-btn {
    line-height: inherit;
  }

  .tool-name {
    font-weight: 550;
  }

  .tool-meta {
    min-width: 0;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    overflow-wrap: anywhere;
  }

  .small-link {
    font-size: var(--fs-sm);
  }

  .prompt {
    align-self: stretch;
  }

  .prompt summary {
    width: fit-content;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    cursor: default;
    user-select: none;
  }

  .prompt summary:hover {
    color: var(--text);
  }

  .prompt[open] summary {
    margin-bottom: 8px;
  }

  .prompt-text {
    display: block;
    width: 100%;
    min-height: 180px;
    font-size: 11.5px;
    line-height: 1.5;
    resize: vertical;
  }

  .vars {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 8px 0 4px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 1.5;
  }

  .update-auto {
    margin-top: 4px;
    font-size: var(--fs);
  }

  .update-state {
    padding-top: 2px;
  }

  .update-detail {
    max-width: 100%;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
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

  .button.primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .button.primary:hover:not(:disabled) {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .button.primary:active:not(:disabled) {
    background: color-mix(in srgb, var(--accent-hover) 88%, black);
  }

  .button.primary:disabled {
    opacity: 0.6;
    color: var(--accent-contrast);
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
