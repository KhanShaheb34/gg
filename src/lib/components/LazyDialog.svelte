<!--
  A dialog whose code loads when it is first opened (see `lazyDialog` in overlays.svelte.ts):
  the dialog itself, with the props it was opened with, once its module is there (a few ms,
  from the app's own files); nothing before. If it cannot load, it closes and says so in the
  console (the app's files are its own: this is not expected).
-->
<script lang="ts">
  import type { Component } from "svelte";

  type Props = { load: () => Promise<{ default: Component<any> }>; props: Record<string, unknown> };
  let { load, props }: Props = $props();

  // svelte-ignore state_referenced_locally
  const loading = load().catch((e: unknown) => {
    console.error("a dialog could not be loaded", e);
    (props.close as (() => void) | undefined)?.();
    return null;
  });
</script>

{#await loading then mod}
  {#if mod}
    <mod.default {...props} />
  {/if}
{/await}
