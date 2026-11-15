<!--
  Install the `gg` terminal command: which folder on the PATH gets the launcher, what is
  written there, and, when the user's shell has a `gg` of its own, that it wins. Remove takes
  out the launcher the app wrote. The app never edits shell files.
-->
<script lang="ts">
  import { launcherPath, preferredFolder } from "../../cli";
  import { shortenPath } from "../../format";
  import { app, notify, toast } from "../../state/app.svelte";
  import { installCli, removedMessage, uninstallCli } from "../../state/cli.svelte";
  import Dialog from "../Dialog.svelte";

  type Props = { close: () => void };

  let { close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  /** Most folders shown as a column of choices; more go in a list. */
  const MAX_RADIOS = 4;
  // The folders as the dialog opened: they do not move under the user's pointer. What the
  // shell says of `gg` may arrive later (cli-changed), and is shown when it does.
  const shown = $state.snapshot(app.cli);
  const shadowedBy = $derived(app.cli.shadowedBy);
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
    void run(async () => {
      const after = await installCli(target);
      const wrote = short(launcherPath(target));
      notify({
        kind: "info",
        message: `Installed the gg command: wrote ${wrote}. Run gg in a repository folder to open it here.`,
        detail:
          after.shadowedBy === null
            ? undefined
            : `Your shell already defines gg as ${after.shadowedBy}; that wins over this command until you remove it.`,
      });
    });
  }

  function remove() {
    void run(async () => {
      toast("info", removedMessage(await uninstallCli()));
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
      Add a folder such as <code>~/.local/bin</code> to your <code>PATH</code> in your shell’s profile, then open this
      again.
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
      Writes one small script, {short(launcherPath(dir))}, and changes nothing else. No administrator rights are
      needed.
    </p>
  {/if}

  {#if shadowedBy !== null}
    <p class="shadowed" data-testid="cli-shadowed">
      Your shell already defines <code>gg</code> as {shadowedBy}; that wins over this command until you remove
      it.
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

  .problem {
    margin-top: 12px;
    color: var(--danger);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
</style>
