<!--
  The keyboard shortcuts, by group, as the shortcut table (shortcuts.ts) has them: the same
  table drives the window's handler, so this cannot list a key that does nothing. Opened with
  ? (no text field focused) or Help › Keyboard Shortcuts.
-->
<script lang="ts">
  import { isMac } from "../platform";
  import { shortcutSheet } from "../shortcuts";
  import Sheet from "./Sheet.svelte";

  let { close }: { close: () => void } = $props();

  const groups = shortcutSheet(isMac());
</script>

<Sheet title="Keyboard shortcuts" width={820} onClose={close} testid="shortcuts">
  <div class="groups" data-autofocus tabindex="-1">
    {#each groups as g (g.group)}
      <section class="group" aria-labelledby="sc-{g.group}">
        <h3 class="group-title" id="sc-{g.group}">{g.group}</h3>
        <dl class="rows">
          {#each g.rows as row (row.does)}
            <div class="row">
              <dt class="does">
                {row.does}
                {#if row.where}<span class="where">{row.where}</span>{/if}
              </dt>
              <dd class="keys">
                {#each row.keys as alt, i (i)}
                  {#if i > 0}<span class="or">or</span>{/if}
                  <span class="combo">
                    {#each alt as k, j (j)}<kbd>{k}</kbd>{/each}
                  </span>
                {/each}
              </dd>
            </div>
          {/each}
        </dl>
      </section>
    {/each}
  </div>
</Sheet>

<style>
  .groups {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    padding: 6px 22px 22px;
    columns: 2 340px;
    column-gap: 34px;
    outline: none;
  }

  .group {
    break-inside: avoid;
    padding-top: 14px;
  }

  .group-title {
    margin-bottom: 6px;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    font-weight: 600;
  }

  .rows {
    margin: 0;
  }

  .row {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 5px 0;
    border-bottom: 1px solid var(--border);
  }

  .row:last-child {
    border-bottom: 0;
  }

  .does {
    flex: 1 1 auto;
    min-width: 0;
    line-height: 1.35;
  }

  .where {
    display: block;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .keys {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: 4px;
    flex: none;
    max-width: 50%;
    margin: 0;
  }

  .combo {
    display: inline-flex;
    gap: 2px;
  }

  .or {
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  kbd {
    display: inline-grid;
    place-items: center;
    min-width: 20px;
    height: 20px;
    padding: 0 5px;
    border: 1px solid var(--border-strong);
    border-bottom-width: 2px;
    border-radius: 5px;
    background: var(--bg);
    color: var(--text);
    font-family: var(--font-ui);
    font-size: var(--fs-xs);
    font-weight: 600;
    line-height: 1;
    white-space: nowrap;
  }
</style>
