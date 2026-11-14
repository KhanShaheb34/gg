<!--
  The name field of a dialog that names something (a branch, a tag): the field, and under it
  why the name cannot be used (faint while nothing is typed, red otherwise), or else `note`.
  `select`: the text it starts with is selected, to be typed over (a rename). `waiting`: the
  operation the dialog's request waits behind, said below.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { visibleText } from "../../format";

  type Props = {
    label: string;
    value: string;
    /** Why the name cannot be used, or null. */
    problem: string | null;
    /** Said under the field when there is no problem. */
    note?: string | null;
    readonly?: boolean;
    select?: boolean;
    /** The title of the operation the request waits behind, or null. */
    waiting?: string | null;
  };

  let {
    label,
    value = $bindable(),
    problem,
    note = null,
    readonly = false,
    select = false,
    waiting = null,
  }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  let input: HTMLInputElement;
  const bad = $derived(problem !== null && value !== "");

  onMount(() => {
    if (select) input.select();
  });
</script>

<div class="field">
  <label class="field-label" for="name-{uid}">{label}</label>
  <input
    id="name-{uid}"
    class="text-input"
    type="text"
    bind:value
    bind:this={input}
    {readonly}
    autocomplete="off"
    autocapitalize="off"
    spellcheck="false"
    aria-invalid={bad}
    aria-describedby="name-hint-{uid}"
  />
  <p class="field-hint" class:problem={bad} id="name-hint-{uid}" aria-live="polite">
    {problem !== null ? visibleText(problem) : note !== null ? visibleText(note) : ""}
  </p>
  {#if waiting !== null}
    <p class="field-hint waiting" aria-live="polite">Waiting for {visibleText(waiting)} to finish…</p>
  {/if}
</div>
