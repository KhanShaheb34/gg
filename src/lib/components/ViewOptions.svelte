<!--
  What the graph shows besides its branches: a View button that opens a popover of three
  checkboxes (remote branches, tags, stashes). Each tick applies at once and the popover stays
  open. While any of them is off, the button reads in the accent colour (as the branch filter
  does when it filters) and its tooltip says what is hidden. In a narrow window (`compact`) it
  shows its icon only, its name in the tooltip. Keys: Down on the button opens; Up, Down, Home
  and End move between the rows, Space or Enter ticks, Esc closes (the focus back on the
  button); a press outside, or the focus leaving, closes too.
-->
<script lang="ts">
  import { tick } from "svelte";
  import type { RepoView } from "../state/repo.svelte";

  let { view, disabled = false, compact = false }: { view: RepoView; disabled?: boolean; compact?: boolean } = $props();

  type Row = { key: "showRemotes" | "showTags" | "showStashes"; label: string; glyph: string };
  const ROWS: Row[] = [
    // A cloud: the remotes' branches.
    { key: "showRemotes", label: "Remote branches", glyph: "M4.9 12.2h6.3a2.8 2.8 0 0 0 .35-5.58 3.7 3.7 0 0 0-7.1-.32A2.95 2.95 0 0 0 4.9 12.2z" },
    // A tag.
    { key: "showTags", label: "Tags", glyph: "M2.6 7.8V3.4c0-.45.35-.8.8-.8h4.4l5.9 5.9-5.2 5.2zM5.6 5.4h.01" },
    // Stacked boxes: stashes. With a branch selection, the stashes on its commits show.
    { key: "showStashes", label: "Stashes", glyph: "M3 5.2h10v7.6H3zM4.6 3h6.8M6.4 8.2h3.2" },
  ];

  const id = $derived(`view-options-${view.repoId}`);
  let open = $state(false);
  let root: HTMLElement;
  let button: HTMLButtonElement;
  let popover: HTMLElement | undefined = $state();

  /** What is off, as the tooltip names it: "tags, stashes". */
  const hidden = $derived(ROWS.filter((r) => !view.viewOptions[r.key]).map((r) => r.label.toLowerCase()));
  const title = $derived.by(() => {
    if (disabled) return "Nothing to show yet: this repository has no commits";
    const lines = compact ? ["View"] : [];
    lines.push(hidden.length > 0 ? `Hidden: ${hidden.join(", ")}` : "What the graph shows");
    return lines.join("\n");
  });

  function boxes(): HTMLInputElement[] {
    return popover ? [...popover.querySelectorAll<HTMLInputElement>("input")] : [];
  }

  async function show() {
    open = true;
    await tick();
    boxes()[0]?.focus();
  }

  function hide(returnFocus: boolean) {
    if (!open) return;
    open = false;
    if (returnFocus) button.focus();
  }

  function onKeydown(e: KeyboardEvent) {
    const all = boxes();
    const at = all.indexOf(document.activeElement as HTMLInputElement);
    switch (e.key) {
      case "ArrowDown":
        all[Math.min(all.length - 1, at + 1)]?.focus();
        break;
      case "ArrowUp":
        all[Math.max(0, at - 1)]?.focus();
        break;
      case "Home":
        all[0]?.focus();
        break;
      case "End":
        all[all.length - 1]?.focus();
        break;
      case "Enter":
        // Space ticks a checkbox by itself; Enter does here too.
        all[at]?.click();
        break;
      case "Escape":
        hide(true);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function onButtonKeydown(e: KeyboardEvent) {
    if (e.key !== "ArrowDown") return;
    e.preventDefault();
    if (open) boxes()[0]?.focus();
    else void show();
  }

  // Closes on a press outside, or when the focus leaves for something outside.
  $effect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (!root.contains(e.target as Node)) hide(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  });

  function onFocusOut(e: FocusEvent) {
    const next = e.relatedTarget as Node | null;
    if (next && !root.contains(next)) hide(false);
  }
</script>

<div class="view-options" bind:this={root} onfocusout={onFocusOut}>
  <button
    class="trigger"
    class:hiding={hidden.length > 0 && !disabled}
    class:open
    class:compact
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-controls={open ? `${id}-dialog` : undefined}
    {title}
    {disabled}
    data-testid="view-options"
    bind:this={button}
    onclick={() => (open ? hide(false) : show())}
    onkeydown={onButtonKeydown}
  >
    <!-- An eye: what the graph shows. -->
    <svg class="lead" width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <path d="M1.3 7C2.7 4.4 4.7 3 7 3s4.3 1.4 5.7 4c-1.4 2.6-3.4 4-5.7 4S2.7 9.6 1.3 7z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" />
      <circle cx="7" cy="7" r="1.75" fill="none" stroke="currentColor" stroke-width="1.3" />
    </svg>
    <span class={compact ? "sr-only" : "label"}>View</span>
    <svg class="chevron" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M2.5 3.8 5 6.3l2.5-2.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  </button>

  {#if open}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="popover" id="{id}-dialog" role="dialog" aria-label="View" tabindex="-1" bind:this={popover} onkeydown={onKeydown}>
      <div class="heading" id="{id}-heading" aria-hidden="true">Show in the graph</div>
      <div role="group" aria-labelledby="{id}-heading">
        {#each ROWS as row (row.key)}
          <label class="row">
            <input
              type="checkbox"
              class="checkbox"
              checked={view.viewOptions[row.key]}
              onchange={(e) => void view.setViewOptions({ [row.key]: e.currentTarget.checked })}
            />
            <svg class="glyph" width="15" height="15" viewBox="0 0 16 16" aria-hidden="true">
              <path d={row.glyph} fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
            <span>{row.label}</span>
          </label>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .view-options {
    position: relative;
    flex: none;
  }

  /* As the branch filter's: a recessed pop-up button, filled on hover and while open. */
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 7px 0 8px;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text);
    font-size: var(--fs-sm);
    transition: background-color 80ms linear, color 80ms linear;
  }

  .trigger.compact {
    gap: 4px;
    padding: 0 6px 0 7px;
  }

  .trigger:hover {
    background: var(--bg-hover);
  }

  .trigger:active,
  .trigger.open {
    background: var(--bg-pressed);
  }

  .trigger:disabled {
    background: transparent;
    color: var(--text-faint);
  }

  /* Something hidden: the button reads in the accent colour, on a faint accent fill. */
  .trigger.hiding {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 11%, transparent);
  }

  .trigger.hiding:hover {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
  }

  .trigger.hiding:active,
  .trigger.hiding.open {
    background: color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .lead,
  .chevron {
    flex: none;
    color: var(--text-muted);
  }

  .hiding .lead,
  .hiding .chevron {
    color: inherit;
  }

  .chevron {
    transition: transform 120ms var(--ease);
  }

  .open .chevron {
    transform: rotate(180deg);
  }

  .label {
    white-space: nowrap;
    font-weight: 500;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .popover {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 30;
    width: 200px;
    padding: 4px 6px 6px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    background: var(--bg-elevated);
    box-shadow: var(--shadow-pop);
    animation: pop 120ms var(--ease);
  }

  .heading {
    display: flex;
    align-items: center;
    height: 24px;
    padding: 0 8px;
    color: var(--text-faint);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.02em;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 28px;
    padding: 0 8px;
    border-radius: var(--radius);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }

  .row:hover,
  .row:has(:focus-visible) {
    background: var(--bg-hover);
  }

  .row:active {
    background: var(--bg-pressed);
  }

  .glyph {
    flex: none;
    display: block;
    color: var(--text-muted);
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
</style>
