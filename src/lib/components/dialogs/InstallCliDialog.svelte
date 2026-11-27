<!--
  Install the `gg` terminal command: which folder on the PATH gets the launcher, what is
  written there, and, when the user's shell has a `gg` of its own, that it wins. When that is
  an alias or a function of zsh or bash, a box (off until the user ticks it) has gg add one
  marked line to the end of their start-up file to turn it off; nothing else here writes to a
  shell file. Remove takes out the launcher the app wrote, and that line.
-->
<script lang="ts">
  import { conflictWord, installedNote, launcherPath, lineFailed, preferredFolder, turnOffCommand } from "../../cli";
  import { shortenPath } from "../../format";
  import { app, notify } from "../../state/app.svelte";
  import { isWindows } from "../../platform";
  import { installCli, reportRemoved, uninstallCli } from "../../state/cli.svelte";
  import Dialog from "../Dialog.svelte";

  type Props = { close: () => void };

  let { close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  const windows = isWindows();
  /** Most folders shown as a column of choices; more go in a list. */
  const MAX_RADIOS = 4;
  // The folders as the dialog opened: they do not move under the user's pointer. What the
  // shell says of `gg` may arrive later (cli-changed), and is shown when it does.
  const shown = $state.snapshot(app.cli);
  const shadowedBy = $derived(app.cli.shadowedBy);
  /** What gg can turn off with a line, if the user asks (zsh or bash, an alias or a function). */
  const offer = $derived(windows ? null : app.cli.turnOff);
  /** The user ticked "Turn off that alias for me": off until they do. */
  let turnOff = $state(false);
  const turningOff = $derived(turnOff && offer !== null);
  let dir = $state(preferredFolder(shown));
  /** A request is out: nothing else can be asked meanwhile. */
  let busy = $state(false);
  let problem = $state<string | null>(null);

  const short = (path: string) => shortenPath(path, app.homeDir);

  async function run(request: () => Promise<void>) {
    if (busy) return;
    busy = true;
    problem = null;
    try {
      await request();
      close();
    } catch (e) {
      problem = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  function install() {
    if (dir === null) return;
    const target = dir;
    const asked = turningOff;
    void run(async () => {
      const { state: after, shellLineAdded } = await installCli(target, asked);
      // What it wrote, then what became of the shell's own gg (or how to use it). One that
      // did not all go through stays, and is shown whole.
      const { note, worked } = installedNote(short(launcherPath(target)), shellLineAdded, after, short);
      notify({ kind: "info", message: "Installed the gg command", note, persistent: !worked });
    });
  }

  function remove() {
    void run(async () => {
      const { removed, line } = await uninstallCli();
      reportRemoved(removed, line);
    });
  }
</script>

<Dialog
  title="Install the ‘gg’ command"
  primary={shown.installed === null ? "Install" : "Install again"}
  valid={dir !== null && !busy}
  once={false}
  cancellable={!busy}
  secondary={shown.installed === null ? undefined : "Remove"}
  onSecondary={remove}
  onSubmit={install}
  onCancel={close}
>
  <p class="dialog-text">
    Then <code>gg</code> in a terminal opens the repository you are in, and <code>gg &lt;path&gt;</code> the one
    at that path.
  </p>
  {#if shown.installed !== null}
    <p class="dialog-text muted" data-testid="cli-installed">Installed at {short(shown.installed)}.</p>
  {/if}

  {#if shown.candidates.length === 0}
    <p class="dialog-text muted" data-testid="cli-no-folder">
      No folder on your <code>PATH</code> can take it: each one is missing, not writable by you, or inside a repository.
      {#if windows}
        Add a folder of your own, such as <code>%USERPROFILE%\bin</code>, to your user <code>PATH</code> (Environment
        Variables in Windows’ settings), then open this again.
      {:else}
        Add a folder such as <code>~/.local/bin</code> to your <code>PATH</code> in your shell’s profile, then open this
        again.
      {/if}
    </p>
  {:else if shown.candidates.length > MAX_RADIOS}
    <!-- A long PATH: a list rather than a column of rows taller than the window. -->
    <div class="field">
      <label class="field-label" for="cli-dir-{uid}">Folder (on your <code>PATH</code>)</label>
      <div class="select-wrap">
        <select id="cli-dir-{uid}" class="select" bind:value={dir} disabled={busy}>
          {#each shown.candidates as candidate (candidate)}
            <option value={candidate}>{short(candidate)}</option>
          {/each}
        </select>
      </div>
    </div>
  {:else}
    <fieldset class="choices">
      <legend class="field-label">Folder (on your <code>PATH</code>)</legend>
      {#each shown.candidates as candidate (candidate)}
        <label class="choice">
          <input class="radio" type="radio" name="cli-dir-{uid}" value={candidate} bind:group={dir} disabled={busy} />
          <span class="choice-title path-tail"><bdi>{short(candidate)}</bdi></span>
        </label>
      {/each}
    </fieldset>
  {/if}
  {#if dir !== null}
    <p class="dialog-text muted written" data-testid="cli-written">
      {#if turningOff && offer !== null}
        Writes one small script, {short(launcherPath(dir))}, and one line at the end of {short(offer.file)}, and changes
        nothing else. No administrator rights are needed.
      {:else}
        Writes one small script, {short(launcherPath(dir))}, and changes nothing else. No administrator rights are
        needed.
      {/if}
    </p>
  {/if}

  {#if shadowedBy !== null}
    <p class="shadowed" data-testid="cli-shadowed">
      {#if windows}
        Another <code>gg</code> comes first on your <code>PATH</code>: {shadowedBy}; that wins over this command until
        you remove it.
      {:else}
        Your shell already defines <code>gg</code> as {shadowedBy}; that wins over this command until you remove
        it.
      {/if}
      {#if !windows && app.cli.shellLine !== null && lineFailed(app.cli)}
        <span class="line-failed" data-testid="cli-line-failed">
          gg’s line in {short(app.cli.shellLine)} does not turn it off: something that runs after that line defines
          <code>gg</code> again.
        </span>
      {/if}
      {#if offer !== null}
        <label class="turn-off">
          <input type="checkbox" class="checkbox" bind:checked={turnOff} disabled={busy} data-testid="cli-turn-off" />
          <span>Turn off that {conflictWord(offer.conflict)} for me</span>
        </label>
        <span class="turn-off-note" data-testid="cli-turn-off-note">
          Adds one line, marked as gg’s, to the end of {short(offer.file)}: <code>{turnOffCommand(offer.conflict)}</code>.
          Removing the command in gg takes it out again. It takes effect in new terminal windows.
        </span>
      {/if}
    </p>
  {/if}

  {#if problem !== null}
    <p class="problem" role="alert">{problem}</p>
  {/if}
</Dialog>

<style>
  code {
    font-family: var(--font-mono);
    font-size: 0.95em;
  }

  .written {
    margin-top: 10px;
  }

  .shadowed {
    margin-top: 12px;
    padding: 8px 10px;
    border: 1px solid color-mix(in srgb, var(--warning) 45%, var(--border));
    border-radius: 8px;
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .line-failed,
  .turn-off,
  .turn-off-note {
    display: block;
    margin-top: 8px;
  }

  .turn-off {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: default;
  }

  .turn-off-note {
    margin-top: 4px;
    color: var(--text-muted);
    font-size: 0.95em;
  }

  .problem {
    margin-top: 12px;
    color: var(--danger);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
</style>
