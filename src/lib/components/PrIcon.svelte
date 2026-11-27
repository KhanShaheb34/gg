<!--
  A pull request's glyph, as GitHub draws its kinds: open (the branch comes back into the base
  along an arrow), draft (the way back only dotted), merged (the branch joined), closed (the
  way back crossed out). Its colour is the caller's (currentColor).
-->
<script lang="ts">
  type Props = { kind?: "open" | "draft" | "merged" | "closed"; size?: number };
  let { kind = "open", size = 16 }: Props = $props();
</script>

<svg class="pr-icon" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
  {#if kind === "merged"}
    <circle cx="4" cy="3.6" r="1.7" fill="none" stroke="currentColor" stroke-width="1.4" />
    <circle cx="4" cy="12.4" r="1.7" fill="none" stroke="currentColor" stroke-width="1.4" />
    <circle cx="12" cy="8.4" r="1.7" fill="none" stroke="currentColor" stroke-width="1.4" />
    <path d="M4 5.3v5.4M4 5.3c0 2 1.8 3.1 3.8 3.1h2.5" fill="none" stroke="currentColor" stroke-width="1.4" />
  {:else}
    <circle cx="4" cy="3.6" r="1.7" fill="none" stroke="currentColor" stroke-width="1.4" />
    <circle cx="4" cy="12.4" r="1.7" fill="none" stroke="currentColor" stroke-width="1.4" />
    <circle cx="12" cy="12.4" r="1.7" fill="none" stroke="currentColor" stroke-width="1.4" />
    <path d="M4 5.3v5.4" fill="none" stroke="currentColor" stroke-width="1.4" />
    {#if kind === "open"}
      <path d="M12 10.7V6.1c0-1.4-.9-2.3-2.3-2.3H7.4" fill="none" stroke="currentColor" stroke-width="1.4" />
      <path d="M8.9 2.1 7.2 3.8l1.7 1.7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
    {:else if kind === "draft"}
      <circle cx="12" cy="8.2" r="0.95" fill="currentColor" />
      <circle cx="12" cy="4.4" r="0.95" fill="currentColor" />
    {:else}
      <path d="M10.3 2.6l3.4 3.4M13.7 2.6l-3.4 3.4M12 8.4v2.3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
    {/if}
  {/if}
</svg>

<style>
  .pr-icon {
    display: block;
    flex: none;
  }
</style>
