<!--
  A line of a notice: its text, and its names set off as small mono chips (as code is in a
  comment); a path in mono, from the home folder (~/…). A long name is cut in the middle, its
  whole text in its tooltip and in what is read out. In a `compact` line (a short line of
  names, kept on one line) the names are cut to the room there is, the longest first, a short
  one never; in prose, which wraps, each is cut to a length that always fits a line, so a
  sentence wraps around it instead of a name breaking mid-word. Text from git (a branch name,
  a path) is isolated, so it cannot reorder what is around it.
-->
<script lang="ts">
  import { homeInText, shortenPath, visibleText } from "../format";
  import { shown } from "../menus/common";
  import { partsOf, type Rich } from "../notice";
  import { app } from "../state/app.svelte";
  import MiddleText from "./MiddleText.svelte";

  let { line, compact = false }: { line: Rich; compact?: boolean } = $props();
  /** A name this short is never cut in a compact line: a longer one beside it gives way. */
  const SHORT = 16;
  /** Most characters of a name, and of a path, in prose: well within a notice's line. */
  const MAX_REF = 32;
  const MAX_PATH = 40;
  const parts = $derived(partsOf(line));
</script>

{#each parts as part, i (i)}{#if "ref" in part}{#if compact}<span class="name fit" class:short={[...part.ref].length <= SHORT}
        ><MiddleText text={part.ref} keep={Math.min(12, Math.floor(part.ref.length / 2))} /></span
      >{:else}<bdi class="name" title={part.ref}>{shown(part.ref, MAX_REF)}</bdi>{/if}{:else if "path" in part}{#if compact}<span
        class="path fit"
        ><MiddleText text={shortenPath(part.path, app.homeDir)} title={part.path} keep={24} dimThrough="/" /></span
      >{:else}<bdi class="path" title={part.path}>{shown(shortenPath(part.path, app.homeDir), MAX_PATH)}</bdi>{/if}{:else}<bdi
      >{visibleText(homeInText(part.text, app.homeDir))}</bdi
    >{/if}{/each}

<style>
  .name {
    padding: 0 4px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-chrome);
    font-family: var(--font-mono);
    font-size: 0.9em;
    white-space: nowrap;
  }

  .path {
    font-family: var(--font-mono);
    font-size: 0.9em;
    white-space: nowrap;
  }

  /* In a compact line: a box MiddleText measures the room of. */
  .fit {
    display: inline-block;
    max-width: 100%;
    line-height: 16px;
    vertical-align: 1px;
  }

  .fit :global(.mid) {
    display: block;
  }
</style>
