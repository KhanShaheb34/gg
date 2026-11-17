<!--
  A row of mutually exclusive choices (a radio group drawn as one segmented control): the
  chosen one raised. Arrow keys move the choice, as in a radio group; only the chosen one is
  in the Tab order.
-->
<script lang="ts" generics="T extends string | number">
  type Props = {
    label: string;
    options: { value: T; label: string; title?: string }[];
    value: T;
    onchange: (value: T) => void;
    testid?: string;
  };

  let { label, options, value, onchange, testid }: Props = $props();

  let group: HTMLDivElement;

  function onKeydown(e: KeyboardEvent) {
    const at = options.findIndex((o) => o.value === value);
    let next = -1;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (at + 1) % options.length;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (at - 1 + options.length) % options.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = options.length - 1;
    if (next < 0) return;
    e.preventDefault();
    onchange(options[next].value);
    queueMicrotask(() => group?.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus());
  }
</script>

<div class="segmented" role="radiogroup" aria-label={label} tabindex="-1" bind:this={group} onkeydown={onKeydown} data-testid={testid}>
  {#each options as option (option.value)}
    {@const chosen = option.value === value}
    <button
      type="button"
      role="radio"
      class="segment"
      aria-checked={chosen}
      tabindex={chosen ? 0 : -1}
      title={option.title}
      onclick={() => onchange(option.value)}>{option.label}</button
    >
  {/each}
</div>

<style>
  .segmented {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 2px;
    max-width: 100%;
    padding: 2px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-chrome);
  }

  .segment {
    min-width: 0;
    height: 24px;
    padding: 0 11px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--fs-sm);
    font-weight: 500;
    white-space: nowrap;
    transition:
      background-color 80ms linear,
      color 80ms linear;
  }

  .segment:hover:not([aria-checked="true"]) {
    background: var(--bg-hover);
    color: var(--text);
  }

  .segment[aria-checked="true"] {
    border-color: var(--border-strong);
    background: var(--bg-elevated);
    color: var(--text);
    font-weight: 600;
    box-shadow: var(--shadow-tab);
  }

  .segment:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 1px;
  }
</style>
